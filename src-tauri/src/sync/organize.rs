//! 邮件整理：星标 / 已读标记、移动、归档、删除。
//!
//! 所有操作按 UID 执行，并在执行前核对 UIDVALIDITY，避免文件夹换代后误操作
//! 其他邮件。服务器支持 MOVE 时使用原子移动，否则退回 COPY + 删除标记 + 清除。

use async_imap::types::Flag;
use futures_util::{TryStreamExt, pin_mut};

use crate::{
    account::{Account, AccountCredentials},
    mail::MailFolder,
};

use super::{
    connection::ImapSession,
    mailbox::{list_in_session, select_validity, validate_folder},
    pool::ImapPool,
};

/// 单次批量操作的最大邮件数，避免生成过长的 IMAP 命令。
pub const MAX_BATCH: usize = 1000;

/// 在同一个文件夹、同一代 UID 下的一组邮件。
#[derive(Clone, Copy)]
pub struct MessageSet<'a> {
    pub folder: &'a str,
    pub uid_validity: u32,
    pub uids: &'a [u32],
}

impl MessageSet<'_> {
    fn validate(&self) -> Result<(), String> {
        validate_folder(self.folder)?;
        if self.uid_validity == 0
            || self.uids.is_empty()
            || self.uids.len() > MAX_BATCH
            || self.uids.contains(&0)
        {
            return Err("邮件标识无效，请刷新文件夹。".into());
        }
        Ok(())
    }

    fn sequence(&self) -> String {
        self.uids
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",")
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MailFlag {
    Seen,
    Flagged,
}

impl MailFlag {
    fn imap(self) -> &'static str {
        match self {
            Self::Seen => "\\Seen",
            Self::Flagged => "\\Flagged",
        }
    }

    fn present(self, flag: &Flag<'_>) -> bool {
        matches!(
            (self, flag),
            (Self::Seen, Flag::Seen) | (Self::Flagged, Flag::Flagged)
        )
    }

    fn unconfirmed(self) -> &'static str {
        match self {
            Self::Seen => "邮件已不存在或服务器未保存已读标记，请刷新文件夹。",
            Self::Flagged => "邮件已不存在或服务器未保存星标，请刷新文件夹。",
        }
    }
}

/// 特殊用途文件夹。优先使用服务器声明的 SPECIAL-USE 属性，其次按常见名称匹配。
#[derive(Clone, Copy)]
pub(super) enum SpecialFolder {
    Sent,
    Trash,
    Archive,
}

impl SpecialFolder {
    fn kind(self) -> &'static str {
        match self {
            Self::Sent => "sent",
            Self::Trash => "trash",
            Self::Archive => "archive",
        }
    }

    fn names(self) -> &'static [&'static str] {
        super::folders::special_names(self.kind())
    }

    pub(super) fn find(self, folders: &[MailFolder]) -> Option<&MailFolder> {
        folders
            .iter()
            .find(|folder| folder.selectable && folder.kind == self.kind())
            .or_else(|| {
                folders.iter().find(|folder| {
                    folder.selectable
                        && self
                            .names()
                            .contains(&folder.display_name.to_lowercase().as_str())
                })
            })
    }
}

async fn drain_store(session: &mut ImapSession, set: &str, command: &str) -> Result<(), String> {
    let stream = session
        .uid_store(set, command)
        .await
        .map_err(|_| "服务器拒绝修改邮件标记。".to_string())?;
    pin_mut!(stream);
    while stream
        .try_next()
        .await
        .map_err(|_| "服务器拒绝修改邮件标记。".to_string())?
        .is_some()
    {}
    Ok(())
}

async fn expunge(session: &mut ImapSession, set: &str, uidplus: bool) -> Result<(), String> {
    let error = || "服务器未能清除已删除的邮件，请刷新文件夹后重试。".to_string();
    if uidplus {
        // UID EXPUNGE 只清除本次操作的邮件，不影响其他客户端标记删除的邮件。
        let stream = session.uid_expunge(set).await.map_err(|_| error())?;
        pin_mut!(stream);
        while stream.try_next().await.map_err(|_| error())?.is_some() {}
    } else {
        let stream = session.expunge().await.map_err(|_| error())?;
        pin_mut!(stream);
        while stream.try_next().await.map_err(|_| error())?.is_some() {}
    }
    Ok(())
}

pub(super) async fn set_flag_in_session(
    session: &mut ImapSession,
    set: MessageSet<'_>,
    flag: MailFlag,
    value: bool,
) -> Result<(), String> {
    set.validate()?;
    select_validity(session, set.folder, set.uid_validity).await?;
    let sequence = set.sequence();
    let command = format!(
        "{}FLAGS.SILENT ({})",
        if value { "+" } else { "-" },
        flag.imap()
    );
    drain_store(session, &sequence, &command).await?;
    // STORE 对不存在的 UID 也可能返回 OK；逐封确认邮件仍存在且标记确实已保存。
    let stream = session
        .uid_fetch(&sequence, "(UID FLAGS)")
        .await
        .map_err(|_| "无法确认邮件标记。".to_string())?;
    pin_mut!(stream);
    let mut confirmed = Vec::with_capacity(set.uids.len());
    while let Some(fetch) = stream
        .try_next()
        .await
        .map_err(|_| "无法确认邮件标记。".to_string())?
    {
        if let Some(uid) = fetch.uid.filter(|uid| set.uids.contains(uid))
            && fetch.flags().any(|item| flag.present(&item)) == value
            && !confirmed.contains(&uid)
        {
            confirmed.push(uid);
        }
    }
    if confirmed.len() != set.uids.len() {
        return Err(flag.unconfirmed().into());
    }
    Ok(())
}

