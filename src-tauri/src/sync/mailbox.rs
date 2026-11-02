//! 按用户操作打开短连接，按 UID 读取邮件；读取正文不隐式设置已读。

use std::{future::Future, time::Duration};

use async_imap::types::{Flag, NameAttribute};
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::{TryStreamExt, pin_mut};
use tokio::time::timeout;

use crate::{
    account::{Account, AccountCredentials},
    mail::{MAX_RAW_MESSAGE_BYTES, MailFolder, MessageSummary, parse_summary},
};

use super::{connection::ImapSession, pool::ImapPool};

pub(super) const OPERATION_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_FOLDER_MESSAGES: usize = 100_000;
const MAX_FOLDERS: usize = 2_000;
const HEADER_LIMIT: usize = 64 * 1024;
const PAGE_SIZE: usize = 50;

pub struct FolderSync {
    pub uid_validity: u32,
    pub messages: Vec<MessageSummary>,
    pub all_uids: Vec<u32>,
    pub has_more: bool,
}

pub(super) async fn bounded<T>(
    operation: impl Future<Output = Result<T, String>>,
) -> Result<T, String> {
    timeout(OPERATION_TIMEOUT, operation)
        .await
        .map_err(|_| "邮件服务器操作超时，请检查网络后重试。".to_string())?
}

pub(super) async fn finish<T>(
    mut session: ImapSession,
    result: Result<T, String>,
) -> Result<T, String> {
    let _ = timeout(Duration::from_secs(1), session.logout()).await;
    result
}

pub async fn list_folders(
    pool: &ImapPool,
    account: &Account,
    credentials: &AccountCredentials,
) -> Result<Vec<MailFolder>, String> {
    pool.run(account, credentials, |mut session| async move {
        let result = list_in_session(&mut session).await;
        (session, result)
    })
    .await
}

pub(super) async fn list_in_session(session: &mut ImapSession) -> Result<Vec<MailFolder>, String> {
    let stream = session
        .list(Some(""), Some("*"))
        .await
        .map_err(|_| "无法读取服务器文件夹列表。".to_string())?;
    pin_mut!(stream);
    let mut folders = Vec::new();
    while let Some(folder) = stream
        .try_next()
        .await
        .map_err(|_| "文件夹列表响应异常。".to_string())?
    {
        if folders.len() >= MAX_FOLDERS {
            return Err("文件夹数量超过当前版本的 2000 个限制。".into());
        }
        validate_folder(folder.name())?;
        let attributes = folder.attributes();
        let display_name = decode_modified_utf7(folder.name());
        let kind = if folder.name().eq_ignore_ascii_case("INBOX") {
            "inbox"
        } else if attributes.contains(&NameAttribute::Sent) {
            "sent"
        } else if attributes.contains(&NameAttribute::Drafts) {
            "drafts"
        } else if attributes.contains(&NameAttribute::Trash) {
            "trash"
        } else if attributes.contains(&NameAttribute::Junk) {
            "junk"
        } else if attributes.contains(&NameAttribute::Archive) {
            "archive"
        } else {
            "custom"
        };
        folders.push(MailFolder {
            name: folder.name().to_string(),
            display_name,
            selectable: !attributes.contains(&NameAttribute::NoSelect),
            kind: kind.to_string(),
        });
    }
    folders.sort_by(|a, b| {
        (a.kind != "inbox").cmp(&(b.kind != "inbox")).then_with(|| {
            a.display_name
                .to_lowercase()
                .cmp(&b.display_name.to_lowercase())
        })
    });
    Ok(folders)
}

pub async fn sync_folder(
    pool: &ImapPool,
    account: &Account,
    credentials: &AccountCredentials,
    folder: &str,
    before_uid: Option<u32>,
) -> Result<FolderSync, String> {
    validate_folder(folder)?;
    pool.run(account, credentials, |mut session| async move {
        let result = sync_in_session(&mut session, folder, before_uid).await;
        (session, result)
    })
    .await
}

