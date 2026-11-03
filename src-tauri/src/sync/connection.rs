//! 只验证登录，不选择邮箱、不下载邮件，也不发送邮件。
//!
//! 两种协议分别拥有覆盖 DNS、连接、TLS、登录的总超时。返回固定的错误信息，
//! 避免服务器回复或底层库错误中回显的凭据被传给前端或写入日志。

use std::{error::Error as StdError, fmt::Debug, future::Future, time::Duration};

use async_imap::{
    Client, Session,
    imap_proto::{Response, Status},
};
use lettre::{
    AsyncSmtpTransport, Tokio1Executor,
    transport::smtp::{
        Error as SmtpError,
        authentication::Credentials,
        client::{Tls, TlsParameters},
    },
};
use serde::Serialize;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
    time::timeout,
};
use tokio_native_tls::{TlsConnector, native_tls};

use crate::account::{Account, AccountCredentials, AuthMethod, Encryption, IncomingProtocol};

const CONNECTION_TIMEOUT: Duration = Duration::from_secs(20);

pub(super) type ImapSession = Session<tokio_native_tls::TlsStream<TcpStream>>;

pub(super) async fn open_imap(
    account: &Account,
    credentials: &AccountCredentials,
) -> Result<ImapSession, String> {
    if account.auth != AuthMethod::Password || account.incoming_protocol != IncomingProtocol::Imap {
        return Err("当前版本仅支持 IMAP 密码 / 授权码登录。".into());
    }
    let connector = native_tls::TlsConnector::new()
        .map(TlsConnector::from)
        .map_err(|_| "无法初始化系统 TLS。".to_string())?;
    open_imap_with_connector(account, &credentials.incoming_password, &connector)
        .await
        .map_err(str::to_string)
}

pub(super) fn smtp_transport(
    account: &Account,
    credentials: &AccountCredentials,
) -> Result<AsyncSmtpTransport<Tokio1Executor>, String> {
    if account.auth != AuthMethod::Password {
        return Err("当前版本仅支持 SMTP 密码 / 授权码登录。".into());
    }
    let parameters = TlsParameters::new(account.outgoing.host.clone())
        .map_err(|_| "SMTP 无法初始化系统 TLS。".to_string())?;
    smtp_transport_with_parameters(account, &credentials.outgoing_password, parameters)
        .map_err(str::to_string)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionCheck {
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionReport {
    pub incoming: ConnectionCheck,
    pub outgoing: ConnectionCheck,
}

impl ConnectionCheck {
    fn failure(message: impl Into<String>) -> Self {
        Self {
            success: false,
            message: message.into(),
        }
    }
}

/// 两个连接并行验证，均不会降级为明文认证。只接受密码 / 授权码登录。
pub async fn test_connection(
    account: &Account,
    credentials: &AccountCredentials,
) -> ConnectionReport {
    test_connection_with_timeout(account, credentials, CONNECTION_TIMEOUT).await
}

async fn test_connection_with_timeout(
    account: &Account,
    credentials: &AccountCredentials,
    budget: Duration,
) -> ConnectionReport {
    let unsupported = if account.auth != AuthMethod::Password {
        Some("当前版本尚未支持 OAuth2，请使用支持密码 / 授权码登录的邮箱。")
    } else if account.incoming_protocol != IncomingProtocol::Imap {
        Some("当前版本仅支持 IMAP，POP3 将在后续版本提供。")
    } else {
        None
    };
    if let Some(message) = unsupported {
        return ConnectionReport {
            incoming: ConnectionCheck::failure(message),
            outgoing: ConnectionCheck::failure(message),
        };
    }

    let (incoming, outgoing) = tokio::join!(
        bounded_check(
            "IMAP",
            budget,
            check_imap(account, &credentials.incoming_password)
        ),
        bounded_check(
            "SMTP",
            budget,
            check_smtp(account, &credentials.outgoing_password)
        ),
    );
    ConnectionReport { incoming, outgoing }
}

async fn bounded_check(
    protocol: &str,
    budget: Duration,
    operation: impl Future<Output = Result<(), &'static str>>,
) -> ConnectionCheck {
    match timeout(budget, operation).await {
        Ok(Ok(())) => ConnectionCheck {
            success: true,
            message: format!("{protocol} 加密连接与登录验证成功。"),
        },
        Ok(Err(message)) => ConnectionCheck::failure(message),
        Err(_) => ConnectionCheck::failure(format!(
            "{protocol} 验证超时，请检查网络、服务器地址及端口后重试。"
        )),
    }
}

async fn check_imap(account: &Account, password: &str) -> Result<(), &'static str> {
    validate_imap_credentials(account.incoming_login(), password)?;
    let connector = native_tls::TlsConnector::new()
        .map(TlsConnector::from)
        .map_err(|_| "IMAP 无法初始化系统 TLS，请检查系统证书设置。")?;
    check_imap_with_connector(account, password, &connector).await
}

async fn check_imap_with_connector(
    account: &Account,
    password: &str,
    connector: &TlsConnector,
) -> Result<(), &'static str> {
    let mut session = open_imap_with_connector(account, password, connector).await?;
    // 登录成功不代表能读取邮件：部分服务商（如网易）直到打开文件夹时才拒绝，
    // 只读打开收件箱可以在保存账户前发现这类问题。
    let examined = timeout(Duration::from_secs(10), session.examine("INBOX")).await;
    let _ = timeout(Duration::from_secs(1), session.logout()).await;
    match examined {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(_)) => Err(
            "IMAP 已登录，但服务器不允许读取收件箱。请在网页版邮箱设置中确认已开启 IMAP 服务，并使用授权码登录。",
        ),
        Err(_) => Err("IMAP 已登录，但打开收件箱超时，请稍后重试。"),
    }
}

