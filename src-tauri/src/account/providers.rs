//! 常见邮件服务商的内置配置表与自动识别。
//!
//! 用户输入邮箱地址后，先按域名查内置表；查不到时按常见命名规则
//! （`imap.域名` / `smtp.域名`）给出猜测，由界面提示用户确认。
//!
//! TODO: 在内置表与猜测之间，接入 Thunderbird ISPDB、域名 autoconfig
//! 以及 MX 记录查询，以覆盖更多企业邮箱。

use serde::Serialize;

use super::{AuthMethod, Encryption, IncomingProtocol, ServerConfig};

/// 自动识别得到的配置来源。
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ConfigSource {
    /// 来自内置的服务商表，可信度高。
    Builtin,
    /// 按常见命名规则猜测，需要用户确认。
    Guess,
}

/// 根据邮箱地址识别出的服务器配置建议。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredConfig {
    /// 服务商名称，例如 "QQ 邮箱"；猜测时为 None。
    pub provider: Option<String>,
    pub source: ConfigSource,
    pub incoming_protocol: IncomingProtocol,
    pub incoming: ServerConfig,
    pub outgoing: ServerConfig,
    /// 推荐的登录方式。
    pub auth: AuthMethod,
    /// 给用户的提示，例如需要先在网页版开启 IMAP 并生成授权码。
    pub note: Option<String>,
}

