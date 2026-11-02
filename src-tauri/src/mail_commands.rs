//! 邮件 IPC：短时间持有数据库锁；网络等待不阻塞离线缓存和草稿。

use std::future::Future;
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

use crate::{
    account::{Account, AccountCredentials, AccountService},
    commands::{AccountsState, ConnectionPermit, with_accounts},
    mail::{
        self, ComposeMessage, MailFolder, MessageDetail, MessagePage, MessageSummary, SendResult,
    },
    storage::StoreError,
    sync,
};

fn ensure_current(service: &AccountService, snapshot: &Account) -> Result<(), String> {
    if service.get_account(&snapshot.id)? != *snapshot {
        return Err("账户设置已变化，请重新打开邮箱后重试。".into());
    }
    Ok(())
}

async fn connection(
    state: &AccountsState,
    account_id: String,
) -> Result<(Account, AccountCredentials, ConnectionPermit), String> {
    let permit = state.network_slot(&account_id).await?;
    let (account, credentials) =
        with_accounts(state, move |service| service.load_connection(&account_id)).await?;
    Ok((account, credentials, permit))
}

#[tauri::command]
pub async fn list_cached_folders(
    account_id: String,
    state: tauri::State<'_, AccountsState>,
) -> Result<Vec<MailFolder>, String> {
    with_accounts(&state, move |service| {
        service.get_account(&account_id)?;
        service
            .store()
            .list_folders(&account_id)
            .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub async fn refresh_folders(
    account_id: String,
    state: tauri::State<'_, AccountsState>,
) -> Result<Vec<MailFolder>, String> {
    let (account, credentials, _permit) = connection(&state, account_id).await?;
    let folders = sync::list_folders(&state.imap, &account, &credentials).await?;
    with_accounts(&state, move |service| {
        ensure_current(service, &account)?;
        service
            .store()
            .replace_folders(&account.id, &folders)
            .map_err(|e| e.to_string())?;
        Ok(folders)
    })
    .await
}

#[tauri::command]
pub async fn list_cached_messages(
    account_id: String,
    folder: String,
    before_uid: Option<u32>,
    state: tauri::State<'_, AccountsState>,
) -> Result<MessagePage, String> {
    with_accounts(&state, move |service| {
        service.get_account(&account_id)?;
        service
            .store()
            .list_messages(&account_id, &folder, before_uid)
            .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub async fn sync_folder(
    account_id: String,
    folder: String,
    before_uid: Option<u32>,
    expected_uid_validity: Option<u32>,
    app: tauri::AppHandle,
    state: tauri::State<'_, AccountsState>,
) -> Result<MessagePage, String> {
    let page = {
        let (account, credentials, _permit) = connection(&state, account_id).await?;
        let cached_account = account.id.clone();
        let cached_folder = folder.clone();
        let known_validity = match expected_uid_validity {
            Some(value) => Some(value),
            None => {
                with_accounts(&state, move |service| {
                    service
                        .store()
                        .cached_folder_validity(&cached_account, &cached_folder)
                        .map_err(|e| e.to_string())
                })
                .await?
            }
        };
        let (page, cursor) = sync_page(before_uid, known_validity, |cursor| {
            sync::sync_folder(&state.imap, &account, &credentials, &folder, cursor)
        })
        .await?;
        let folder = folder.clone();
        with_accounts(&state, move |service| {
            ensure_current(service, &account)?;
            service
                .store()
                .cache_folder_page(
                    &account.id,
                    &folder,
                    page.uid_validity,
                    &page.messages,
                    Some(&page.all_uids),
                )
                .map_err(|e| e.to_string())?;
            // 返回缓存合并后的摘要，保留已下载正文产生的预览和准确附件标记。
            let result = service
                .store()
                .list_messages(&account.id, &folder, cursor)
                .map_err(|e| e.to_string())?;
            Ok((account.id, result))
        })
        .await?
        // 连接许可在此释放，预取任务随后排队使用同一账户的连接。
    };
    let (account_id, page) = page;
    if let Some(validity) = page.uid_validity {
        spawn_prefetch(app, account_id, folder, validity);
    }
    Ok(page)
}

async fn sync_page<F, Fut>(
    before_uid: Option<u32>,
    expected: Option<u32>,
    mut request: F,
) -> Result<(sync::FolderSync, Option<u32>), String>
where
    F: FnMut(Option<u32>) -> Fut,
    Fut: Future<Output = Result<sync::FolderSync, String>>,
{
    let page = request(before_uid).await?;
    if before_uid.is_some() && expected.is_some_and(|value| value != page.uid_validity) {
        // UID 只在同一 UIDVALIDITY 内有意义，旧游标不能用来截断新一代邮箱。
        return Ok((request(None).await?, None));
    }
    Ok((page, before_uid))
}

async fn message_data(
    state: &AccountsState,
    account_id: String,
    folder: String,
    uid: u32,
    uid_validity: u32,
) -> Result<(MessageSummary, Vec<u8>), String> {
    let cached_account = account_id.clone();
    let cached_folder = folder.clone();
    let (summary, cached) = with_accounts(state, move |service| {
        service.get_account(&cached_account)?;
        let summary = service
            .store()
            .get_message_summary(&cached_account, &cached_folder, uid, uid_validity)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "邮件不在当前缓存列表中，请刷新文件夹后重试。".to_string())?;
        let raw = service
            .store()
            .get_message_raw(&cached_account, &cached_folder, uid, uid_validity)
            .map_err(|e| e.to_string())?;
        Ok((summary, raw))
    })
    .await?;
    if let Some(raw) = cached {
        return Ok((summary, raw));
    }
    let (account, credentials, _permit) = connection(state, account_id).await?;
    let raw = sync::fetch_message(
        &state.imap,
        &account,
        &credentials,
        &folder,
        uid,
        uid_validity,
    )
    .await?;
    Ok((summary, raw))
}

async fn parse(raw: Vec<u8>, summary: MessageSummary) -> Result<(Vec<u8>, MessageDetail), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let detail = mail::parse_message(&raw, summary)?;
        Ok::<_, String>((raw, detail))
    })
    .await
    .map_err(|_| "无法解析邮件，请稍后重试。".to_string())?
}

/// 保存正文缓存。网络等待期间也可能发生文件夹刷新，`cache_message` 只会更新
/// 当前一代 (UIDVALIDITY) 中仍存在的邮件，不会把旧 UID 的正文写入新邮箱。
async fn store_detail(
    state: &AccountsState,
    account_id: String,
    folder: String,
    uid: u32,
    uid_validity: u32,
    raw: Vec<u8>,
    detail: MessageDetail,
) -> Result<MessageDetail, String> {
    with_accounts(state, move |service| {
        service.get_account(&account_id)?;
        service
            .store()
            .cache_message(&account_id, &folder, uid, uid_validity, &raw, &detail)
            .map_err(|error| match error {
                StoreError::MailNotFound => "邮件列表已变化，请重新选择邮件。".to_string(),
                other => other.to_string(),
            })
    })
    .await
}

#[tauri::command]
pub async fn get_message(
    account_id: String,
    folder: String,
    uid: u32,
    uid_validity: u32,
    state: tauri::State<'_, AccountsState>,
) -> Result<MessageDetail, String> {
    // 快速路径：已解析的正文直接从数据库返回，不重新解析 MIME，也不连接服务器。
    let lookup_account = account_id.clone();
    let lookup_folder = folder.clone();
    let cached = with_accounts(&state, move |service| {
        service.get_account(&lookup_account)?;
        service
            .store()
            .get_message_detail(&lookup_account, &lookup_folder, uid, uid_validity)
            .map_err(|e| e.to_string())
    })
    .await?;
    if let Some(detail) = cached {
        return Ok(detail);
    }
    let (summary, raw) = message_data(
        &state,
        account_id.clone(),
        folder.clone(),
        uid,
        uid_validity,
    )
    .await?;
    let (raw, detail) = parse(raw, summary).await?;
    store_detail(&state, account_id, folder, uid, uid_validity, raw, detail).await
}

/// 每次预取的邮件数量与总字节上限：一次批量 FETCH，避免逐封往返。
const PREFETCH_BATCH: usize = 10;
const PREFETCH_BATCH_BYTES: u64 = 8 * 1024 * 1024;
/// 每个文件夹后台预取的最新邮件数量（约一屏）；超大邮件留到用户打开时再下载。
const PREFETCH_LIMIT: usize = 30;
const PREFETCH_MAX_MESSAGE_BYTES: u32 = 2 * 1024 * 1024;

/// 同步完成后在后台下载并解析最新邮件的正文，让打开邮件和离线阅读无需等待网络。
/// 预取与用户操作共享按账户串行的连接许可，用户点击的邮件不会被长时间阻塞：
/// 每批下载完成后立即释放许可。
fn spawn_prefetch(app: tauri::AppHandle, account_id: String, folder: String, uid_validity: u32) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AccountsState>();
        if !state.begin_prefetch(&account_id, &folder) {
            return;
        }
        let _ = prefetch(&state, &account_id, &folder, uid_validity).await;
        state.end_prefetch(&account_id, &folder);
    });
}