pub(super) async fn sync_in_session(
    session: &mut ImapSession,
    folder: &str,
    before_uid: Option<u32>,
) -> Result<FolderSync, String> {
    validate_folder(folder)?;
    if before_uid == Some(0) {
        return Err("邮件分页位置无效，请刷新文件夹。".into());
    }
    let mailbox = session
        .select(folder)
        .await
        .map_err(|_| "无法打开此文件夹，请刷新文件夹列表。".to_string())?;
    let uid_validity = mailbox
        .uid_validity
        .filter(|value| *value > 0)
        .ok_or_else(|| "服务器未提供有效的 UIDVALIDITY，无法安全缓存邮件。".to_string())?;
    if mailbox.exists as usize > MAX_FOLDER_MESSAGES {
        return Err("此文件夹超过当前版本的 100000 封邮件限制，请先按文件夹整理。".into());
    }
    let ids = session
        .uid_search("ALL")
        .await
        .map_err(|_| "无法读取文件夹邮件索引。".to_string())?;
    if ids.len() > MAX_FOLDER_MESSAGES || ids.contains(&0) {
        return Err("服务器返回的邮件索引数量或 UID 无效。".into());
    }
    let mut all_uids: Vec<u32> = ids.into_iter().collect();
    all_uids.sort_unstable();
    let candidates: Vec<u32> = all_uids
        .iter()
        .rev()
        .copied()
        .filter(|uid| before_uid.is_none_or(|before| *uid < before))
        .collect();
    let has_more = candidates.len() > PAGE_SIZE;
    let requested = &candidates[..candidates.len().min(PAGE_SIZE)];
    let mut messages = Vec::with_capacity(requested.len());
    if !requested.is_empty() {
        let sequence = requested
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let query = format!("(UID FLAGS RFC822.SIZE BODY.PEEK[HEADER]<0.{HEADER_LIMIT}>)");
        let stream = session
            .uid_fetch(sequence, query)
            .await
            .map_err(|_| "无法读取邮件摘要。".to_string())?;
        pin_mut!(stream);
        while let Some(fetch) = stream
            .try_next()
            .await
            .map_err(|_| "邮件摘要响应异常。".to_string())?
        {
            let Some(uid) = fetch.uid.filter(|uid| requested.contains(uid)) else {
                continue;
            };
            let Some(header) = fetch.header() else {
                continue;
            };
            if header.len() >= HEADER_LIMIT {
                return Err("某封邮件的头部过大，暂时无法读取该页。".into());
            }
            let size = fetch
                .size
                .ok_or_else(|| "服务器未提供邮件大小。".to_string())?;
            let unread = !fetch.flags().any(|flag| flag == Flag::Seen);
            let flagged = fetch.flags().any(|flag| flag == Flag::Flagged);
            messages.push(parse_summary(
                header,
                uid,
                uid_validity,
                unread,
                flagged,
                size,
            )?);
            if messages.len() > PAGE_SIZE {
                return Err("服务器返回了重复或过量的邮件摘要。".into());
            }
        }
    }
    messages.sort_by_key(|message| std::cmp::Reverse(message.uid));
    messages.dedup_by_key(|message| message.uid);
    Ok(FolderSync {
        uid_validity,
        messages,
        all_uids,
        has_more,
    })
}

pub async fn fetch_message(
    pool: &ImapPool,
    account: &Account,
    credentials: &AccountCredentials,
    folder: &str,
    uid: u32,
    expected_uid_validity: u32,
) -> Result<Vec<u8>, String> {
    validate_message_key(folder, uid, expected_uid_validity)?;
    pool.run(account, credentials, |mut session| async move {
        let result = fetch_in_session(&mut session, folder, uid, expected_uid_validity).await;
        (session, result)
    })
    .await
}

/// 一次 UID FETCH 批量下载多封邮件正文，供后台预取使用。调用方负责按
/// 同步时得到的 RFC822.SIZE 控制每批总量；服务器未返回的 UID 直接跳过。
pub async fn fetch_messages(
    pool: &ImapPool,
    account: &Account,
    credentials: &AccountCredentials,
    folder: &str,
    expected_uid_validity: u32,
    uids: &[u32],
) -> Result<Vec<(u32, Vec<u8>)>, String> {
    if uids.is_empty() {
        return Ok(Vec::new());
    }
    for uid in uids {
        validate_message_key(folder, *uid, expected_uid_validity)?;
    }
    pool.run(account, credentials, |mut session| async move {
        let result = fetch_many_in_session(&mut session, folder, expected_uid_validity, uids).await;
        (session, result)
    })
    .await
}

