//! 账户层：邮箱账户的模型、增删改查与凭据管理。
//!
//! 职责：
//! - 定义邮箱账户的数据模型（服务器地址、端口、加密方式、认证方式等）。
//! - 根据邮箱地址自动识别常见服务商的服务器配置（见 [`providers`]）。
//! - 管理多账户的添加、编辑、删除。
//! - 对接系统凭据管理器安全存储密码 / 授权码 / OAuth 令牌。
//!
//! 凭据通过系统凭据管理器保存，账户配置独立保存于 SQLite。
//!
//! 序列化约定：结构体字段以 camelCase 传给前端，与 `src/lib/types.ts` 保持一致。

pub mod credentials;
pub mod providers;
pub mod service;
mod validation;

use serde::{Deserialize, Serialize};

pub use providers::{DiscoveredConfig, autodiscover};
pub use service::AccountService;

/// 收发信使用的加密方式。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Encryption {
    /// 不加密（不推荐）。
    None,
    /// 建立连接后升级为 TLS。
    StartTls,
    /// 全程 TLS / SSL。
    Tls,
}

/// 账户登录认证方式。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AuthMethod {
    /// 用户名加密码 / 授权码 / 应用专用密码。
    Password,
    /// OAuth2 授权登录（Gmail、Outlook 等）。
    OAuth2,
}

/// 单个邮件服务器（收信或发信）的连接配置。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub encryption: Encryption,
}

/// 收信使用的协议。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum IncomingProtocol {
    Imap,
    Pop3,
}

/// 一个邮箱账户的完整配置。凭据本身不在此结构中，另存于系统凭据管理器。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    /// 本地唯一标识。
    pub id: String,
    /// 完整邮箱地址。
    pub email: String,
    /// 显示名称（发件人姓名）。
    pub display_name: String,
    /// 登录名可与邮箱地址不同；旧配置缺省时仍使用邮箱地址。
    #[serde(default)]
    pub incoming_username: String,
    #[serde(default)]
    pub outgoing_username: String,
    #[serde(default = "default_use_same_password")]
    pub use_same_password: bool,
    pub incoming_protocol: IncomingProtocol,
    pub incoming: ServerConfig,
    pub outgoing: ServerConfig,
    pub auth: AuthMethod,
}

fn default_use_same_password() -> bool {
    true
}

impl Account {
    pub fn incoming_login(&self) -> &str {
        if self.incoming_username.trim().is_empty() {
            &self.email
        } else {
            &self.incoming_username
        }
    }

    pub fn outgoing_login(&self) -> &str {
        if self.outgoing_username.trim().is_empty() {
            &self.email
        } else {
            &self.outgoing_username
        }
    }

    /// 仅在内存中补齐旧配置，读取账户不会触发迁移或改写凭据。
    pub(crate) fn normalize_usernames(&mut self) {
        if self.incoming_username.trim().is_empty() {
            self.incoming_username = self.email.clone();
        }
        if self.outgoing_username.trim().is_empty() {
            self.outgoing_username = self.email.clone();
        }
    }
}

/// 仅在后端验证与凭据保存期间存在；禁止序列化或调试输出。
pub struct AccountCredentials {
    pub incoming_password: String,
    pub outgoing_password: String,
}

/// 添加或编辑账户的请求。密码只用于验证连接并交给系统凭据管理器。
/// 编辑时不传密码或传空字符串表示保留原凭据。
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountInput {
    pub id: Option<String>,
    pub email: String,
    pub display_name: String,
    #[serde(default)]
    pub incoming_username: String,
    #[serde(default)]
    pub outgoing_username: String,
    #[serde(default = "default_use_same_password")]
    pub use_same_password: bool,
    pub incoming_protocol: IncomingProtocol,
    pub incoming: ServerConfig,
    pub outgoing: ServerConfig,
    pub auth: AuthMethod,
    pub password: Option<String>,
    pub outgoing_password: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 序列化格式必须与前端 `src/lib/types.ts` 一致。
    #[test]
    fn serializes_with_frontend_naming() {
        let account = Account {
            id: "1".into(),
            email: "a@example.com".into(),
            display_name: "A".into(),
            incoming_username: "CORP\\alice".into(),
            outgoing_username: "smtp-alice".into(),
            use_same_password: false,
            incoming_protocol: IncomingProtocol::Imap,
            incoming: ServerConfig {
                host: "imap.example.com".into(),
                port: 993,
                encryption: Encryption::Tls,
            },
            outgoing: ServerConfig {
                host: "smtp.example.com".into(),
                port: 587,
                encryption: Encryption::StartTls,
            },
            auth: AuthMethod::OAuth2,
        };
        let json = serde_json::to_value(&account).unwrap();
        assert_eq!(json["displayName"], "A");
        assert_eq!(json["incomingProtocol"], "imap");
        assert_eq!(json["auth"], "oauth2");
        assert_eq!(json["incomingUsername"], "CORP\\alice");
        assert_eq!(json["outgoingUsername"], "smtp-alice");
        assert_eq!(json["useSamePassword"], false);
        assert_eq!(json["outgoing"]["encryption"], "starttls");
    }

    #[test]
    fn legacy_json_defaults_to_email_and_shared_credentials() {
        let legacy = serde_json::json!({
            "id": "legacy", "email": "a@example.com", "displayName": "A",
            "incomingProtocol": "imap", "auth": "password",
            "incoming": {"host": "mail.example.com", "port": 993, "encryption": "tls"},
            "outgoing": {"host": "mail.example.com", "port": 587, "encryption": "starttls"}
        });
        let account: Account = serde_json::from_value(legacy.clone()).unwrap();
        assert!(account.use_same_password);
        assert_eq!(account.incoming_login(), "a@example.com");
        assert_eq!(account.outgoing_login(), "a@example.com");
        let input: AccountInput = serde_json::from_value(legacy).unwrap();
        assert!(input.use_same_password);
        assert!(input.incoming_username.is_empty());
        assert!(input.outgoing_password.is_none());
    }
}