/// 网易邮箱（163 / 126 / yeah.net / 188 及网易企业邮箱）要求登录后先用 IMAP ID（RFC 2971）
/// 表明客户端身份，否则打开文件夹时返回 "Unsafe Login"。其他服务器不发送，避免多一次往返。
pub(super) fn requires_client_id(host: &str) -> bool {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    ["163.com", "126.com", "yeah.net", "188.com", "netease.com"]
        .iter()
        .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
}

/// 发送客户端标识。服务器拒绝或不支持时忽略：之后打开文件夹的结果才是准确的判断依据。
pub(super) async fn identify_client<T>(session: &mut Session<T>)
where
    T: AsyncRead + AsyncWrite + Unpin + Debug + Send,
{
    let _ = timeout(
        Duration::from_secs(10),
        session.id([
            ("name", Some("WispMail")),
            ("version", Some(env!("CARGO_PKG_VERSION"))),
            ("vendor", Some("WispMail")),
        ]),
    )
    .await;
}

pub(super) async fn open_imap_with_connector(
    account: &Account,
    password: &str,
    connector: &TlsConnector,
) -> Result<ImapSession, &'static str> {
    validate_imap_credentials(account.incoming_login(), password)?;
    let server = &account.incoming;
    if server.encryption == Encryption::None {
        return Err("IMAP 不允许明文登录，请选择 TLS 或 STARTTLS。");
    }
    let tcp = TcpStream::connect((server.host.as_str(), server.port))
        .await
        .map_err(|_| "IMAP 连接失败，请检查网络、服务器地址和端口。")?;
    let stream = if server.encryption == Encryption::StartTls {
        let mut client = Client::new(tcp);
        read_greeting(&mut client).await?;
        client
            .run_command_and_check_ok("STARTTLS", None)
            .await
            .map_err(|_| "IMAP 服务器未接受 STARTTLS，请核对加密方式和端口。")?;
        connector.connect(&server.host, client.into_inner()).await
    } else {
        connector.connect(&server.host, tcp).await
    }
    .map_err(|_| "IMAP TLS 握手失败，请检查加密端口、证书有效期及证书域名。")?;

    let mut client = Client::new(stream);
    if server.encryption == Encryption::Tls {
        read_greeting(&mut client).await?;
    }
    let mut session = authenticate_imap_session(client, account.incoming_login(), password).await?;
    if requires_client_id(&server.host) {
        identify_client(&mut session).await;
    }
    Ok(session)
}

async fn read_greeting<T>(client: &mut Client<T>) -> Result<(), &'static str>
where
    T: AsyncRead + AsyncWrite + Unpin + Debug + Send,
{
    let response = client
        .read_response()
        .await
        .map_err(|_| "IMAP 服务器响应异常，请确认服务器和端口属于 IMAP 服务。")?
        .ok_or("IMAP 服务器在登录前关闭了连接。")?;
    match response.parsed() {
        Response::Data {
            status: Status::Ok, ..
        } => Ok(()),
        _ => Err("IMAP 服务器未接受登录连接，请检查服务是否启用。"),
    }
}

