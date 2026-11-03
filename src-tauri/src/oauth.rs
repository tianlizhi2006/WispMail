//! OAuth2 登录（授权码 + PKCE，回环地址重定向），为 IMAP / SMTP 换取访问令牌。
//!
//! - 应用 ID（Client ID）在编译时通过环境变量内置；开发时也可在运行时用同名环境变量覆盖。
//! - 刷新令牌保存在系统凭据管理器，访问令牌只在内存中缓存。
//! - 令牌只会发给对应服务商自己的邮件服务器（见 [`OAuthProvider::allows_servers`]），
//!   防止被篡改的账户配置把令牌发送到其他主机。

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::mpsc,
    time::timeout,
};

use crate::account::{Account, AuthMethod, Encryption, ServerConfig};

/// 访问令牌剩余有效期少于此值时提前刷新，避免长连接或发信途中过期。
const REFRESH_MARGIN: Duration = Duration::from_secs(5 * 60);
/// 等待用户在浏览器中完成登录的最长时间。
pub const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
/// 微软个人账户（Outlook.com / Hotmail）所在的固定租户。
const MICROSOFT_CONSUMER_TENANT: &str = "9188040d-6c67-4c5b-b112-36a304b66dad";

pub struct OAuthProvider {
    pub id: &'static str,
    pub name: &'static str,
    authorize_url: &'static str,
    token_url: &'static str,
    scopes: &'static str,
    client_id_env: &'static str,
    builtin_client_id: Option<&'static str>,
    imap: (&'static str, u16, Encryption),
    /// 允许的发信服务器；第一个用于个人账户，第二个用于工作或学校账户。
    smtp: &'static [(&'static str, u16, Encryption)],
}

pub const MICROSOFT: OAuthProvider = OAuthProvider {
    id: "microsoft",
    name: "Microsoft",
    authorize_url: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
    token_url: "https://login.microsoftonline.com/common/oauth2/v2.0/token",
    scopes: "offline_access openid email profile https://outlook.office.com/IMAP.AccessAsUser.All https://outlook.office.com/SMTP.Send",
    client_id_env: "WISPMAIL_MICROSOFT_CLIENT_ID",
    builtin_client_id: option_env!("WISPMAIL_MICROSOFT_CLIENT_ID"),
    imap: ("outlook.office365.com", 993, Encryption::Tls),
    smtp: &[
        ("smtp-mail.outlook.com", 587, Encryption::StartTls),
        ("smtp.office365.com", 587, Encryption::StartTls),
    ],
};

pub const PROVIDERS: &[&OAuthProvider] = &[&MICROSOFT];

fn server((host, port, encryption): (&str, u16, Encryption)) -> ServerConfig {
    ServerConfig {
        host: host.to_string(),
        port,
        encryption,
    }
}

fn valid_client_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".-_".contains(&byte))
}

impl OAuthProvider {
    pub fn client_id(&self) -> Option<String> {
        std::env::var(self.client_id_env)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| valid_client_id(value))
            .or_else(|| {
                self.builtin_client_id
                    .map(str::trim)
                    .filter(|value| valid_client_id(value))
                    .map(str::to_string)
            })
    }

    pub fn incoming(&self) -> ServerConfig {
        server(self.imap)
    }

    /// 个人账户与工作 / 学校账户使用不同的发信服务器。
    pub fn outgoing(&self, personal: bool) -> ServerConfig {
        let index = usize::from(!personal).min(self.smtp.len() - 1);
        server(self.smtp[index])
    }

    pub fn allows_servers(&self, incoming: &ServerConfig, outgoing: &ServerConfig) -> bool {
        *incoming == self.incoming() && self.smtp.iter().any(|item| *outgoing == server(*item))
    }
}

/// 按收信服务器找到账户所属的 OAuth 服务商。
pub fn provider_for(incoming: &ServerConfig) -> Option<&'static OAuthProvider> {
    PROVIDERS
        .iter()
        .copied()
        .find(|provider| provider.incoming() == *incoming)
}

pub fn provider_by_id(id: &str) -> Option<&'static OAuthProvider> {
    PROVIDERS.iter().copied().find(|provider| provider.id == id)
}