pub(super) async fn fetch_many_in_session(
    session: &mut ImapSession,
    folder: &str,
    expected: u32,
    uids: &[u32],
) -> Result<Vec<(u32, Vec<u8>)>, String> {
    select_validity(session, folder, expected).await?;
    let sequence = uids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let query = format!("(UID BODY.PEEK[]<0.{}>)", MAX_RAW_MESSAGE_BYTES + 1);
    let stream = session
        .uid_fetch(sequence, query)
        .await
        .map_err(|_| "无法下载邮件正文。".to_string())?;
    pin_mut!(stream);
    let mut bodies: Vec<(u32, Vec<u8>)> = Vec::with_capacity(uids.len());
    while let Some(fetch) = stream
        .try_next()
        .await
        .map_err(|_| "下载邮件正文失败。".to_string())?
    {
        let Some(uid) = fetch.uid.filter(|uid| uids.contains(uid)) else {
            continue;
        };
        if let Some(body) = fetch.body()
            && body.len() <= MAX_RAW_MESSAGE_BYTES
            && !bodies.iter().any(|(seen, _)| *seen == uid)
        {
            bodies.push((uid, body.to_vec()));
        }
    }
    Ok(bodies)
}

pub(super) async fn select_checked(
    session: &mut ImapSession,
    folder: &str,
    uid: u32,
    expected: u32,
) -> Result<(), String> {
    validate_message_key(folder, uid, expected)?;
    select_validity(session, folder, expected).await
}

async fn select_validity(
    session: &mut ImapSession,
    folder: &str,
    expected: u32,
) -> Result<(), String> {
    let mailbox = session
        .select(folder)
        .await
        .map_err(|_| "无法打开此文件夹。".to_string())?;
    if mailbox.uid_validity != Some(expected) {
        return Err("服务器文件夹标识已改变（UIDVALIDITY），请重新同步后再操作邮件。".into());
    }
    Ok(())
}

pub(super) async fn fetch_in_session(
    session: &mut ImapSession,
    folder: &str,
    uid: u32,
    expected: u32,
) -> Result<Vec<u8>, String> {
    select_checked(session, folder, uid, expected).await?;
    let size = {
        let stream = session
            .uid_fetch(uid.to_string(), "(UID RFC822.SIZE)")
            .await
            .map_err(|_| "无法查询邮件大小。".to_string())?;
        pin_mut!(stream);
        let mut size = None;
        while let Some(fetch) = stream
            .try_next()
            .await
            .map_err(|_| "查询邮件大小失败。".to_string())?
        {
            if fetch.uid == Some(uid) {
                size = fetch.size;
            }
        }
        size.ok_or_else(|| "邮件已不存在，请刷新文件夹。".to_string())?
    };
    if size as usize > MAX_RAW_MESSAGE_BYTES {
        return Err("此邮件超过 20 MiB，当前版本暂不下载正文。".into());
    }
    let query = format!("(UID BODY.PEEK[]<0.{}>)", MAX_RAW_MESSAGE_BYTES + 1);
    let stream = session
        .uid_fetch(uid.to_string(), query)
        .await
        .map_err(|_| "无法下载邮件正文。".to_string())?;
    pin_mut!(stream);
    let mut raw = None;
    while let Some(fetch) = stream
        .try_next()
        .await
        .map_err(|_| "下载邮件正文失败。".to_string())?
    {
        if fetch.uid != Some(uid) {
            continue;
        }
        if let Some(body) = fetch.body() {
            if body.len() > MAX_RAW_MESSAGE_BYTES {
                return Err("邮件实际大小超过 20 MiB，已停止读取。".into());
            }
            raw = Some(body.to_vec());
        }
    }
    raw.ok_or_else(|| "服务器没有返回邮件正文，邮件可能已被删除。".to_string())
}