/// 内置服务商条目。
struct Provider {
    name: &'static str,
    domains: &'static [&'static str],
    imap: (&'static str, u16, Encryption),
    smtp: (&'static str, u16, Encryption),
    auth: AuthMethod,
    note: Option<&'static str>,
}

const AUTH_CODE_NOTE: &str =
    "需先在网页版邮箱设置中开启 IMAP/SMTP 服务，并使用生成的授权码代替登录密码。";
const APP_PASSWORD_NOTE: &str = "需在账户安全设置中生成“应用专用密码”，用它代替登录密码。";

const PROVIDERS: &[Provider] = &[
    Provider {
        name: "Gmail",
        domains: &["gmail.com", "googlemail.com"],
        imap: ("imap.gmail.com", 993, Encryption::Tls),
        smtp: ("smtp.gmail.com", 465, Encryption::Tls),
        auth: AuthMethod::OAuth2,
        note: None,
    },
    Provider {
        name: "Outlook",
        domains: &["outlook.com", "hotmail.com", "live.com", "msn.com"],
        imap: ("outlook.office365.com", 993, Encryption::Tls),
        smtp: ("smtp-mail.outlook.com", 587, Encryption::StartTls),
        auth: AuthMethod::OAuth2,
        note: None,
    },
    Provider {
        name: "iCloud",
        domains: &["icloud.com", "me.com", "mac.com"],
        imap: ("imap.mail.me.com", 993, Encryption::Tls),
        smtp: ("smtp.mail.me.com", 587, Encryption::StartTls),
        auth: AuthMethod::Password,
        note: Some(APP_PASSWORD_NOTE),
    },
    Provider {
        name: "Yahoo",
        domains: &["yahoo.com"],
        imap: ("imap.mail.yahoo.com", 993, Encryption::Tls),
        smtp: ("smtp.mail.yahoo.com", 465, Encryption::Tls),
        auth: AuthMethod::Password,
        note: Some(APP_PASSWORD_NOTE),
    },
    Provider {
        name: "QQ 邮箱",
        domains: &["qq.com", "foxmail.com"],
        imap: ("imap.qq.com", 993, Encryption::Tls),
        smtp: ("smtp.qq.com", 465, Encryption::Tls),
        auth: AuthMethod::Password,
        note: Some(AUTH_CODE_NOTE),
    },
    Provider {
        name: "网易 163 邮箱",
        domains: &["163.com"],
        imap: ("imap.163.com", 993, Encryption::Tls),
        smtp: ("smtp.163.com", 465, Encryption::Tls),
        auth: AuthMethod::Password,
        note: Some(AUTH_CODE_NOTE),
    },
    Provider {
        name: "网易 126 邮箱",
        domains: &["126.com"],
        imap: ("imap.126.com", 993, Encryption::Tls),
        smtp: ("smtp.126.com", 465, Encryption::Tls),
        auth: AuthMethod::Password,
        note: Some(AUTH_CODE_NOTE),
    },
    Provider {
        name: "网易 yeah.net 邮箱",
        domains: &["yeah.net"],
        imap: ("imap.yeah.net", 993, Encryption::Tls),
        smtp: ("smtp.yeah.net", 465, Encryption::Tls),
        auth: AuthMethod::Password,
        note: Some(AUTH_CODE_NOTE),
    },
    Provider {
        name: "阿里邮箱",
        domains: &["aliyun.com"],
        imap: ("imap.aliyun.com", 993, Encryption::Tls),
        smtp: ("smtp.aliyun.com", 465, Encryption::Tls),
        auth: AuthMethod::Password,
        note: None,
    },
    Provider {
        name: "新浪邮箱",
        domains: &["sina.com", "sina.cn"],
        imap: ("imap.sina.com", 993, Encryption::Tls),
        smtp: ("smtp.sina.com", 465, Encryption::Tls),
        auth: AuthMethod::Password,
        note: Some(AUTH_CODE_NOTE),
    },
    Provider {
        name: "139 邮箱",
        domains: &["139.com"],
        imap: ("imap.139.com", 993, Encryption::Tls),
        smtp: ("smtp.139.com", 465, Encryption::Tls),
        auth: AuthMethod::Password,
        note: Some(AUTH_CODE_NOTE),
    },
];

fn server((host, port, encryption): (&str, u16, Encryption)) -> ServerConfig {
    ServerConfig {
        host: host.to_string(),
        port,
        encryption,
    }
}

/// 从邮箱地址中取出小写域名；地址不合法时返回 None。
fn domain_of(email: &str) -> Option<String> {
    let (local, domain) = email.trim().rsplit_once('@')?;
    let domain = domain.trim().to_ascii_lowercase();
    if local.is_empty() || domain.is_empty() || !domain.contains('.') {
        return None;
    }
    Some(domain)
}

/// 根据邮箱地址推断服务器配置。地址不合法时返回 None。
pub fn autodiscover(email: &str) -> Option<DiscoveredConfig> {
    let domain = domain_of(email)?;

    if let Some(p) = PROVIDERS
        .iter()
        .find(|p| p.domains.contains(&domain.as_str()))
    {
        return Some(DiscoveredConfig {
            provider: Some(p.name.to_string()),
            source: ConfigSource::Builtin,
            incoming_protocol: IncomingProtocol::Imap,
            incoming: server(p.imap),
            outgoing: server(p.smtp),
            auth: p.auth,
            note: p.note.map(str::to_string),
        });
    }

    // 兜底猜测：大多数自建 / 企业邮箱遵循 imap.域名 / smtp.域名 的命名。
    Some(DiscoveredConfig {
        provider: None,
        source: ConfigSource::Guess,
        incoming_protocol: IncomingProtocol::Imap,
        incoming: server((&format!("imap.{domain}"), 993, Encryption::Tls)),
        outgoing: server((&format!("smtp.{domain}"), 465, Encryption::Tls)),
        auth: AuthMethod::Password,
        note: Some("未能识别该邮箱服务商，以下为推测配置，请核对服务器地址与端口。".to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_builtin_provider_case_insensitively() {
        let cfg = autodiscover("Someone@QQ.com").unwrap();
        assert_eq!(cfg.source, ConfigSource::Builtin);
        assert_eq!(cfg.provider.as_deref(), Some("QQ 邮箱"));
        assert_eq!(cfg.incoming.host, "imap.qq.com");
        assert_eq!(cfg.outgoing.port, 465);
    }

    #[test]
    fn gmail_prefers_oauth() {
        let cfg = autodiscover("me@gmail.com").unwrap();
        assert_eq!(cfg.auth, AuthMethod::OAuth2);
    }

    #[test]
    fn unknown_domain_falls_back_to_guess() {
        let cfg = autodiscover("me@corp.example").unwrap();
        assert_eq!(cfg.source, ConfigSource::Guess);
        assert_eq!(cfg.incoming.host, "imap.corp.example");
        assert_eq!(cfg.outgoing.host, "smtp.corp.example");
    }

    #[test]
    fn rejects_invalid_addresses() {
        for bad in ["", "no-at-sign", "@example.com", "user@", "user@localhost"] {
            assert!(autodiscover(bad).is_none(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn every_domain_is_unique() {
        let mut all: Vec<_> = PROVIDERS.iter().flat_map(|p| p.domains).collect();
        let len = all.len();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), len, "duplicate domain in PROVIDERS");
    }
}