/// 账户使用 OAuth 时返回其服务商，并确认收发信服务器都属于该服务商。
pub fn provider_for_account(account: &Account) -> Result<&'static OAuthProvider, String> {
    provider_for(&account.incoming)
        .filter(|provider| provider.allows_servers(&account.incoming, &account.outgoing))
        .ok_or_else(|| "该账户的服务器不属于支持 OAuth 登录的服务商，已拒绝发送登录令牌。".into())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthProviderInfo {
    pub id: &'static str,
    pub name: &'static str,
    /// 此版本是否内置了应用 ID；未内置时无法登录。
    pub configured: bool,
}

pub fn provider_infos() -> Vec<OAuthProviderInfo> {
    PROVIDERS
        .iter()
        .map(|provider| OAuthProviderInfo {
            id: provider.id,
            name: provider.name,
            configured: provider.client_id().is_some(),
        })
        .collect()
}

// ---------------------------------------------------------------- PKCE 与授权地址

pub(crate) fn random_token() -> String {
    let mut bytes = [0u8; 32];
    bytes[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    bytes[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    URL_SAFE_NO_PAD.encode(bytes)
}

pub(crate) fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

pub(crate) fn authorize_url(
    provider: &OAuthProvider,
    client_id: &str,
    redirect_uri: &str,
    challenge: &str,
    state: &str,
    login_hint: Option<&str>,
) -> String {
    let mut url = url::Url::parse(provider.authorize_url).expect("valid provider URL");
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("client_id", client_id)
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("response_mode", "query")
            .append_pair("scope", provider.scopes)
            .append_pair("state", state)
            .append_pair("code_challenge", challenge)
            .append_pair("code_challenge_method", "S256");
        match login_hint {
            Some(hint) => query.append_pair("login_hint", hint),
            None => query.append_pair("prompt", "select_account"),
        };
    }
    url.into()
}

// ---------------------------------------------------------------- 回环重定向

/// 只监听本机回环地址；IPv6 监听失败时仅使用 IPv4。
pub(crate) struct Loopback {
    v4: TcpListener,
    v6: Option<TcpListener>,
    port: u16,
}

impl Loopback {
    pub(crate) async fn bind() -> Result<Self, String> {
        let v4 = TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|_| "无法在本机开启登录回调端口。".to_string())?;
        let port = v4
            .local_addr()
            .map_err(|_| "无法在本机开启登录回调端口。".to_string())?
            .port();
        let v6 = TcpListener::bind(("::1", port)).await.ok();
        Ok(Self { v4, v6, port })
    }

    pub(crate) fn redirect_uri(&self) -> String {
        format!("http://localhost:{}", self.port)
    }

    async fn accept(&self) -> std::io::Result<TcpStream> {
        match &self.v6 {
            Some(v6) => tokio::select! {
                result = self.v4.accept() => result.map(|(stream, _)| stream),
                result = v6.accept() => result.map(|(stream, _)| stream),
            },
            None => self.v4.accept().await.map(|(stream, _)| stream),
        }
    }

    /// 等待浏览器带着授权码回到本机。state 不匹配的请求被忽略，防止其他网页伪造回调。
    pub(crate) async fn wait_for_code(&self, expected_state: &str) -> Result<String, String> {
        loop {
            let Ok(mut stream) = self.accept().await else {
                continue;
            };
            let Ok(Ok(target)) =
                timeout(Duration::from_secs(10), read_request_target(&mut stream)).await
            else {
                continue;
            };
            match callback_result(&target, expected_state) {
                Callback::Ignore => respond(&mut stream, "404 Not Found", "未找到页面。").await,
                Callback::Code(code) => {
                    respond(
                        &mut stream,
                        "200 OK",
                        "登录完成，可以关闭此页面并返回 WispMail。",
                    )
                    .await;
                    return Ok(code);
                }
                Callback::Denied(message) => {
                    respond(&mut stream, "200 OK", &message).await;
                    return Err(message);
                }
            }
        }
    }
}

