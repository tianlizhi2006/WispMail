//! 同步层：与邮件服务器通信，收发邮件。
//!
//! 职责：
//! - IMAP 同步邮件，使用 IDLE 实现新邮件实时到达。
//! - POP3 收信。
//! - SMTP 发信。
//! - 把服务器状态与本地缓存（storage 层）对齐：拉取新邮件、同步已读 / 星标 /
//!   删除 / 移动等操作。
//!
//! 后续将引入 `async-imap`（含 IDLE）、`lettre`（SMTP）、
//! `mail-parser` / `mail-builder`（解析与生成邮件）。

use crate::account::Account;

/// 一次同步的结果摘要。
#[derive(Debug, Default)]
pub struct SyncOutcome {
    pub new_messages: u32,
    pub updated_flags: u32,
}

/// 同步指定账户的邮件。
///
/// TODO: 实现 IMAP 增量同步与 IDLE 监听。
pub async fn sync_account(_account: &Account) -> Result<SyncOutcome, SyncError> {
    Ok(SyncOutcome::default())
}

/// 同步过程中可能出现的错误。
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("尚未实现")]
    NotImplemented,
}
