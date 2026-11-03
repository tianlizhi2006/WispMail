//! Tauri 账户命令。数据库与系统凭据操作放到阻塞工作线程中，数据库锁只在本地读写期间持有；
//! 网络验证在锁外进行，避免验证账户时阻塞离线阅读。

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, Weak},
};

use tauri::Manager;

use crate::account::{
    Account, AccountCredentials, AccountInput, AccountService, AuthMethod, IncomingProtocol,
};
use crate::inbox_watch::InboxWatchers;
use crate::oauth::{self, CachedToken, OAuthProviderInfo, OAuthState};
use crate::sync::{self, ConnectionReport, ImapPool};

pub struct AccountsState {
    accounts: Arc<Mutex<Result<AccountService, String>>>,
    network_limit: Arc<tokio::sync::Semaphore>,
    account_connections: Mutex<HashMap<String, Weak<tokio::sync::Semaphore>>>,
    pub(crate) imap: ImapPool,
    /// 正在后台预取正文的 (账户, 文件夹)，避免重复启动预取任务。
    prefetching: Mutex<HashSet<(String, String)>>,
    pub(crate) oauth: OAuthState,
}

pub(crate) struct ConnectionPermit {
    _account: tokio::sync::OwnedSemaphorePermit,
    _global: tokio::sync::OwnedSemaphorePermit,
}

impl AccountsState {
    pub fn new(service: Result<AccountService, String>) -> Self {
        Self {
            accounts: Arc::new(Mutex::new(service)),
            network_limit: Arc::new(tokio::sync::Semaphore::new(4)),
            account_connections: Mutex::new(HashMap::new()),
            imap: ImapPool::default(),
            prefetching: Mutex::new(HashSet::new()),
            oauth: OAuthState::default(),
        }
    }

    /// 标记某文件夹开始预取；已有同一文件夹的预取在运行时返回 false。
    pub(crate) fn begin_prefetch(&self, account_id: &str, folder: &str) -> bool {
        self.prefetching
            .lock()
            .is_ok_and(|mut set| set.insert((account_id.to_string(), folder.to_string())))
    }

    pub(crate) fn end_prefetch(&self, account_id: &str, folder: &str) {
        if let Ok(mut set) = self.prefetching.lock() {
            set.remove(&(account_id.to_string(), folder.to_string()));
        }
    }

    pub(crate) async fn network_slot(&self, account_id: &str) -> Result<ConnectionPermit, String> {
        // 同一账户按序连接，避免较早的同步覆盖新状态；不同账户最多并行四个。
        let account_gate = {
            let mut gates = self
                .account_connections
                .lock()
                .map_err(|_| "邮件连接服务暂时不可用。".to_string())?;
            gates.retain(|_, gate| gate.strong_count() > 0);
            if let Some(gate) = gates.get(account_id).and_then(Weak::upgrade) {
                gate
            } else {
                let gate = Arc::new(tokio::sync::Semaphore::new(1));
                gates.insert(account_id.to_string(), Arc::downgrade(&gate));
                gate
            }
        };
        let account_permit = account_gate
            .acquire_owned()
            .await
            .map_err(|_| "邮件连接服务已停止，请重启应用。".to_string())?;
        let global_permit = Arc::clone(&self.network_limit)
            .acquire_owned()
            .await
            .map_err(|_| "邮件连接服务已停止，请重启应用。".to_string())?;
        Ok(ConnectionPermit {
            _account: account_permit,
            _global: global_permit,
        })
    }
}

impl AccountsState {
    /// 把保存的凭据换成本次连接使用的登录信息：密码账户原样返回；
    /// OAuth 账户用刷新令牌换取（或复用缓存的）访问令牌，并保存服务商轮换后的新刷新令牌。
    pub(crate) async fn live_credentials(
        &self,
        account: &Account,
        stored: AccountCredentials,
    ) -> Result<AccountCredentials, String> {
        if account.auth != AuthMethod::OAuth2 {
            return Ok(stored);
        }
        let provider = oauth::provider_for_account(account)?;
        let refresh_token = stored.incoming_password;
        let mut tokens = self.oauth.tokens.lock().await;
        if let Some(access) = tokens
            .get(&account.id)
            .and_then(|cached| cached.usable_for(&refresh_token))
        {
            return Ok(access_credentials(access));
        }
        let refreshed = oauth::refresh(provider, &refresh_token).await?;
        let mut current = refresh_token;
        if let Some(rotated) = refreshed
            .refresh_token
            .clone()
            .filter(|token| *token != current)
        {
            let id = account.id.clone();
            let token = rotated.clone();
            // 保存失败时旧刷新令牌通常仍然有效，下次连接会再次尝试。
            if with_accounts(self, move |service| {
                service.replace_oauth_token(&id, &token)
            })
            .await
            .is_ok()
            {
                current = rotated;
            }
        }
        let credentials = access_credentials(&refreshed.access_token);
        tokens.insert(account.id.clone(), CachedToken::new(current, &refreshed));
        Ok(credentials)
    }
}