#[cfg(test)]
async fn authenticate_imap<T>(
    client: Client<T>,
    username: &str,
    password: &str,
) -> Result<(), &'static str>
where
    T: AsyncRead + AsyncWrite + Unpin + Debug + Send,
{
    let mut session = authenticate_imap_session(client, username, password).await?;
    let _ = timeout(Duration::from_secs(1), session.logout()).await;
    Ok(())
}

async fn authenticate_imap_session<T>(
    client: Client<T>,
    username: &str,
    password: &str,
) -> Result<Session<T>, &'static str>
where
    T: AsyncRead + AsyncWrite + Unpin + Debug + Send,
{
    client
        .login(username, password)
        .await
        .map_err(|(error, _)| {
            use async_imap::error::Error;
            match error {
                Error::No(_) | Error::Bad(_) => {
                    "IMAP 登录被拒绝，请检查收信用户名、密码 / 授权码，并确认已开启 IMAP 服务。"
                }
                Error::Validate(_) => "IMAP 登录信息格式无效，请检查收信用户名和密码 / 授权码。",
                _ => "IMAP 登录过程中连接中断或响应异常，请稍后重试。",
            }
        })
}

async fn check_smtp(account: &Account, password: &str) -> Result<(), &'static str> {
    validate_smtp_credentials(account.outgoing_login(), password)?;
    if account.outgoing.encryption == Encryption::None {
        return Err("SMTP 不允许明文登录，请选择 TLS 或 STARTTLS。");
    }
    let parameters = TlsParameters::new(account.outgoing.host.clone())
        .map_err(|_| "SMTP 无法初始化系统 TLS，请检查服务器地址及系统证书设置。")?;
    check_smtp_with_parameters(account, password, parameters).await
}

async fn check_smtp_with_parameters(
    account: &Account,
    password: &str,
    parameters: TlsParameters,
) -> Result<(), &'static str> {
    let transport = smtp_transport_with_parameters(account, password, parameters)?;
    match transport.test_connection().await {
        Ok(true) => Ok(()),
        Ok(false) => Err("SMTP 登录后服务器无响应，请稍后重试。"),
        Err(error) => Err(smtp_error_message(&error)),
    }
}

pub(super) fn smtp_transport_with_parameters(
    account: &Account,
    password: &str,
    parameters: TlsParameters,
) -> Result<AsyncSmtpTransport<Tokio1Executor>, &'static str> {
    validate_smtp_credentials(account.outgoing_login(), password)?;
    let server = &account.outgoing;
    let tls = match server.encryption {
        Encryption::Tls => Tls::Wrapper(parameters),
        Encryption::StartTls => Tls::Required(parameters),
        Encryption::None => return Err("SMTP 不允许明文登录，请选择 TLS 或 STARTTLS。"),
    };
    // builder_dangerous 仅表示手动指定服务器；上方始终要求 TLS，并使用默认严格证书验证。
    let transport = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&server.host)
        .port(server.port)
        .tls(tls)
        .credentials(Credentials::new(
            account.outgoing_login().to_owned(),
            password.to_owned(),
        ))
        .timeout(Some(CONNECTION_TIMEOUT))
        .build::<Tokio1Executor>();
    Ok(transport)
}

fn valid_login_value(value: &str) -> bool {
    !value.is_empty() && !value.chars().any(char::is_control)
}

fn validate_imap_credentials(username: &str, password: &str) -> Result<(), &'static str> {
    if !valid_login_value(username) || !valid_login_value(password) {
        return Err("请输入有效的 IMAP 收信用户名和密码 / 授权码，不能包含控制字符。");
    }
    Ok(())
}

fn validate_smtp_credentials(username: &str, password: &str) -> Result<(), &'static str> {
    if !valid_login_value(username) || !valid_login_value(password) {
        return Err("请输入有效的 SMTP 发信用户名和密码 / 授权码，不能包含控制字符。");
    }
    Ok(())
}