pub(super) async fn move_in_session(
    session: &mut ImapSession,
    set: MessageSet<'_>,
    target: &str,
) -> Result<(), String> {
    set.validate()?;
    validate_folder(target)?;
    if target == set.folder {
        return Err("邮件已在该文件夹中。".into());
    }
    let capabilities = session
        .capabilities()
        .await
        .map_err(|_| "无法读取服务器功能列表。".to_string())?;
    select_validity(session, set.folder, set.uid_validity).await?;
    let sequence = set.sequence();
    if capabilities.has_str("MOVE") {
        session
            .uid_mv(&sequence, target)
            .await
            .map_err(|_| "服务器未能移动邮件，请刷新文件夹后重试。".to_string())?;
    } else {
        session
            .uid_copy(&sequence, target)
            .await
            .map_err(|_| "服务器未能复制邮件到目标文件夹，原邮件保持不变。".to_string())?;
        drain_store(session, &sequence, "+FLAGS.SILENT (\\Deleted)").await?;
        expunge(session, &sequence, capabilities.has_str("UIDPLUS")).await?;
    }
    Ok(())
}

pub(super) async fn delete_permanently_in_session(
    session: &mut ImapSession,
    set: MessageSet<'_>,
) -> Result<(), String> {
    set.validate()?;
    let capabilities = session
        .capabilities()
        .await
        .map_err(|_| "无法读取服务器功能列表。".to_string())?;
    select_validity(session, set.folder, set.uid_validity).await?;
    let sequence = set.sequence();
    drain_store(session, &sequence, "+FLAGS.SILENT (\\Deleted)").await?;
    expunge(session, &sequence, capabilities.has_str("UIDPLUS")).await
}

/// 查找特殊文件夹；不存在时按需创建（仅归档使用），返回服务器上的文件夹原名。
async fn special_in_session(
    session: &mut ImapSession,
    special: SpecialFolder,
    create: Option<&str>,
) -> Result<Option<String>, String> {
    let folders = list_in_session(session).await?;
    if let Some(folder) = special.find(&folders) {
        return Ok(Some(folder.name.clone()));
    }
    let Some(name) = create else {
        return Ok(None);
    };
    session
        .create(name)
        .await
        .map_err(|_| format!("服务器没有可用的文件夹，且不允许创建 {name} 文件夹。"))?;
    Ok(Some(name.to_string()))
}

pub async fn set_flags(
    pool: &ImapPool,
    account: &Account,
    credentials: &AccountCredentials,
    set: MessageSet<'_>,
    flag: MailFlag,
    value: bool,
) -> Result<(), String> {
    set.validate()?;
    // 增删标记是幂等的，复用连接失败后重试一次不会产生副作用。
    pool.run(account, credentials, |mut session| async move {
        let result = set_flag_in_session(&mut session, set, flag, value).await;
        (session, result)
    })
    .await
}

pub async fn move_messages(
    pool: &ImapPool,
    account: &Account,
    credentials: &AccountCredentials,
    set: MessageSet<'_>,
    target: &str,
) -> Result<(), String> {
    set.validate()?;
    pool.run_once(account, credentials, |mut session| async move {
        let result = move_in_session(&mut session, set, target).await;
        (session, result)
    })
    .await
}

/// 移到归档文件夹，没有时创建 "Archive"。返回目标文件夹名。
pub async fn archive_messages(
    pool: &ImapPool,
    account: &Account,
    credentials: &AccountCredentials,
    set: MessageSet<'_>,
) -> Result<String, String> {
    set.validate()?;
    pool.run_once(account, credentials, |mut session| async move {
        let result = async {
            let target = special_in_session(&mut session, SpecialFolder::Archive, Some("Archive"))
                .await?
                .ok_or_else(|| "服务器没有归档文件夹。".to_string())?;
            if target == set.folder {
                return Err("邮件已在归档文件夹中。".to_string());
            }
            move_in_session(&mut session, set, &target).await?;
            Ok(target)
        }
        .await;
        (session, result)
    })
    .await
}

/// 移到废纸篓，返回目标文件夹名。服务器没有废纸篓或邮件已在废纸篓中时
/// 不做任何修改并返回 `None`，由界面向用户确认后改用永久删除。
pub async fn trash_messages(
    pool: &ImapPool,
    account: &Account,
    credentials: &AccountCredentials,
    set: MessageSet<'_>,
) -> Result<Option<String>, String> {
    set.validate()?;
    pool.run_once(account, credentials, |mut session| async move {
        let result = async {
            let Some(target) = special_in_session(&mut session, SpecialFolder::Trash, None)
                .await?
                .filter(|target| target != set.folder)
            else {
                return Ok(None);
            };
            move_in_session(&mut session, set, &target).await?;
            Ok(Some(target))
        }
        .await;
        (session, result)
    })
    .await
}

pub async fn delete_permanently(
    pool: &ImapPool,
    account: &Account,
    credentials: &AccountCredentials,
    set: MessageSet<'_>,
) -> Result<(), String> {
    set.validate()?;
    // 删除标记 + 清除可安全重试：已清除的邮件不会再次匹配。
    pool.run(account, credentials, |mut session| async move {
        let result = delete_permanently_in_session(&mut session, set).await;
        (session, result)
    })
    .await
}