async fn read_request_target(stream: &mut TcpStream) -> std::io::Result<String> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];
    while !buffer.windows(4).any(|window| window == b"\r\n\r\n") && buffer.len() < 16 * 1024 {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let text = String::from_utf8_lossy(&buffer);
    let mut parts = text.lines().next().unwrap_or_default().split(' ');
    match (parts.next(), parts.next()) {
        (Some("GET"), Some(target)) if target.starts_with('/') => Ok(target.to_string()),
        _ => Ok(String::new()),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Callback {
    Ignore,
    Code(String),
    Denied(String),
}

pub(crate) fn callback_result(target: &str, expected_state: &str) -> Callback {
    let Ok(url) = url::Url::parse(&format!("http://localhost{target}")) else {
        return Callback::Ignore;
    };
    if url.path() != "/" {
        return Callback::Ignore;
    }
    let params: HashMap<String, String> = url.query_pairs().into_owned().collect();
    if params.get("state").map(String::as_str) != Some(expected_state) {
        return Callback::Ignore;
    }
    if let Some(error) = params.get("error") {
        return Callback::Denied(match error.as_str() {
            "access_denied" => "你取消了授权，账户未添加。".into(),
            "consent_required" | "interaction_required" => {
                "需要在浏览器中同意 WispMail 访问邮箱后才能继续。".into()
            }
            _ => "服务商拒绝了登录请求，请稍后重试；工作或学校账户可能需要管理员批准。".into(),
        });
    }
    match params.get("code") {
        Some(code) if !code.is_empty() && code.len() <= 4096 => Callback::Code(code.clone()),
        _ => Callback::Ignore,
    }
}

async fn respond(stream: &mut TcpStream, status: &str, message: &str) {
    let body = format!(
        "<!DOCTYPE html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\"><title>WispMail</title></head>\
         <body style=\"font-family:system-ui,sans-serif;display:grid;place-items:center;height:90vh;color:#24302b\">\
         <p style=\"font-size:18px\">{message}</p></body></html>"
    );
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

// ---------------------------------------------------------------- 令牌

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    id_token: Option<String>,
}

#[derive(Deserialize)]
struct TokenError {
    error: String,
    #[serde(default)]
    error_description: String,
}

/// 令牌请求的用途：首次登录用授权码换取，之后用刷新令牌更新。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Grant {
    Code,
    Refresh,
}

/// 从微软的错误说明中取出错误码（如 AADSTS7000218），便于对照文档排查。
fn error_code(description: &str) -> Option<&str> {
    let start = description.find("AADSTS")?;
    let rest = &description[start..];
    let end = rest[6..]
        .find(|c: char| !c.is_ascii_digit())
        .map_or(rest.len(), |index| index + 6);
    (end > 6).then(|| &rest[..end])
}

fn token_error_message(
    provider: &OAuthProvider,
    grant: Grant,
    error: &str,
    description: &str,
) -> String {
    let code = error_code(description);
    let message = match (code, error) {
        // 应用被注册为“Web”或“单页应用”平台时，桌面应用无法换取令牌。
        (Some("AADSTS7000218" | "AADSTS9002327" | "AADSTS700025"), _) => {
            "应用注册的平台类型不正确：请在 Entra 的“身份验证”中把 http://localhost 放在“移动和桌面应用程序”平台下，并开启“允许公共客户端流”。".to_string()
        }
        (Some("AADSTS50011"), _) => {
            "回调地址未登记：请在 Entra 应用的“移动和桌面应用程序”平台中添加 http://localhost。".to_string()
        }
        (_, "invalid_grant" | "interaction_required") if grant == Grant::Code => {
            "授权码无效或已过期，请重新登录。".to_string()
        }
        (_, "invalid_grant" | "interaction_required") => format!(
            "{} 账户的授权已失效（可能修改了密码或撤销了授权），请在账户设置中重新登录。",
            provider.name
        ),
        (_, "invalid_client" | "unauthorized_client") => {
            "此版本内置的应用 ID 无效或未正确配置，请联系开发者。".to_string()
        }
        _ => format!("{} 登录服务拒绝了请求，请稍后重试。", provider.name),
    };
    match code {
        Some(code) => format!("{message}（微软错误码 {code}）"),
        None if !error.is_empty() => format!("{message}（{error}）"),
        None => message,
    }
}

pub struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Instant,
    id_token: Option<String>,
}

fn valid_token(value: &str) -> bool {
    !value.is_empty() && value.len() <= 16 * 1024 && !value.chars().any(char::is_control)
}

