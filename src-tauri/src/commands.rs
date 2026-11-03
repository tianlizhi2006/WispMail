//! Tauri 账户命令。数据库与系统凭据操作放到阻塞工作线程中，数据库锁只在本地读写期间持有；
//! 网络验证在锁外进行，避免验证账户时阻塞离线阅读。

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, Weak},
};

use tauri::Manager;

use crate::account::{Account, AccountInput, AccountService};
use crate::inbox_watch::InboxWatchers;
use crate::sync::{self, ConnectionReport, ImapPool};

pub struct AccountsState {
    accounts: Arc<Mutex<Result<AccountService, String>>>,
    network_limit: Arc<tokio::sync::Semaphore>,
    account_connections: Mutex<HashMap<String, Weak<tokio::sync::Semaphore>>>,
    pub(crate) imap: ImapPool,
    /// 正在后台预取正文的 (账户, 文件夹)，避免重复启动预取任务。
    prefetching: Mutex<HashSet<(String, String)>>,
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
    let report = sync::test_connection(&account, &credentials).await;
    if !report.incoming.success || !report.outgoing.success {
        let mut failures = Vec::new();
        if !report.incoming.success {
            failures.push(format!("IMAP：{}", report.incoming.message));
        }
        if !report.outgoing.success {
            failures.push(format!("SMTP：{}", report.outgoing.message));
        }
        return Err(format!("账户未保存。{}", failures.join("；")));
    }
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

#[tauri::command]
pub async fn delete_account(
    id: String,
    app: tauri::AppHandle,
    state: tauri::State<'_, AccountsState>,
) -> Result<(), String> {
    let account_id = id.clone();
    with_accounts(&state, move |service| service.delete_account(&id)).await?;
    state.imap.forget(&account_id);
    app.state::<InboxWatchers>().stop(&account_id);
    Ok(())
}