async fn prefetch(
    state: &AccountsState,
    account_id: &str,
    folder: &str,
    uid_validity: u32,
) -> Result<(), String> {
    // 只查询一次候选列表：已缓存的邮件不在其中，总量受 PREFETCH_LIMIT 限制。
    let lookup_account = account_id.to_string();
    let lookup_folder = folder.to_string();
    let candidates = with_accounts(state, move |service| {
        service
            .store()
            .uncached_messages(
                &lookup_account,
                &lookup_folder,
                uid_validity,
                PREFETCH_LIMIT,
            )
            .map_err(|e| e.to_string())
    })
    .await?
    .into_iter()
    .filter(|(_, size)| *size <= PREFETCH_MAX_MESSAGE_BYTES)
    .collect::<Vec<_>>();
    let mut remaining = candidates.as_slice();
    while !remaining.is_empty() {
        let mut bytes = 0u64;
        let mut count = 0;
        for (_, size) in remaining {
            if count >= PREFETCH_BATCH
                || (count > 0 && bytes + u64::from(*size) > PREFETCH_BATCH_BYTES)
            {
                break;
            }
            bytes += u64::from(*size);
            count += 1;
        }
        let batch = remaining[..count]
            .iter()
            .map(|(uid, _)| *uid)
            .collect::<Vec<_>>();
        remaining = &remaining[count..];
        // 每批单独获取连接许可，用户点击邮件时最多等待一批下载完成。
        let bodies = {
            let (account, credentials, _permit) = connection(state, account_id.to_string()).await?;
            sync::fetch_messages(
                &state.imap,
                &account,
                &credentials,
                folder,
                uid_validity,
                &batch,
            )
            .await?
        };
        for (uid, raw) in bodies {
            let summary_account = account_id.to_string();
            let summary_folder = folder.to_string();
            let Some(summary) = with_accounts(state, move |service| {
                service
                    .store()
                    .get_message_summary(&summary_account, &summary_folder, uid, uid_validity)
                    .map_err(|e| e.to_string())
            })
            .await?
            else {
                continue;
            };
            let Ok((raw, detail)) = parse(raw, summary).await else {
                continue;
            };
            // 邮件在预取期间被删除或文件夹已换代时跳过即可。
            let _ = store_detail(
                state,
                account_id.to_string(),
                folder.to_string(),
                uid,
                uid_validity,
                raw,
                detail,
            )
            .await;
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn set_message_read(
    account_id: String,
    folder: String,
    uid: u32,
    uid_validity: u32,
    read: bool,
    state: tauri::State<'_, AccountsState>,
) -> Result<(), String> {
    let (account, credentials, _permit) = connection(&state, account_id).await?;
    sync::set_message_read(
        &state.imap,
        &account,
        &credentials,
        &folder,
        uid,
        uid_validity,
        read,
    )
    .await?;
    with_accounts(&state, move |service| {
        ensure_current(service, &account)?;
        service
            .store()
            .set_message_read(&account.id, &folder, uid, uid_validity, read)
            .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub async fn save_attachment(
    account_id: String,
    folder: String,
    uid: u32,
    uid_validity: u32,
    index: usize,
    app: tauri::AppHandle,
    state: tauri::State<'_, AccountsState>,
) -> Result<Option<String>, String> {
    let (_, raw) = message_data(&state, account_id, folder, uid, uid_validity).await?;
    tauri::async_runtime::spawn_blocking(move || {
        let (info, bytes) = mail::attachment_bytes(&raw, index)?;
        // 保存位置只来自本地文件选择器，不接受邮件内容或前端传入的任意目标路径。
        let Some(selected) = app
            .dialog()
            .file()
            .set_file_name(info.name)
            .blocking_save_file()
        else {
            return Ok(None);
        };
        let path = selected
            .into_path()
            .map_err(|_| "请选择本机文件保存位置。".to_string())?;
        std::fs::write(&path, bytes)
            .map_err(|_| "附件保存失败，请检查目标文件的写入权限。".to_string())?;
        Ok(Some(path.to_string_lossy().into_owned()))
    })
    .await
    .map_err(|_| "附件保存未完成，请重试。".to_string())?
}

fn external_email_url(value: &str) -> Result<String, String> {
    if value.len() > 8192 || value.chars().any(char::is_control) {
        return Err("邮件链接过长或格式无效。".into());
    }
    let url = url::Url::parse(value).map_err(|_| "邮件链接格式无效。".to_string())?;
    match url.scheme() {
        "http" | "https" if url.host_str().is_some() => Ok(url.into()),
        "mailto" if !url.path().is_empty() || url.query().is_some() => Ok(url.into()),
        _ => Err("仅支持打开网页链接和邮件地址。".into()),
    }
}

/// 正文中的链接必须经白名单校验，并由系统浏览器/邮件应用打开。
#[tauri::command]
pub async fn open_email_link(url: String, app: tauri::AppHandle) -> Result<(), String> {
    let target = external_email_url(&url)?;
    tauri::async_runtime::spawn_blocking(move || {
        app.opener()
            .open_url(target, None::<&str>)
            .map_err(|_| "无法打开链接，请检查系统默认浏览器或邮件应用。".to_string())
    })
    .await
    .map_err(|_| "链接打开操作未能完成。".to_string())?
}

#[tauri::command]
pub async fn get_draft(
    account_id: String,
    state: tauri::State<'_, AccountsState>,
) -> Result<Option<ComposeMessage>, String> {
    with_accounts(&state, move |service| {
        service.get_account(&account_id)?;
        service
            .store()
            .get_draft(&account_id)
            .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub async fn save_draft(
    account_id: String,
    draft: ComposeMessage,
    state: tauri::State<'_, AccountsState>,
) -> Result<(), String> {
    with_accounts(&state, move |service| {
        service.get_account(&account_id)?;
        service
            .store()
            .save_draft(&account_id, &draft)
            .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub async fn clear_draft(
    account_id: String,
    state: tauri::State<'_, AccountsState>,
) -> Result<(), String> {
    with_accounts(&state, move |service| {
        service.get_account(&account_id)?;
        service
            .store()
            .clear_draft(&account_id)
            .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub async fn send_message(
    account_id: String,
    message: ComposeMessage,
    state: tauri::State<'_, AccountsState>,
) -> Result<SendResult, String> {
    let (account, credentials, _permit) = connection(&state, account_id).await?;
    let mut result = sync::send_message(&account, &credentials, &message).await?;
    // SMTP 已接受邮件后，本地清理失败不能转成“发送失败”，以免用户重复发送。
    let cleanup = with_accounts(&state, move |service| {
        ensure_current(service, &account)?;
        service
            .store()
            .clear_draft_if_matches(&account.id, &message)
            .map_err(|e| e.to_string())
    })
    .await;
    if cleanup.is_err() {
        let note = "邮件已发送，但本地草稿未能清理；请勿重复发送，可在草稿中手动丢弃。";
        result.warning = Some(match result.warning {
            Some(existing) => format!("{existing} {note}"),
            None => note.into(),
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_links_only_allow_web_and_mail_protocols() {
        for value in [
            "https://example.com/report?q=1#details",
            "http://example.com/image",
            "mailto:reader@example.com?subject=Hello",
            "mailto:?to=reader@example.com",
        ] {
            assert!(external_email_url(value).is_ok());
        }
        for value in [
            "javascript:alert(1)",
            "file:///C:/Windows/System32/cmd.exe",
            "data:text/html,<script>alert(1)</script>",
            "ms-settings:privacy",
            "ftp://example.com/file",
            "//example.com",
            "https://example.com/\r\n",
            "mailto:",
        ] {
            assert!(external_email_url(value).is_err(), "accepted {value}");
        }
    }

    #[tokio::test]
    async fn changed_mailbox_restarts_pagination_instead_of_skipping_new_messages() {
        let mut cursors = Vec::new();
        let (page, cursor) = sync_page(Some(50), Some(7), |before| {
            cursors.push(before);
            std::future::ready(Ok(sync::FolderSync {
                uid_validity: 8,
                messages: vec![],
                all_uids: if before.is_none() {
                    vec![100, 101]
                } else {
                    vec![]
                },
                has_more: false,
            }))
        })
        .await
        .unwrap();
        assert_eq!(cursors, vec![Some(50), None]);
        assert_eq!(cursor, None);
        assert_eq!(page.all_uids, vec![100, 101]);
    }

    #[tokio::test]
    async fn unchanged_mailbox_keeps_cursor_and_uses_one_request() {
        let mut requests = 0;
        let (_, cursor) = sync_page(Some(50), Some(7), |before| {
            requests += 1;
            assert_eq!(before, Some(50));
            std::future::ready(Ok(sync::FolderSync {
                uid_validity: 7,
                messages: vec![],
                all_uids: vec![],
                has_more: false,
            }))
        })
        .await
        .unwrap();
        assert_eq!(requests, 1);
        assert_eq!(cursor, Some(50));
    }
}