async fn token_request(
    provider: &OAuthProvider,
    grant: Grant,
    form: &[(&str, &str)],
) -> Result<Tokens, String> {
    let client = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|_| "无法初始化网络连接。".to_string())?;
    let response = client
        .post(provider.token_url)
        .form(form)
        .send()
        .await
        .map_err(|_| format!("无法连接 {} 登录服务，请检查网络后重试。", provider.name))?;
    let status = response.status();
    let body = response
        .bytes()
        .await
        .map_err(|_| format!("{} 登录服务响应中断，请重试。", provider.name))?;
    if !status.is_success() {
        let (error, description) = serde_json::from_slice::<TokenError>(&body)
            .map(|error| (error.error, error.error_description))
            .unwrap_or_else(|_| (format!("HTTP {}", status.as_u16()), String::new()));
        return Err(token_error_message(provider, grant, &error, &description));
    }
    let tokens: TokenResponse = serde_json::from_slice(&body)
        .map_err(|_| format!("{} 登录服务返回了无法识别的结果。", provider.name))?;
    if !valid_token(&tokens.access_token)
        || tokens
            .refresh_token
            .as_deref()
            .is_some_and(|token| !valid_token(token))
    {
        return Err(format!("{} 登录服务返回了无效的令牌。", provider.name));
    }
    Ok(Tokens {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        expires_at: Instant::now() + Duration::from_secs(tokens.expires_in.unwrap_or(3600)),
        id_token: tokens.id_token,
    })
}

pub async fn refresh(provider: &OAuthProvider, refresh_token: &str) -> Result<Tokens, String> {
    let client_id = provider
        .client_id()
        .ok_or("此版本未内置应用 ID，无法刷新登录状态。")?;
    token_request(
        provider,
        Grant::Refresh,
        &[
            ("client_id", client_id.as_str()),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("scope", provider.scopes),
        ],
    )
    .await
}

/// 登录得到的账户身份。
pub struct Identity {
    pub email: String,
    pub name: String,
    /// 微软个人账户（Outlook.com / Hotmail）。
    pub personal: bool,
}

#[derive(Deserialize)]
struct Claims {
    email: Option<String>,
    preferred_username: Option<String>,
    name: Option<String>,
    tid: Option<String>,
}

/// id_token 直接来自令牌接口的 TLS 响应，这里只读取其中的身份信息。
pub(crate) fn identity(id_token: &str) -> Result<Identity, String> {
    let invalid = || "无法从登录结果中读取邮箱地址。".to_string();
    let payload = id_token.split('.').nth(1).ok_or_else(invalid)?;
    let claims: Claims = URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or_else(invalid)?;
    let email = [claims.email, claims.preferred_username]
        .into_iter()
        .flatten()
        .map(|value| value.trim().to_string())
        .find(|value| crate::account::normalized_email(value).is_some())
        .ok_or_else(|| "此账户没有可用的邮箱地址，无法添加。".to_string())?;
    let name = claims
        .name
        .map(|name| name.trim().chars().take(100).collect::<String>())
        .filter(|name| !name.is_empty() && !name.chars().any(char::is_control))
        .unwrap_or_default();
    Ok(Identity {
        email: crate::account::normalized_email(&email).unwrap_or(email),
        name,
        personal: claims.tid.as_deref() == Some(MICROSOFT_CONSUMER_TENANT),
    })
}

pub struct SignedIn {
    pub identity: Identity,
    pub tokens: Tokens,
}

/// 进行中的浏览器登录可以接收的指令。
pub enum SignInControl {
    Cancel,
    /// 用户从浏览器地址栏粘贴回来的授权码（已核对 state）。
    Code(String),
}

pub(crate) struct PendingSignIn {
    pub(crate) control: mpsc::UnboundedSender<SignInControl>,
    pub(crate) state: String,
}

