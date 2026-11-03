use std::net::IpAddr;

use super::{AccountInput, AuthMethod, Encryption, IncomingProtocol, ServerConfig};

fn valid_dns_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
}

/// 接受普通 dot-atom 邮箱地址；拒绝空白、换行、多重 @ 和无效域名。
pub(super) fn email_domain(email: &str) -> Option<String> {
    let email = email.trim();
    if email.len() > 254 {
        return None;
    }
    let (local, domain) = email.split_once('@')?;
    if local.is_empty()
        || local.len() > 64
        || local.starts_with('.')
        || local.ends_with('.')
        || local.contains("..")
        || !local
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b".!#$%&'*+-/=?^_`{|}~".contains(&c))
        || !domain.contains('.')
        || !valid_dns_name(domain)
    {
        return None;
    }
    Some(domain.to_ascii_lowercase())
}

/// 规范化邮箱地址（域名转小写）；地址无效时返回 None。
pub(crate) fn normalized_email(email: &str) -> Option<String> {
    let domain = email_domain(email)?;
    let local = email.trim().split_once('@')?.0;
    Some(format!("{local}@{domain}"))
}

fn validate_server(server: &mut ServerConfig, label: &str) -> Result<(), String> {
    server.host = server.host.trim().to_ascii_lowercase();
    if server.host.parse::<IpAddr>().is_err() && !valid_dns_name(&server.host) {
        return Err(format!("{label}服务器地址无效，请只填写主机名或 IP 地址。"));
    }
    if server.port == 0 {
        return Err(format!("{label}端口必须在 1–65535 之间。"));
    }
    if server.encryption == Encryption::None {
        return Err(format!(
            "{label}连接必须使用 TLS 或 STARTTLS，以保护登录凭据。"
        ));
    }
    Ok(())
}

fn validate_username(username: &mut String, email: &str, label: &str) -> Result<(), String> {
    if username.chars().any(char::is_control) || username.chars().count() > 512 {
        return Err(format!(
            "{label}登录名不能超过 512 个字符，也不能包含控制字符。"
        ));
    }
    *username = username.trim().to_string();
    if username.is_empty() {
        *username = email.to_string();
    }
    Ok(())
}

fn validate_password(password: Option<&str>, label: &str) -> Result<(), String> {
    if let Some(password) = password
        && (password.len() > 4096 || password.chars().any(char::is_control))
    {
        return Err(format!(
            "{label}密码或授权码格式无效，不能包含控制字符或超过 4096 字节。"
        ));
    }
    Ok(())
}

pub(super) fn validate_input(input: &mut AccountInput) -> Result<(), String> {
    input.email = input.email.trim().to_string();
    let domain = email_domain(&input.email)
        .ok_or_else(|| "请输入有效的邮箱地址（如 name@example.com）。".to_string())?;
    let local = input.email.split_once('@').unwrap().0;
    input.email = format!("{local}@{domain}");
    validate_username(&mut input.incoming_username, &input.email, "IMAP")?;
    validate_username(&mut input.outgoing_username, &input.email, "SMTP")?;
    input.display_name = input.display_name.trim().to_string();
    if input.display_name.chars().count() > 100 || input.display_name.chars().any(char::is_control)
    {
        return Err("显示名称不能超过 100 个字符，也不能包含控制字符。".into());
    }
    if input.incoming_protocol != IncomingProtocol::Imap {
        return Err("当前版本仅支持 IMAP 收信，POP3 将在后续版本提供。".into());
    }
    validate_server(&mut input.incoming, "IMAP")?;
    validate_server(&mut input.outgoing, "SMTP")?;
    if input.auth == AuthMethod::OAuth2 {
        // OAuth 账户的服务器与登录名固定；令牌由浏览器登录获得，从不经过界面。
        if !crate::oauth::provider_for(&input.incoming)
            .is_some_and(|provider| provider.allows_servers(&input.incoming, &input.outgoing))
        {
            return Err("OAuth 登录的账户只能使用服务商自己的邮件服务器。".into());
        }
        if input
            .password
            .as_deref()
            .is_some_and(|value| !value.is_empty())
            || input
                .outgoing_password
                .as_deref()
                .is_some_and(|value| !value.is_empty())
        {
            return Err("OAuth 登录的账户不使用密码，请通过浏览器重新登录。".into());
        }
        input.incoming_username = input.email.clone();
        input.outgoing_username = input.email.clone();
        input.use_same_password = true;
        return Ok(());
    }
    validate_password(input.password.as_deref(), "IMAP")?;
    if !input.use_same_password {
        validate_password(input.outgoing_password.as_deref(), "SMTP")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_ambiguous_and_injected_addresses() {
        for email in [
            "a@b@example.com",
            "a b@example.com",
            "a@ example.com",
            "a\r\n@example.com",
            "a@example..com",
            "a@-example.com",
            ".a@example.com",
            "a..b@example.com",
            "a@example.com/",
            "a@localhost",
            "a@例子.公司",
        ] {
            assert!(email_domain(email).is_none(), "accepted {email:?}");
        }
        assert_eq!(
            email_domain(" Alice+news@EXAMPLE.com ").as_deref(),
            Some("example.com")
        );
    }

    #[test]
    fn custom_usernames_preserve_case_and_domain_separator() {
        for (raw, expected) in [
            (" CORP\\Alice ", "CORP\\Alice"),
            ("alice", "alice"),
            ("  ", "a@example.com"),
        ] {
            let mut username = raw.to_string();
            validate_username(&mut username, "a@example.com", "IMAP").unwrap();
            assert_eq!(username, expected);
        }
        for raw in ["user\r\nLOGIN", "\tuser", "user\0name"] {
            assert!(validate_username(&mut raw.into(), "a@example.com", "SMTP").is_err());
        }
        assert!(validate_username(&mut "a".repeat(513), "a@example.com", "SMTP").is_err());
    }
}