pub async fn set_message_read(
    pool: &ImapPool,
    account: &Account,
    credentials: &AccountCredentials,
    folder: &str,
    uid: u32,
    expected_uid_validity: u32,
    read: bool,
) -> Result<(), String> {
    validate_message_key(folder, uid, expected_uid_validity)?;
    // 设置 \Seen 是幂等的，复用连接失败后重试一次不会产生副作用。
    pool.run(account, credentials, |mut session| async move {
        let result =
            set_read_in_session(&mut session, folder, uid, expected_uid_validity, read).await;
        (session, result)
    })
    .await
}

pub(super) async fn set_read_in_session(
    session: &mut ImapSession,
    folder: &str,
    uid: u32,
    expected: u32,
    read: bool,
) -> Result<(), String> {
    select_checked(session, folder, uid, expected).await?;
    {
        let command = if read {
            "+FLAGS.SILENT (\\Seen)"
        } else {
            "-FLAGS.SILENT (\\Seen)"
        };
        let stream = session
            .uid_store(uid.to_string(), command)
            .await
            .map_err(|_| "无法修改邮件已读状态。".to_string())?;
        pin_mut!(stream);
        while stream
            .try_next()
            .await
            .map_err(|_| "服务器拒绝修改已读状态。".to_string())?
            .is_some()
        {}
    }
    // STORE 对不存在的 UID 也可能返回 OK；确认目标仍存在并且标记确实已保存。
    let stream = session
        .uid_fetch(uid.to_string(), "(UID FLAGS)")
        .await
        .map_err(|_| "无法确认邮件已读状态。".to_string())?;
    pin_mut!(stream);
    let mut verified = false;
    while let Some(fetch) = stream
        .try_next()
        .await
        .map_err(|_| "无法确认邮件已读状态。".to_string())?
    {
        if fetch.uid == Some(uid) {
            verified = fetch.flags().any(|flag| flag == Flag::Seen) == read;
        }
    }
    if !verified {
        return Err("邮件已不存在或服务器未保存已读标记，请刷新文件夹。".into());
    }
    Ok(())
}

fn validate_message_key(folder: &str, uid: u32, expected: u32) -> Result<(), String> {
    validate_folder(folder)?;
    if uid == 0 || expected == 0 {
        return Err("邮件标识无效，请刷新文件夹。".into());
    }
    Ok(())
}

fn validate_folder(folder: &str) -> Result<(), String> {
    if folder.is_empty() || folder.len() > 4096 || folder.chars().any(char::is_control) {
        return Err("文件夹名称无效。".into());
    }
    Ok(())
}

/// IMAP modified UTF-7 仅用于显示；所有服务器操作保留 LIST 返回的原名。
fn decode_modified_utf7(name: &str) -> String {
    let mut decoded = String::new();
    let mut rest = name;
    while let Some(index) = rest.find('&') {
        decoded.push_str(&rest[..index]);
        rest = &rest[index + 1..];
        let Some(end) = rest.find('-') else {
            return name.to_string();
        };
        let encoded = &rest[..end];
        if encoded.is_empty() {
            decoded.push('&');
        } else {
            let mut base64 = encoded.replace(',', "/");
            while !base64.len().is_multiple_of(4) {
                base64.push('=');
            }
            let Ok(bytes) = STANDARD.decode(base64) else {
                return name.to_string();
            };
            if !bytes.len().is_multiple_of(2) {
                return name.to_string();
            }
            let units: Vec<u16> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                .collect();
            let Ok(value) = String::from_utf16(&units) else {
                return name.to_string();
            };
            if value.chars().any(char::is_control) {
                return name.to_string();
            }
            decoded.push_str(&value);
        }
        rest = &rest[end + 1..];
    }
    decoded.push_str(rest);
    decoded
}

#[cfg(test)]
mod tests {
    use super::decode_modified_utf7;

    #[test]
    fn decodes_folder_names_without_changing_invalid_names() {
        assert_eq!(decode_modified_utf7("&XfJT0ZAB-"), "已发送");
        assert_eq!(decode_modified_utf7("Work &- Family"), "Work & Family");
        assert_eq!(decode_modified_utf7("bad&broken"), "bad&broken");
        assert_eq!(decode_modified_utf7("Inbox"), "Inbox");
    }
}