/// 浏览器停在回调地址却没有回到 WispMail 时（如代理插件拦截了 localhost），
/// 用户可以粘贴地址栏中的完整地址。只接受本机回调地址和本次登录的 state。
pub(crate) fn code_from_pasted_redirect(pasted: &str, state: &str) -> Result<String, String> {
    let invalid = || {
        "这不是本次登录的回调地址。请复制浏览器地址栏中以 http://localhost 开头、包含 code= 的完整地址。".to_string()
    };
    let url = url::Url::parse(pasted.trim()).map_err(|_| invalid())?;
    if !matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")) {
        return Err(invalid());
    }
    let target = format!("{}?{}", url.path(), url.query().unwrap_or_default());
    match callback_result(&target, state) {
        Callback::Code(code) => Ok(code),
        Callback::Denied(message) => Err(message),
        Callback::Ignore => Err(invalid()),
    }
}

/// 登录进度，界面据此提示当前步骤。
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SignInStage {
    Exchanging,
    Verifying,
    Saving,
}

/// 完整的浏览器登录：打开授权页，等待回调（或用户粘贴的回调地址），用授权码换取令牌。
/// `open` 负责用系统浏览器打开地址；`control` 用于取消或提交粘贴的授权码。
pub async fn sign_in(
    provider: &OAuthProvider,
    login_hint: Option<&str>,
    state: &str,
    open: impl FnOnce(String) -> Result<(), String>,
    mut control: mpsc::UnboundedReceiver<SignInControl>,
    progress: impl Fn(SignInStage),
) -> Result<SignedIn, String> {
    let client_id = provider.client_id().ok_or_else(|| {
        format!(
            "此版本未内置 {} 应用 ID，暂时无法使用该方式登录。",
            provider.name
        )
    })?;
    let loopback = Loopback::bind().await?;
    let redirect_uri = loopback.redirect_uri();
    let verifier = random_token();
    open(authorize_url(
        provider,
        &client_id,
        &redirect_uri,
        &pkce_challenge(&verifier),
        state,
        login_hint,
    ))?;
    let code = tokio::select! {
        result = timeout(SIGN_IN_TIMEOUT, loopback.wait_for_code(state)) => {
            result.map_err(|_| "等待浏览器登录超时，请重新尝试。".to_string())??
        }
        message = control.recv() => match message {
            Some(SignInControl::Code(code)) => code,
            Some(SignInControl::Cancel) | None => return Err("已取消登录。".into()),
        },
    };
    progress(SignInStage::Exchanging);
    let tokens = token_request(
        provider,
        Grant::Code,
        &[
            ("client_id", client_id.as_str()),
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("code_verifier", verifier.as_str()),
            ("scope", provider.scopes),
        ],
    )
    .await?;
    if tokens.refresh_token.is_none() {
        return Err("登录服务没有返回长期授权，请重新登录并同意“保持访问权限”。".into());
    }
    let identity = identity(tokens.id_token.as_deref().unwrap_or_default())?;
    Ok(SignedIn { identity, tokens })
}

// ---------------------------------------------------------------- 访问令牌缓存

pub(crate) struct CachedToken {
    /// 生成此访问令牌的刷新令牌；凭据被替换后缓存自动失效。
    refresh_token: String,
    access_token: String,
    expires_at: Instant,
}

impl CachedToken {
    pub(crate) fn new(refresh_token: String, tokens: &Tokens) -> Self {
        Self {
            refresh_token,
            access_token: tokens.access_token.clone(),
            expires_at: tokens.expires_at,
        }
    }

    pub(crate) fn usable_for(&self, refresh_token: &str) -> Option<&str> {
        (self.refresh_token == refresh_token && self.expires_at > Instant::now() + REFRESH_MARGIN)
            .then_some(self.access_token.as_str())
    }
}

#[derive(Default)]
pub struct OAuthState {
    /// 同一时间只刷新一次，避免并发请求互相作废对方的刷新令牌。
    pub(crate) tokens: tokio::sync::Mutex<HashMap<String, CachedToken>>,
    /// 进行中的浏览器登录。
    pub(crate) pending: std::sync::Mutex<Option<PendingSignIn>>,
}

impl OAuthState {
    pub async fn forget(&self, account_id: &str) {
        self.tokens.lock().await.remove(account_id);
    }
}