fn access_credentials(access_token: &str) -> AccountCredentials {
    AccountCredentials {
        incoming_password: access_token.to_string(),
        outgoing_password: access_token.to_string(),
    }
}

pub(crate) async fn with_accounts<T: Send + 'static>(
    state: &AccountsState,
    operation: impl FnOnce(&mut AccountService) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let accounts = Arc::clone(&state.accounts);
    tauri::async_runtime::spawn_blocking(move || {
        let mut state = accounts
            .lock()
            .map_err(|_| "账户服务暂时不可用，请重启应用。".to_string())?;
        let service = state.as_mut().map_err(|message| message.clone())?;
        operation(service)
    })
    .await
    .map_err(|_| "账户操作未能完成，请重试。".to_string())?
}

#[tauri::command]
pub async fn list_accounts(state: tauri::State<'_, AccountsState>) -> Result<Vec<Account>, String> {
    with_accounts(&state, |service| service.list_accounts()).await
}

#[tauri::command]
pub async fn test_account_connection(
    input: AccountInput,
    state: tauri::State<'_, AccountsState>,
) -> Result<ConnectionReport, String> {
    let (account, credentials) =
        with_accounts(&state, move |service| service.resolve_input(input)).await?;
    let credentials = state.live_credentials(&account, credentials).await?;
    Ok(sync::test_connection(&account, &credentials).await)
}

#[tauri::command]
pub async fn save_account(
    input: AccountInput,
    app: tauri::AppHandle,
    state: tauri::State<'_, AccountsState>,
) -> Result<Account, String> {
    let (account, credentials) =
        with_accounts(&state, move |service| service.resolve_input(input)).await?;
    // 不信任前端之前的测试结果：保存时始终验证当前配置与凭据。验证在锁外进行，
    // 保存时 save_verified_account 会重新检查邮箱唯一性。
    let live = state
        .live_credentials(&account, credentials.clone_secret())
        .await?;
    ensure_verified(sync::test_connection(&account, &live).await)?;
    let account_id = account.id.clone();
    let saved = with_accounts(&state, move |service| {
        service.save_verified_account(account, &credentials)
    })
    .await?;
    // 凭据可能已更换，丢弃旧凭据登录的空闲连接，并用新设置重新监视收件箱。
    state.imap.forget(&account_id);
    app.state::<InboxWatchers>().start(&app, &saved.id);
    Ok(saved)
}

fn ensure_verified(report: ConnectionReport) -> Result<(), String> {
    if report.incoming.success && report.outgoing.success {
        return Ok(());
    }
    let mut failures = Vec::new();
    if !report.incoming.success {
        failures.push(format!("IMAP：{}", report.incoming.message));
    }
    if !report.outgoing.success {
        failures.push(format!("SMTP：{}", report.outgoing.message));
    }
    Err(format!("账户未保存。{}", failures.join("；")))
}

#[tauri::command]
pub fn oauth_providers() -> Vec<OAuthProviderInfo> {
    oauth::provider_infos()
}

/// 取消进行中的浏览器登录。
#[tauri::command]
pub fn cancel_oauth_sign_in(state: tauri::State<'_, AccountsState>) {
    if let Some(pending) = state
        .oauth
        .pending
        .lock()
        .ok()
        .and_then(|mut slot| slot.take())
    {
        let _ = pending.control.send(oauth::SignInControl::Cancel);
    }
}

/// 浏览器没能回到 WispMail 时，用户粘贴地址栏中的回调地址继续登录。
#[tauri::command]
pub fn submit_oauth_redirect(
    url: String,
    state: tauri::State<'_, AccountsState>,
) -> Result<(), String> {
    let pending = state
        .oauth
        .pending
        .lock()
        .map_err(|_| "登录服务暂时不可用。".to_string())?;
    let pending = pending
        .as_ref()
        .ok_or("当前没有进行中的登录，请重新点击登录。")?;
    let code = oauth::code_from_pasted_redirect(&url, &pending.state)?;
    pending
        .control
        .send(oauth::SignInControl::Code(code))
        .map_err(|_| "登录已结束，请重新点击登录。".to_string())
}