fn smtp_error_message(error: &SmtpError) -> &'static str {
    if error.is_timeout() {
        "SMTP 验证超时，请检查网络、服务器地址及端口后重试。"
    } else if error.is_tls() || has_native_tls_cause(error) {
        "SMTP TLS 连接失败，请检查服务器是否支持所选加密方式、端口和证书。"
    } else if let Some(status) = error.status() {
        match status.to_string().as_str() {
            "530" | "534" | "535" | "538" => {
                "SMTP 登录被拒绝，请检查发信用户名、密码 / 授权码，并确认已开启 SMTP 服务。"
            }
            _ if error.is_transient() => "SMTP 服务器暂时拒绝连接或登录，请稍后重试。",
            _ => "SMTP 服务器拒绝验证，请检查账户权限及 SMTP 服务设置。",
        }
    } else if error.is_client() {
        "SMTP 认证或加密协商失败，请确认服务器支持密码登录及所选加密方式。"
    } else {
        "SMTP 连接失败或响应异常，请检查网络、服务器地址和端口。"
    }
}

// Lettre 将部分握手错误包装为 Connection；按错误类型分类，不读取服务器错误文本。
fn has_native_tls_cause(error: &SmtpError) -> bool {
    let mut source = error.source();
    while let Some(cause) = source {
        if cause.is::<native_tls::Error>() {
            return true;
        }
        source = cause.source();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::ServerConfig;
    use tokio::{
        io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
        net::TcpListener,
    };

    fn account() -> Account {
        Account {
            id: "test".into(),
            email: "user@example.com".into(),
            display_name: "Test".into(),
            incoming_username: String::new(),
            outgoing_username: String::new(),
            use_same_password: true,
            incoming_protocol: IncomingProtocol::Imap,
            auth: AuthMethod::Password,
            incoming: ServerConfig {
                host: "127.0.0.1".into(),
                port: 993,
                encryption: Encryption::Tls,
            },
            outgoing: ServerConfig {
                host: "127.0.0.1".into(),
                port: 465,
                encryption: Encryption::Tls,
            },
        }
    }

    fn credentials() -> AccountCredentials {
        AccountCredentials {
            incoming_password: "secret".into(),
            outgoing_password: "secret".into(),
        }
    }

    #[tokio::test]
    async fn rejects_unsupported_settings_without_connecting() {
        let mut account = account();
        account.auth = AuthMethod::OAuth2;
        let result = test_connection(&account, &credentials()).await;
        assert!(!result.incoming.success && !result.outgoing.success);
        assert!(result.incoming.message.contains("OAuth2"));
        account.auth = AuthMethod::Password;
        account.incoming_protocol = IncomingProtocol::Pop3;
        assert!(
            test_connection(&account, &credentials())
                .await
                .incoming
                .message
                .contains("POP3")
        );
        account.incoming_protocol = IncomingProtocol::Imap;
        account.incoming.encryption = Encryption::None;
        account.outgoing.encryption = Encryption::None;
        let result = test_connection(&account, &credentials()).await;
        assert!(result.incoming.message.contains("明文"));
        assert!(result.outgoing.message.contains("明文"));
    }

    #[tokio::test]
    async fn imap_starttls_rejection_never_sends_credentials() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut account = account();
        account.incoming.port = listener.local_addr().unwrap().port();
        account.incoming.encryption = Encryption::StartTls;
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            stream.write_all(b"* OK test ready\r\n").await.unwrap();
            let mut reader = BufReader::new(stream);
            let mut command = String::new();
            reader.read_line(&mut command).await.unwrap();
            assert!(command.ends_with(" STARTTLS\r\n"));
            let tag = command.split_whitespace().next().unwrap();
            reader
                .get_mut()
                .write_all(format!("{tag} NO encryption unavailable\r\n").as_bytes())
                .await
                .unwrap();
            let mut rest = String::new();
            reader.read_to_string(&mut rest).await.unwrap();
            assert!(
                rest.is_empty(),
                "must close connection instead of sending LOGIN"
            );
        });
        let result = bounded_check(
            "IMAP",
            Duration::from_secs(2),
            check_imap(&account, "secret"),
        )
        .await;
        assert!(!result.success);
        assert!(result.message.contains("STARTTLS"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn smtp_requires_starttls_before_authentication() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut account = account();
        account.outgoing.port = listener.local_addr().unwrap().port();
        account.outgoing.encryption = Encryption::StartTls;
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            stream
                .write_all(b"220 localhost ESMTP ready\r\n")
                .await
                .unwrap();
            let mut reader = BufReader::new(stream);
            let mut command = String::new();
            reader.read_line(&mut command).await.unwrap();
            assert!(command.starts_with("EHLO "));
            // Advertise plaintext auth but deliberately omit STARTTLS.
            reader
                .get_mut()
                .write_all(b"250-localhost\r\n250 AUTH PLAIN LOGIN\r\n")
                .await
                .unwrap();
            let mut rest = String::new();
            reader.read_to_string(&mut rest).await.unwrap();
            assert!(
                !rest.contains("AUTH"),
                "must never fall back to plaintext AUTH"
            );
        });
        let result = bounded_check(
            "SMTP",
            Duration::from_secs(2),
            check_smtp(&account, "secret"),
        )
        .await;
        assert!(!result.success);
        assert!(result.message.contains("TLS") || result.message.contains("加密"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn imap_authentication_errors_never_echo_server_text() {
        let (client_stream, mut server_stream) = tokio::io::duplex(4096);
        let server = tokio::spawn(async move {
            let mut reader = BufReader::new(&mut server_stream);
            let mut command = String::new();
            reader.read_line(&mut command).await.unwrap();
            assert!(command.contains("LOGIN \"user@example.com\" \"secret\""));
            let tag = command.split_whitespace().next().unwrap();
            reader
                .get_mut()
                .write_all(format!("{tag} NO password secret was rejected\r\n").as_bytes())
                .await
                .unwrap();
        });
        let error = authenticate_imap(Client::new(client_stream), "user@example.com", "secret")
            .await
            .unwrap_err();
        assert!(error.contains("登录被拒绝"));
        assert!(!error.contains("secret"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn both_protocols_timeout_on_unresponsive_servers() {
        let imap = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let smtp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut account = account();
        account.incoming.port = imap.local_addr().unwrap().port();
        account.outgoing.port = smtp.local_addr().unwrap().port();
        let result =
            test_connection_with_timeout(&account, &credentials(), Duration::from_millis(30)).await;
        assert!(!result.incoming.success && !result.outgoing.success);
        assert!(result.incoming.message.contains("超时"));
        assert!(result.outgoing.message.contains("超时"));
    }

    fn fixture_acceptor() -> tokio_native_tls::TlsAcceptor {
        let identity = native_tls::Identity::from_pkcs8(
            include_bytes!("test-fixtures/server.pem"),
            include_bytes!("test-fixtures/server-key.pem"),
        )
        .unwrap();
        native_tls::TlsAcceptor::new(identity).unwrap().into()
    }

    fn fixture_imap_connector() -> TlsConnector {
        let certificate =
            native_tls::Certificate::from_pem(include_bytes!("test-fixtures/root.pem")).unwrap();
        native_tls::TlsConnector::builder()
            .add_root_certificate(certificate)
            .build()
            .unwrap()
            .into()
    }

    fn fixture_smtp_parameters() -> TlsParameters {
        let certificate = lettre::transport::smtp::client::Certificate::from_pem(include_bytes!(
            "test-fixtures/root.pem"
        ))
        .unwrap();
        TlsParameters::builder("127.0.0.1".into())
            .add_root_certificate(certificate)
            .build_native()
            .unwrap()
    }

    async fn fixture_line<T: AsyncRead + AsyncWrite + Unpin>(reader: &mut BufReader<T>) -> String {
        let mut line = String::new();
        assert!(reader.read_line(&mut line).await.unwrap() > 0);
        line
    }

    async fn fixture_imap_server(
        listener: TcpListener,
        encryption: Encryption,
        expected_login: &'static str,
    ) {
        let (mut stream, _) = listener.accept().await.unwrap();
        if encryption == Encryption::StartTls {
            stream.write_all(b"* OK fixture ready\r\n").await.unwrap();
            let mut reader = BufReader::new(stream);
            let command = fixture_line(&mut reader).await;
            assert!(command.ends_with(" STARTTLS\r\n"));
            let tag = command.split_whitespace().next().unwrap();
            reader
                .get_mut()
                .write_all(format!("{tag} OK Begin TLS\r\n").as_bytes())
                .await
                .unwrap();
            stream = reader.into_inner();
        }
        let mut stream = fixture_acceptor().accept(stream).await.unwrap();
        if encryption == Encryption::Tls {
            stream.write_all(b"* OK fixture ready\r\n").await.unwrap();
        }
        let mut reader = BufReader::new(stream);
        let login = fixture_line(&mut reader).await;
        assert!(login.ends_with(expected_login));
        let tag = login.split_whitespace().next().unwrap();
        reader
            .get_mut()
            .write_all(format!("{tag} OK Authenticated\r\n").as_bytes())
            .await
            .unwrap();
        // 连接测试会只读打开收件箱，确认登录后确实可以读取邮件。
        let examine = fixture_line(&mut reader).await;
        assert!(examine.ends_with(" EXAMINE \"INBOX\"\r\n"), "{examine}");
        let tag = examine.split_whitespace().next().unwrap();
        reader
            .get_mut()
            .write_all(
                format!(
                    "* 0 EXISTS\r\n* OK [UIDVALIDITY 1] valid\r\n{tag} OK [READ-ONLY] done\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let logout = fixture_line(&mut reader).await;
        assert!(logout.ends_with(" LOGOUT\r\n"));
        let tag = logout.split_whitespace().next().unwrap();
        reader
            .get_mut()
            .write_all(format!("* BYE closing\r\n{tag} OK Logout\r\n").as_bytes())
            .await
            .unwrap();
    }

    async fn fixture_smtp_server(
        listener: TcpListener,
        encryption: Encryption,
        accept_login: bool,
        expected_auth: &'static str,
    ) {
        let (mut stream, _) = listener.accept().await.unwrap();
        if encryption == Encryption::StartTls {
            stream
                .write_all(b"220 localhost ESMTP fixture\r\n")
                .await
                .unwrap();
            let mut reader = BufReader::new(stream);
            assert!(fixture_line(&mut reader).await.starts_with("EHLO "));
            reader
                .get_mut()
                .write_all(b"250-localhost\r\n250-STARTTLS\r\n250 AUTH PLAIN\r\n")
                .await
                .unwrap();
            assert_eq!(fixture_line(&mut reader).await, "STARTTLS\r\n");
            reader
                .get_mut()
                .write_all(b"220 Begin TLS\r\n")
                .await
                .unwrap();
            stream = reader.into_inner();
        }
        let mut stream = fixture_acceptor().accept(stream).await.unwrap();
        if encryption == Encryption::Tls {
            stream
                .write_all(b"220 localhost ESMTP fixture\r\n")
                .await
                .unwrap();
        }
        let mut reader = BufReader::new(stream);
        assert!(fixture_line(&mut reader).await.starts_with("EHLO "));
        reader
            .get_mut()
            .write_all(b"250-localhost\r\n250 AUTH PLAIN\r\n")
            .await
            .unwrap();
        // Authentication must happen before NOOP; compare the exact wire value
        // so using the mailbox address or the other server's password fails.
        assert_eq!(fixture_line(&mut reader).await, expected_auth);
        if !accept_login {
            reader
                .get_mut()
                .write_all(b"535 password secret was rejected\r\n")
                .await
                .unwrap();
            return;
        }
        reader
            .get_mut()
            .write_all(b"235 Authenticated\r\n")
            .await
            .unwrap();
        assert_eq!(fixture_line(&mut reader).await, "NOOP\r\n");
        reader.get_mut().write_all(b"250 OK\r\n").await.unwrap();
        assert_eq!(fixture_line(&mut reader).await, "QUIT\r\n");
        reader
            .get_mut()
            .write_all(b"221 Goodbye\r\n")
            .await
            .unwrap();
    }

    async fn successful_encrypted_login(encryption: Encryption, distinct_logins: bool) {
        let imap = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let smtp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut account = account();
        account.incoming.port = imap.local_addr().unwrap().port();
        account.incoming.encryption = encryption;
        account.outgoing.port = smtp.local_addr().unwrap().port();
        account.outgoing.encryption = encryption;
        let mut credentials = credentials();
        let (expected_imap, expected_smtp) = if distinct_logins {
            account.incoming_username = r"DOMAIN\alice".into();
            account.outgoing_username = "smtp-user".into();
            account.use_same_password = false;
            credentials.incoming_password = "incoming-secret".into();
            credentials.outgoing_password = "outgoing-secret".into();
            (
                " LOGIN \"DOMAIN\\\\alice\" \"incoming-secret\"\r\n",
                "AUTH PLAIN AHNtdHAtdXNlcgBvdXRnb2luZy1zZWNyZXQ=\r\n",
            )
        } else {
            (
                " LOGIN \"user@example.com\" \"secret\"\r\n",
                "AUTH PLAIN AHVzZXJAZXhhbXBsZS5jb20Ac2VjcmV0\r\n",
            )
        };
        let imap_server = tokio::spawn(fixture_imap_server(imap, encryption, expected_imap));
        let smtp_server = tokio::spawn(fixture_smtp_server(smtp, encryption, true, expected_smtp));
        let connector = fixture_imap_connector();
        let (incoming, outgoing) = tokio::join!(
            bounded_check(
                "IMAP",
                Duration::from_secs(5),
                check_imap_with_connector(&account, &credentials.incoming_password, &connector)
            ),
            bounded_check(
                "SMTP",
                Duration::from_secs(5),
                check_smtp_with_parameters(
                    &account,
                    &credentials.outgoing_password,
                    fixture_smtp_parameters()
                )
            ),
        );
        assert!(incoming.success, "{}", incoming.message);
        assert!(outgoing.success, "{}", outgoing.message);
        imap_server.await.unwrap();
        smtp_server.await.unwrap();
    }

    #[tokio::test]
    async fn tls_verifies_both_logins_without_fetching_or_sending_mail() {
        successful_encrypted_login(Encryption::Tls, false).await;
    }

    #[tokio::test]
    async fn starttls_verifies_both_logins_after_upgrade() {
        successful_encrypted_login(Encryption::StartTls, false).await;
    }

    #[tokio::test]
    async fn tls_uses_distinct_non_email_usernames_and_passwords() {
        successful_encrypted_login(Encryption::Tls, true).await;
    }

    #[tokio::test]
    async fn starttls_uses_distinct_non_email_usernames_and_passwords() {
        successful_encrypted_login(Encryption::StartTls, true).await;
    }

    #[tokio::test]
    async fn invalid_credentials_are_reported_for_only_the_affected_protocol() {
        let mut account = account();
        account.incoming.encryption = Encryption::None;
        account.outgoing.encryption = Encryption::None;
        let mut credentials = credentials();
        credentials.incoming_password.clear();
        let report = test_connection(&account, &credentials).await;
        assert!(report.incoming.message.contains("收信用户名和密码"));
        assert!(report.outgoing.message.contains("明文"));
        credentials.incoming_password = "valid-incoming".into();
        credentials.outgoing_password = "invalid\r\npassword".into();
        let report = test_connection(&account, &credentials).await;
        assert!(report.incoming.message.contains("明文"));
        assert!(report.outgoing.message.contains("发信用户名和密码"));
        assert!(!report.outgoing.message.contains("invalid"));
    }

    #[tokio::test]
    async fn smtp_authentication_errors_never_echo_server_text() {
        let smtp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut account = account();
        account.outgoing.port = smtp.local_addr().unwrap().port();
        let server = tokio::spawn(fixture_smtp_server(
            smtp,
            Encryption::Tls,
            false,
            "AUTH PLAIN AHVzZXJAZXhhbXBsZS5jb20Ac2VjcmV0\r\n",
        ));
        let error = check_smtp_with_parameters(&account, "secret", fixture_smtp_parameters())
            .await
            .unwrap_err();
        assert!(error.contains("登录被拒绝"));
        assert!(!error.contains("secret"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn production_connectors_reject_untrusted_certificates() {
        let imap = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let smtp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut account = account();
        account.incoming.port = imap.local_addr().unwrap().port();
        account.outgoing.port = smtp.local_addr().unwrap().port();
        let imap_server = tokio::spawn(async move {
            let (stream, _) = imap.accept().await.unwrap();
            if let Ok(mut tls) = fixture_acceptor().accept(stream).await {
                let mut byte = [0];
                assert!(!matches!(tls.read(&mut byte).await, Ok(1..)));
            }
        });
        let smtp_server = tokio::spawn(async move {
            let (stream, _) = smtp.accept().await.unwrap();
            if let Ok(mut tls) = fixture_acceptor().accept(stream).await {
                let mut byte = [0];
                assert!(!matches!(tls.read(&mut byte).await, Ok(1..)));
            }
        });
        let report = test_connection(&account, &credentials()).await;
        assert!(!report.incoming.success && report.incoming.message.contains("TLS"));
        assert!(!report.outgoing.success && report.outgoing.message.contains("TLS"));
        imap_server.await.unwrap();
        smtp_server.await.unwrap();
    }
}