pub fn is_oauth(account: &Account) -> bool {
    account.auth == AuthMethod::OAuth2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_is_unpadded_base64url_sha256() {
        // 期望值由 Python hashlib 独立计算：base64url(sha256(verifier)) 去掉末尾 "="。
        assert_eq!(
            pkce_challenge("wispmail-pkce-test-verifier-0123456789abcdef"),
            "jndTD6BDqK5yxXMEogdsdCr4JDzi5epSNEujKpnwVW4"
        );
        let verifier = random_token();
        assert_eq!(verifier.len(), 43);
        assert_ne!(verifier, random_token());
    }

    #[test]
    fn authorize_url_requests_mail_scopes_with_pkce() {
        let url = authorize_url(
            &MICROSOFT,
            "client-id",
            "http://localhost:4567",
            "challenge",
            "state&x",
            None,
        );
        let parsed = url::Url::parse(&url).unwrap();
        let params: HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(parsed.host_str(), Some("login.microsoftonline.com"));
        assert_eq!(params["redirect_uri"], "http://localhost:4567");
        assert_eq!(params["state"], "state&x");
        assert_eq!(params["code_challenge_method"], "S256");
        assert_eq!(params["prompt"], "select_account");
        assert!(params["scope"].contains("IMAP.AccessAsUser.All"));
        assert!(params["scope"].contains("offline_access"));
        let hinted = authorize_url(&MICROSOFT, "id", "r", "c", "s", Some("me@outlook.com"));
        assert!(hinted.contains("login_hint=me%40outlook.com"));
        assert!(!hinted.contains("prompt="));
    }

    #[test]
    fn pasted_redirects_must_be_local_and_match_the_state() {
        assert_eq!(
            code_from_pasted_redirect(
                " http://localhost:51234/?code=M.C5_abc&state=s1&session_state=x ",
                "s1"
            ),
            Ok("M.C5_abc".into())
        );
        assert_eq!(
            code_from_pasted_redirect("http://127.0.0.1:1/?code=c&state=s1", "s1"),
            Ok("c".into())
        );
        for bad in [
            "http://localhost:51234/?code=c&state=other",
            "https://evil.test/?code=c&state=s1",
            "code=c&state=s1",
            "",
        ] {
            assert!(code_from_pasted_redirect(bad, "s1").is_err(), "{bad}");
        }
        assert!(
            code_from_pasted_redirect("http://localhost:1/?error=access_denied&state=s1", "s1")
                .unwrap_err()
                .contains("取消")
        );
    }

    #[test]
    fn token_errors_explain_registration_problems_with_the_microsoft_code() {
        let web_platform = token_error_message(
            &MICROSOFT,
            Grant::Code,
            "invalid_client",
            "AADSTS7000218: The request body must contain the following parameter: 'client_assertion' or 'client_secret'. Trace ID: x",
        );
        assert!(
            web_platform.contains("移动和桌面应用程序"),
            "{web_platform}"
        );
        assert!(web_platform.ends_with("（微软错误码 AADSTS7000218）"));
        let expired_code = token_error_message(
            &MICROSOFT,
            Grant::Code,
            "invalid_grant",
            "AADSTS70008: expired",
        );
        assert!(expired_code.starts_with("授权码无效或已过期"));
        let revoked = token_error_message(&MICROSOFT, Grant::Refresh, "invalid_grant", "");
        assert!(revoked.contains("重新登录") && revoked.ends_with("（invalid_grant）"));
        assert_eq!(error_code("x AADSTS50011: y"), Some("AADSTS50011"));
        assert_eq!(error_code("AADSTS"), None);
    }

    #[test]
    fn callback_requires_matching_state() {
        assert_eq!(
            callback_result("/?code=abc&state=s1", "s1"),
            Callback::Code("abc".into())
        );
        assert_eq!(
            callback_result("/?code=abc&state=other", "s1"),
            Callback::Ignore
        );
        assert_eq!(callback_result("/?code=abc", "s1"), Callback::Ignore);
        assert_eq!(callback_result("/favicon.ico", "s1"), Callback::Ignore);
        assert_eq!(callback_result("", "s1"), Callback::Ignore);
        assert!(matches!(
            callback_result("/?error=access_denied&state=s1", "s1"),
            Callback::Denied(message) if message.contains("取消")
        ));
        // 伪造的错误回调不能中断登录。
        assert_eq!(
            callback_result("/?error=access_denied&state=x", "s1"),
            Callback::Ignore
        );
    }

    #[tokio::test]
    async fn loopback_answers_the_browser_and_returns_the_code() {
        let loopback = Loopback::bind().await.unwrap();
        let port = loopback.port;
        assert_eq!(loopback.redirect_uri(), format!("http://localhost:{port}"));
        let browser = tokio::spawn(async move {
            let mut pages = Vec::new();
            for target in [
                "/favicon.ico",
                "/?code=forged&state=wrong",
                "/?code=the-code&state=s1",
            ] {
                let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
                stream
                    .write_all(
                        format!("GET {target} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes(),
                    )
                    .await
                    .unwrap();
                let mut page = String::new();
                stream.read_to_string(&mut page).await.unwrap();
                pages.push(page);
            }
            pages
        });
        let code = timeout(Duration::from_secs(5), loopback.wait_for_code("s1"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(code, "the-code");
        let pages = browser.await.unwrap();
        assert!(pages[0].starts_with("HTTP/1.1 404"));
        assert!(pages[1].starts_with("HTTP/1.1 404"));
        assert!(pages[2].starts_with("HTTP/1.1 200") && pages[2].contains("登录完成"));
    }

    fn id_token(claims: serde_json::Value) -> String {
        format!(
            "header.{}.signature",
            URL_SAFE_NO_PAD.encode(claims.to_string())
        )
    }

    #[test]
    fn identity_reads_email_name_and_account_kind() {
        let personal = identity(&id_token(serde_json::json!({
            "preferred_username": "Me@Outlook.com", "name": "张三",
            "tid": MICROSOFT_CONSUMER_TENANT
        })))
        .unwrap();
        assert_eq!(personal.email, "Me@outlook.com");
        assert_eq!(personal.name, "张三");
        assert!(personal.personal);
        let work = identity(&id_token(serde_json::json!({
            "email": "alice@contoso.com", "preferred_username": "alice_upn",
            "tid": "11111111-2222-3333-4444-555555555555"
        })))
        .unwrap();
        assert_eq!(work.email, "alice@contoso.com");
        assert!(!work.personal);
        assert_eq!(
            MICROSOFT.outgoing(personal.personal).host,
            "smtp-mail.outlook.com"
        );
        assert_eq!(MICROSOFT.outgoing(work.personal).host, "smtp.office365.com");
        assert!(identity(&id_token(serde_json::json!({"name": "x"}))).is_err());
        assert!(identity("not-a-token").is_err());
    }

    #[test]
    fn tokens_only_go_to_the_providers_own_servers() {
        let incoming = MICROSOFT.incoming();
        assert!(MICROSOFT.allows_servers(&incoming, &MICROSOFT.outgoing(true)));
        assert!(MICROSOFT.allows_servers(&incoming, &MICROSOFT.outgoing(false)));
        let mut evil = MICROSOFT.outgoing(true);
        evil.host = "smtp.evil.test".into();
        assert!(!MICROSOFT.allows_servers(&incoming, &evil));
        let mut plain = MICROSOFT.outgoing(true);
        plain.port = 25;
        assert!(!MICROSOFT.allows_servers(&incoming, &plain));
        assert!(provider_for(&incoming).is_some());
        assert!(provider_for(&evil).is_none());
    }

    #[test]
    fn cached_access_token_is_tied_to_refresh_token_and_expiry() {
        let tokens = Tokens {
            access_token: "access".into(),
            refresh_token: None,
            expires_at: Instant::now() + Duration::from_secs(3600),
            id_token: None,
        };
        let cached = CachedToken::new("refresh-1".into(), &tokens);
        assert_eq!(cached.usable_for("refresh-1"), Some("access"));
        assert_eq!(cached.usable_for("refresh-2"), None);
        let expiring = Tokens {
            expires_at: Instant::now() + Duration::from_secs(60),
            ..tokens
        };
        assert_eq!(
            CachedToken::new("refresh-1".into(), &expiring).usable_for("refresh-1"),
            None
        );
    }

    #[test]
    fn client_ids_are_restricted_to_safe_characters() {
        assert!(valid_client_id("2f6c1c9e-0000-4000-8000-123456789abc"));
        assert!(valid_client_id("123-abc.apps.googleusercontent.com"));
        assert!(!valid_client_id(""));
        assert!(!valid_client_id("id&scope=evil"));
    }
}