pub const OAUTH_PROGRESS_EVENT: &str = "oauth-progress";

/// 在系统浏览器中登录服务商账户，验证收发信后保存。传入 `account_id` 时为已有账户重新授权，
/// 登录的必须是同一个邮箱。
#[tauri::command]
pub async fn sign_in_oauth(
    provider: String,
    account_id: Option<String>,
    app: tauri::AppHandle,
    state: tauri::State<'_, AccountsState>,
) -> Result<Account, String> {
    use tauri_plugin_opener::OpenerExt;
    let provider = oauth::provider_by_id(&provider).ok_or("不支持该登录方式。")?;
    let existing = match account_id {
        Some(id) => Some(with_accounts(&state, move |service| service.get_account(&id)).await?),
        None => None,
    };
    use tauri::Emitter;
    let (control, controls) = tokio::sync::mpsc::unbounded_channel();
    let sign_in_state = oauth::random_token();
    {
        let mut pending = state
            .oauth
            .pending
            .lock()
            .map_err(|_| "登录服务暂时不可用。".to_string())?;
        if pending
            .as_ref()
            .is_some_and(|pending| !pending.control.is_closed())
        {
            return Err("已有一个登录正在进行，请先在浏览器中完成或取消。".into());
        }
        *pending = Some(oauth::PendingSignIn {
            control,
            state: sign_in_state.clone(),
        });
    }
    let opener = app.clone();
    let progress = |stage: oauth::SignInStage| {
        let _ = app.emit(OAUTH_PROGRESS_EVENT, stage);
    };
    let signed_in = oauth::sign_in(
        provider,
        existing.as_ref().map(|account| account.email.as_str()),
        &sign_in_state,
        move |url| {
            opener
                .opener()
                .open_url(url, None::<&str>)
                .map_err(|_| "无法打开系统浏览器，请检查默认浏览器设置。".to_string())
        },
        controls,
        progress,
    )
    .await;
    if let Ok(mut pending) = state.oauth.pending.lock() {
        pending.take();
    }
    let oauth::SignedIn { identity, tokens } = signed_in?;
    if let Some(account) = &existing
        && !account.email.eq_ignore_ascii_case(&identity.email)
    {
        return Err(format!(
            "登录的是 {}，与该账户的 {} 不一致，授权未更新。",
            identity.email, account.email
        ));
    }
    let email = existing
        .as_ref()
        .map(|account| account.email.clone())
        .unwrap_or_else(|| identity.email.clone());
    let display_name = match &existing {
        Some(account) => account.display_name.clone(),
        None if !identity.name.is_empty() => identity.name.clone(),
        None => email.split('@').next().unwrap_or_default().to_string(),
    };
    let account = Account {
        id: existing
            .as_ref()
            .map(|account| account.id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        incoming_username: email.clone(),
        outgoing_username: email.clone(),
        email,
        display_name,
        use_same_password: true,
        incoming_protocol: IncomingProtocol::Imap,
        incoming: provider.incoming(),
        outgoing: provider.outgoing(identity.personal),
        auth: AuthMethod::OAuth2,
    };
    progress(oauth::SignInStage::Verifying);
    let report = sync::test_connection(&account, &access_credentials(&tokens.access_token)).await;
    ensure_verified(report)?;
    progress(oauth::SignInStage::Saving);
    let refresh_token = tokens.refresh_token.clone().unwrap_or_default();
    let stored = AccountCredentials {
        incoming_password: refresh_token.clone(),
        outgoing_password: refresh_token.clone(),
    };
    let saved = with_accounts(&state, move |service| {
        service.save_verified_account(account, &stored)
    })
    .await?;
    state
        .oauth
        .tokens
        .lock()
        .await
        .insert(saved.id.clone(), CachedToken::new(refresh_token, &tokens));
    state.imap.forget(&saved.id);
    app.state::<InboxWatchers>().start(&app, &saved.id);
    Ok(saved)
}

#[tauri::command]
pub async fn delete_account(
    id: String,
    app: tauri::AppHandle,
    state: tauri::State<'_, AccountsState>,
) -> Result<(), String> {
    let account_id = id.clone();
    with_accounts(&state, move |service| service.delete_account(&id)).await?;
    state.imap.forget(&account_id);
    state.oauth.forget(&account_id).await;
    app.state::<InboxWatchers>().stop(&account_id);
    Ok(())
}
