//! 账户层：邮箱账户的模型、增删改查与凭据管理。
//!
//! 职责：
//! - 定义邮箱账户的数据模型（服务器地址、端口、加密方式、认证方式等）。
//! - 根据邮箱地址自动识别常见服务商的服务器配置。
//! - 管理多账户的添加、编辑、删除。
//! - 对接系统凭据管理器安全存储密码 / 授权码 / OAuth 令牌。
//!
//! 后续将引入 `keyring` 存储凭据、`oauth2` 处理授权登录。

use serde::{Deserialize, Serialize};

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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    /// 用户名加密码 / 授权码。
    Password,
    /// OAuth2 授权登录（Gmail、Outlook 等）。
    OAuth2,
}

/// 单个邮件服务器（收信或发信）的连接配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    /// 本地唯一标识。
    pub id: String,
    /// 完整邮箱地址。
    pub email: String,
    /// 显示名称（发件人姓名）。
    pub display_name: String,
    pub incoming_protocol: IncomingProtocol,
    pub incoming: ServerConfig,
    pub outgoing: ServerConfig,
    pub auth: AuthMethod,
}

/// 根据邮箱地址自动推断服务器配置。
///
/// TODO: 接入 Thunderbird ISPDB / autoconfig，先内置常见服务商表作为兜底。
pub fn autodiscover(_email: &str) -> Option<Account> {
    None
}
