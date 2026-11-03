use std::collections::HashSet;

use rusqlite::{OptionalExtension, params};

use super::{Store, StoreError};
use crate::mail::{
    ComposeMessage, MAX_RAW_MESSAGE_BYTES, MESSAGE_PAGE_SIZE, MailFolder, MessageDetail,
    MessagePage, MessageSummary, validate_compose,
};

/// 已解析正文 JSON 的缓存上限；超过时只保存原文，打开时重新解析。
const MAX_DETAIL_CACHE_BYTES: usize = 4 * 1024 * 1024;

impl Store {
    pub fn list_folders(&self, account_id: &str) -> Result<Vec<MailFolder>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT configuration FROM mail_folders WHERE account_id = ?1 ORDER BY rowid",
        )?;
        let rows = statement.query_map([account_id], |row| row.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }

    pub fn replace_folders(
        &self,
        account_id: &str,
        folders: &[MailFolder],
    ) -> Result<(), StoreError> {
        if self.get_account(account_id)?.is_none() {
            return Err(StoreError::NotFound);
        }
        let names = folders
            .iter()
            .map(|folder| folder.name.as_str())
            .collect::<HashSet<_>>();
        let transaction = self.connection.unchecked_transaction()?;
        let previous = {
            let mut statement =
                transaction.prepare("SELECT name FROM mail_folders WHERE account_id = ?1")?;
            statement
                .query_map([account_id], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?
        };
        for folder in folders {
            transaction.execute(
                "INSERT INTO mail_folders (account_id, name, configuration) VALUES (?1, ?2, ?3)
                 ON CONFLICT(account_id, name) DO UPDATE SET configuration = excluded.configuration",
                params![account_id, folder.name, serde_json::to_string(folder)?],
            )?;
        }
        for name in previous {
            if !names.contains(name.as_str()) {
                transaction.execute(
                    "DELETE FROM mail_folders WHERE account_id = ?1 AND name = ?2",
                    params![account_id, name],
                )?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn cached_folder_validity(
        &self,
        account_id: &str,
        folder: &str,
    ) -> Result<Option<u32>, StoreError> {
        Ok(self
            .connection
            .query_row(
                "SELECT uid_validity FROM mail_folders WHERE account_id = ?1 AND name = ?2",
                params![account_id, folder],
                |row| row.get::<_, Option<u32>>(0),
            )
            .optional()?
            .flatten())
    }

    pub fn cache_folder_page(
        &self,
        account_id: &str,
        folder: &str,
        uid_validity: u32,
        summaries: &[MessageSummary],
        all_uids: Option<&[u32]>,
    ) -> Result<(), StoreError> {
        if uid_validity == 0
            || summaries
                .iter()
                .any(|summary| summary.uid_validity != uid_validity || summary.uid == 0)
            || all_uids.is_some_and(|uids| uids.contains(&0))
        {
            return Err(StoreError::InvalidInput(
                "服务器返回的邮件 UID 或 UIDVALIDITY 无效。".into(),
            ));
        }
        let transaction = self.connection.unchecked_transaction()?;
        let previous: Option<u32> = transaction
            .query_row(
                "SELECT uid_validity FROM mail_folders WHERE account_id = ?1 AND name = ?2",
                params![account_id, folder],
                |row| row.get::<_, Option<u32>>(0),
            )
            .optional()?
            .flatten();
        let placeholder = MailFolder {
            name: folder.into(),
            display_name: folder.into(),
            selectable: true,
            kind: if folder.eq_ignore_ascii_case("INBOX") {
                "inbox"
            } else {
                "other"
            }
            .into(),
        };
        transaction.execute("INSERT INTO mail_folders(account_id, name, configuration) VALUES(?1,?2,?3) ON CONFLICT(account_id,name) DO NOTHING", params![account_id, folder, serde_json::to_string(&placeholder)?])?;
        if previous != Some(uid_validity) {
            transaction.execute(
                "DELETE FROM messages WHERE account_id = ?1 AND folder = ?2",
                params![account_id, folder],
            )?;
            transaction.execute(
                "UPDATE mail_folders SET remote_min_uid = NULL WHERE account_id = ?1 AND name = ?2",
                params![account_id, folder],
            )?;
        }
        transaction.execute(
            "UPDATE mail_folders SET uid_validity = ?3 WHERE account_id = ?1 AND name = ?2",
            params![account_id, folder, uid_validity],
        )?;
        let live = all_uids.map(|uids| uids.iter().copied().collect::<HashSet<_>>());
        if let (Some(uids), Some(live)) = (all_uids, &live) {
            let cached = {
                let mut statement = transaction
                    .prepare("SELECT uid FROM messages WHERE account_id = ?1 AND folder = ?2")?;
                statement
                    .query_map(params![account_id, folder], |row| row.get::<_, u32>(0))?
                    .collect::<Result<Vec<_>, _>>()?
            };
            let mut delete = transaction.prepare_cached(
                "DELETE FROM messages WHERE account_id = ?1 AND folder = ?2 AND uid = ?3",
            )?;
            for uid in cached {
                if !live.contains(&uid) {
                    delete.execute(params![account_id, folder, uid])?;
                }
            }
            transaction.execute(
                "UPDATE mail_folders SET remote_min_uid = ?3 WHERE account_id = ?1 AND name = ?2",
                params![account_id, folder, uids.iter().min().copied()],
            )?;
        }
        {
            // 更新 FLAGS 时保留已下载正文生成的摘要与准确附件状态。
            let mut detailed = transaction.prepare_cached(
                "SELECT summary FROM messages WHERE account_id = ?1 AND folder = ?2 AND uid_validity = ?3 AND uid = ?4 AND raw IS NOT NULL",
            )?;
            let mut upsert = transaction.prepare_cached(
                "INSERT INTO messages(account_id,folder,uid_validity,uid,summary) VALUES(?1,?2,?3,?4,?5)
                ON CONFLICT(account_id,folder,uid_validity,uid) DO UPDATE SET summary = excluded.summary",
            )?;
            for summary in summaries {
                if live
                    .as_ref()
                    .is_some_and(|live| !live.contains(&summary.uid))
                {
                    continue;
                }
                let mut summary = summary.clone();
                let previous: Option<String> = detailed
                    .query_row(
                        params![account_id, folder, uid_validity, summary.uid],
                        |row| row.get(0),
                    )
                    .optional()?;
                if let Some(previous) = previous {
                    let previous: MessageSummary = serde_json::from_str(&previous)?;
                    summary.snippet = previous.snippet;
                    summary.has_attachments = previous.has_attachments;
                }
                upsert.execute(params![
                    account_id,
                    folder,
                    uid_validity,
                    summary.uid,
                    serde_json::to_string(&summary)?
                ])?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn list_messages(
        &self,
        account_id: &str,
        folder: &str,
        before_uid: Option<u32>,
    ) -> Result<MessagePage, StoreError> {
        let Some((validity, remote_min)) = self
            .connection
            .query_row(
                "SELECT uid_validity, remote_min_uid FROM mail_folders WHERE account_id = ?1 AND name = ?2",
                params![account_id, folder],
                |row| Ok((row.get::<_, Option<u32>>(0)?, row.get::<_, Option<u32>>(1)?)),
            )
            .optional()?
            .and_then(|(validity, min)| validity.map(|validity| (validity, min)))
        else {
            return Ok(MessagePage {
                messages: vec![],
                has_more: false,
                uid_validity: None,
            });
        };
        let before = before_uid.map(i64::from).unwrap_or(i64::from(u32::MAX) + 1);
        let mut statement = self.connection.prepare_cached("SELECT summary FROM messages WHERE account_id = ?1 AND folder = ?2 AND uid_validity = ?3 AND uid < ?4 ORDER BY uid DESC LIMIT ?5")?;
        let rows = statement.query_map(
            params![
                account_id,
                folder,
                validity,
                before,
                (MESSAGE_PAGE_SIZE + 1) as i64
            ],
            |row| row.get::<_, String>(0),
        )?;
        let mut messages = rows
            .map(|row| Ok(serde_json::from_str::<MessageSummary>(&row?)?))
            .collect::<Result<Vec<_>, StoreError>>()?;
        let mut has_more = messages.len() > MESSAGE_PAGE_SIZE;
        messages.truncate(MESSAGE_PAGE_SIZE);
        if let Some(remote_min) = remote_min {
            let oldest = messages
                .last()
                .map(|message| i64::from(message.uid))
                .unwrap_or(before);
            has_more |= i64::from(remote_min) < oldest;
        }
        Ok(MessagePage {
            messages,
            has_more,
            uid_validity: Some(validity),
        })
    }

    pub fn get_message_summary(
        &self,
        account_id: &str,
        folder: &str,
        uid: u32,
        uid_validity: u32,
    ) -> Result<Option<MessageSummary>, StoreError> {
        let json: Option<String> = self.connection.prepare_cached("SELECT summary FROM messages WHERE account_id = ?1 AND folder = ?2 AND uid = ?3 AND uid_validity = ?4")?.query_row(params![account_id, folder, uid, uid_validity], |row| row.get(0)).optional()?;
        json.map(|json| serde_json::from_str(&json).map_err(StoreError::from))
            .transpose()
    }

    /// 读取已解析的正文缓存；摘要取当前摘要列（已读 / 星标可能在解析后变化）。
    /// 邮件不存在、正文未下载或解析结果未缓存时返回 `None`。
    pub fn get_message_detail(
        &self,
        account_id: &str,
        folder: &str,
        uid: u32,
        uid_validity: u32,
    ) -> Result<Option<MessageDetail>, StoreError> {
        let row: Option<(String, Option<String>)> = self
            .connection
            .prepare_cached("SELECT summary, detail FROM messages WHERE account_id = ?1 AND folder = ?2 AND uid = ?3 AND uid_validity = ?4")?
            .query_row(params![account_id, folder, uid, uid_validity], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()?;
        let Some((summary, Some(detail))) = row else {
            return Ok(None);
        };
        // 旧版本写入的或结构已变化的缓存视为未命中，由调用方从原文重新解析。
        let Ok(mut detail) = serde_json::from_str::<MessageDetail>(&detail) else {
            return Ok(None);
        };
        detail.summary = serde_json::from_str(&summary)?;
        Ok(Some(detail))
    }

    pub fn get_message_raw(
        &self,
        account_id: &str,
        folder: &str,
        uid: u32,
        uid_validity: u32,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(self.connection.prepare_cached("SELECT raw FROM messages WHERE account_id = ?1 AND folder = ?2 AND uid = ?3 AND uid_validity = ?4")?.query_row(params![account_id, folder, uid, uid_validity], |row| row.get::<_, Option<Vec<u8>>>(0)).optional()?.flatten())
    }

    /// 最新的若干封尚未下载正文的邮件 (uid, 大小)，供后台预取。
    pub fn uncached_messages(
        &self,
        account_id: &str,
        folder: &str,
        uid_validity: u32,
        limit: usize,
    ) -> Result<Vec<(u32, u32)>, StoreError> {
        let mut statement = self.connection.prepare_cached(
            "SELECT uid, summary FROM messages WHERE account_id = ?1 AND folder = ?2 AND uid_validity = ?3 AND raw IS NULL ORDER BY uid DESC LIMIT ?4",
        )?;
        let rows = statement.query_map(
            params![account_id, folder, uid_validity, limit as i64],
            |row| Ok((row.get::<_, u32>(0)?, row.get::<_, String>(1)?)),
        )?;
        rows.map(|row| {
            let (uid, summary) = row?;
            let summary: MessageSummary = serde_json::from_str(&summary)?;
            Ok((uid, summary.size))
        })
        .collect()
    }

    /// 写入正文原文与解析结果。调用方传入已解析的 detail，避免重复解析 MIME。
    /// 摘要只更新正文派生字段（预览与附件标记），保留当前的已读 / 星标状态。
    /// 返回合并后的完整详情；邮件已不在缓存中时返回 `MailNotFound`。
    pub fn cache_message(
        &self,
        account_id: &str,
        folder: &str,
        uid: u32,
        uid_validity: u32,
        raw: &[u8],
        detail: &MessageDetail,
    ) -> Result<MessageDetail, StoreError> {
        if raw.len() > MAX_RAW_MESSAGE_BYTES {
            return Err(StoreError::InvalidInput(
                "邮件超过 20 MB，无法缓存。".into(),
            ));
        }
        let mut summary = self
            .get_message_summary(account_id, folder, uid, uid_validity)?
            .ok_or(StoreError::MailNotFound)?;
        summary.snippet = detail.summary.snippet.clone();
        summary.has_attachments = detail.summary.has_attachments;
        let mut detail = detail.clone();
        detail.summary = summary.clone();
        let detail_json = serde_json::to_string(&detail)?;
        // 解析结果过大（多为内嵌大图）时只缓存原文，打开时再解析，避免数据库体积翻倍。
        let detail_json = (detail_json.len() <= MAX_DETAIL_CACHE_BYTES).then_some(detail_json);
        self.connection.prepare_cached("UPDATE messages SET raw = ?5, summary = ?6, detail = ?7 WHERE account_id = ?1 AND folder = ?2 AND uid = ?3 AND uid_validity = ?4")?.execute(params![account_id, folder, uid, uid_validity, raw, serde_json::to_string(&summary)?, detail_json])?;
        Ok(detail)
    }

    #[cfg(test)]
    pub fn cache_message_raw(
        &self,
        account_id: &str,
        folder: &str,
        uid: u32,
        uid_validity: u32,
        raw: &[u8],
    ) -> Result<(), StoreError> {
        let summary = self
            .get_message_summary(account_id, folder, uid, uid_validity)?
            .ok_or(StoreError::MailNotFound)?;
        let detail = crate::mail::parse_message(raw, summary).map_err(StoreError::InvalidInput)?;
        self.cache_message(account_id, folder, uid, uid_validity, raw, &detail)?;
        Ok(())
    }

    pub fn set_message_read(
        &self,
        account_id: &str,
        folder: &str,
        uid: u32,
        uid_validity: u32,
        read: bool,
    ) -> Result<(), StoreError> {
        let changed =
            self.update_summaries(account_id, folder, uid_validity, &[uid], |summary| {
                summary.unread = !read;
            })?;
        if changed == 0 {
            return Err(StoreError::MailNotFound);
        }
        Ok(())
    }

    /// 在一个事务中修改一组邮件的摘要（已读、星标等），跳过已不在缓存中的邮件。
    /// 返回实际更新的邮件数。
    pub fn update_summaries(
        &self,
        account_id: &str,
        folder: &str,
        uid_validity: u32,
        uids: &[u32],
        change: impl Fn(&mut MessageSummary),
    ) -> Result<usize, StoreError> {
        let transaction = self.connection.unchecked_transaction()?;
        let mut changed = 0;
        {
            let mut select = transaction.prepare_cached(
                "SELECT summary FROM messages WHERE account_id = ?1 AND folder = ?2 AND uid = ?3 AND uid_validity = ?4",
            )?;
            let mut update = transaction.prepare_cached(
                "UPDATE messages SET summary = ?5 WHERE account_id = ?1 AND folder = ?2 AND uid = ?3 AND uid_validity = ?4",
            )?;
            for uid in uids {
                let Some(json) = select
                    .query_row(params![account_id, folder, uid, uid_validity], |row| {
                        row.get::<_, String>(0)
                    })
                    .optional()?
                else {
                    continue;
                };
                let mut summary: MessageSummary = serde_json::from_str(&json)?;
                change(&mut summary);
                update.execute(params![
                    account_id,
                    folder,
                    uid,
                    uid_validity,
                    serde_json::to_string(&summary)?
                ])?;
                changed += 1;
            }
        }
        transaction.commit()?;
        Ok(changed)
    }

    /// 邮件被移出或删除后从本地缓存移除。目标文件夹的新副本会在下次同步时出现
    /// （服务器分配新 UID），这里不猜测新 UID。
    pub fn remove_messages(
        &self,
        account_id: &str,
        folder: &str,
        uid_validity: u32,
        uids: &[u32],
    ) -> Result<(), StoreError> {
        let transaction = self.connection.unchecked_transaction()?;
        {
            let mut delete = transaction.prepare_cached(
                "DELETE FROM messages WHERE account_id = ?1 AND folder = ?2 AND uid_validity = ?3 AND uid = ?4",
            )?;
            for uid in uids {
                delete.execute(params![account_id, folder, uid_validity, uid])?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn purge_account_mail(&self, account_id: &str) -> Result<(), StoreError> {
        self.connection.execute(
            "DELETE FROM mail_folders WHERE account_id = ?1",
            [account_id],
        )?;
        Ok(())
    }

    pub fn get_draft(&self, account_id: &str) -> Result<Option<ComposeMessage>, StoreError> {
        let json: Option<String> = self
            .connection
            .query_row(
                "SELECT configuration FROM drafts WHERE account_id = ?1",
                [account_id],
                |row| row.get(0),
            )
            .optional()?;
        json.map(|json| serde_json::from_str(&json).map_err(StoreError::from))
            .transpose()
    }

    pub fn save_draft(&self, account_id: &str, draft: &ComposeMessage) -> Result<(), StoreError> {
        validate_compose(draft, false).map_err(StoreError::InvalidInput)?;
        self.connection.execute("INSERT INTO drafts(account_id,configuration) VALUES(?1,?2) ON CONFLICT(account_id) DO UPDATE SET configuration = excluded.configuration", params![account_id, serde_json::to_string(draft)?])?;
        Ok(())
    }

    pub fn clear_draft_if_matches(
        &self,
        account_id: &str,
        draft: &ComposeMessage,
    ) -> Result<bool, StoreError> {
        Ok(self.connection.execute(
            "DELETE FROM drafts WHERE account_id = ?1 AND configuration = ?2",
            params![account_id, serde_json::to_string(draft)?],
        )? > 0)
    }

    pub fn clear_draft(&self, account_id: &str) -> Result<(), StoreError> {
        self.connection
            .execute("DELETE FROM drafts WHERE account_id = ?1", [account_id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::{Account, AuthMethod, Encryption, IncomingProtocol, ServerConfig};
    use crate::mail::parse_summary;

    const RAW: &[u8] = b"From: sender@example.com\r\nTo: a@example.com\r\nSubject: cached mail\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nOffline body";

    fn account(id: &str) -> Account {
        Account {
            id: id.into(),
            email: format!("{id}@example.com"),
            display_name: id.into(),
            incoming_username: format!("{id}@example.com"),
            outgoing_username: format!("{id}@example.com"),
            use_same_password: true,
            incoming_protocol: IncomingProtocol::Imap,
            incoming: ServerConfig {
                host: "imap.example.com".into(),
                port: 993,
                encryption: Encryption::Tls,
            },
            outgoing: ServerConfig {
                host: "smtp.example.com".into(),
                port: 465,
                encryption: Encryption::Tls,
            },
            auth: AuthMethod::Password,
        }
    }

    fn store() -> Store {
        let store = Store::in_memory();
        store.insert_account(&account("a")).unwrap();
        store.insert_account(&account("b")).unwrap();
        store
    }

    fn summary(uid: u32, validity: u32) -> MessageSummary {
        parse_summary(RAW, uid, validity, true, false, RAW.len() as u32).unwrap()
    }

    fn folder(name: &str) -> MailFolder {
        MailFolder {
            name: name.into(),
            display_name: name.into(),
            selectable: true,
            kind: "other".into(),
        }
    }

    fn draft() -> ComposeMessage {
        ComposeMessage {
            to: vec!["unfinished@".into()],
            body: "Unsent text".into(),
            ..Default::default()
        }
    }

    #[test]
    fn headers_body_read_state_and_draft_survive_restart_for_offline_use() {
        let directory =
            std::env::temp_dir().join(format!("wispmail-mail-test-{}", uuid::Uuid::new_v4()));
        let path = directory.join("mail.sqlite3");
        {
            let store = Store::open(&path).unwrap();
            store.insert_account(&account("a")).unwrap();
            store.replace_folders("a", &[folder("INBOX")]).unwrap();
            store
                .cache_folder_page("a", "INBOX", 42, &[summary(1, 42)], Some(&[1]))
                .unwrap();
            store.cache_message_raw("a", "INBOX", 1, 42, RAW).unwrap();
            store.set_message_read("a", "INBOX", 1, 42, true).unwrap();
            store.save_draft("a", &draft()).unwrap();
        }
        {
            let store = Store::open(&path).unwrap();
            assert_eq!(store.list_folders("a").unwrap().len(), 1);
            let page = store.list_messages("a", "INBOX", None).unwrap();
            assert!(!page.messages[0].unread);
            assert_eq!(page.messages[0].snippet, "Offline body");
            assert!(!page.has_more);
            assert_eq!(
                store
                    .get_message_raw("a", "INBOX", 1, 42)
                    .unwrap()
                    .as_deref(),
                Some(RAW)
            );
            assert!(store.get_draft("a").unwrap() == Some(draft()));
        }
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn schema_one_migration_keeps_accounts_and_adds_cache_tables() {
        let directory =
            std::env::temp_dir().join(format!("wispmail-migration-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("mail.sqlite3");
        {
            let connection = rusqlite::Connection::open(&path).unwrap();
            connection.execute_batch("CREATE TABLE accounts(id TEXT PRIMARY KEY NOT NULL,email TEXT NOT NULL COLLATE NOCASE UNIQUE,configuration TEXT NOT NULL); PRAGMA user_version=1;").unwrap();
            connection
                .execute(
                    "INSERT INTO accounts VALUES(?1,?2,?3)",
                    params![
                        "a",
                        "a@example.com",
                        serde_json::to_string(&account("a")).unwrap()
                    ],
                )
                .unwrap();
        }
        {
            let store = Store::open(&path).unwrap();
            assert_eq!(store.get_account("a").unwrap(), Some(account("a")));
            assert_eq!(
                store
                    .connection
                    .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
                    .unwrap(),
                3
            );
            store
                .cache_folder_page("a", "INBOX", 1, &[summary(1, 1)], None)
                .unwrap();
            store.save_draft("a", &draft()).unwrap();
        }
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn uidvalidity_change_invalidates_old_bodies_even_when_uid_is_reused() {
        let store = store();
        store
            .cache_folder_page("a", "INBOX", 1, &[summary(1, 1)], Some(&[1]))
            .unwrap();
        store.cache_message_raw("a", "INBOX", 1, 1, RAW).unwrap();
        store
            .cache_folder_page("a", "INBOX", 2, &[summary(1, 2)], Some(&[1]))
            .unwrap();
        assert!(
            store
                .get_message_summary("a", "INBOX", 1, 1)
                .unwrap()
                .is_none()
        );
        assert!(store.get_message_raw("a", "INBOX", 1, 2).unwrap().is_none());
        assert_eq!(store.cached_folder_validity("a", "INBOX").unwrap(), Some(2));
        assert!(store.set_message_read("a", "INBOX", 1, 1, true).is_err());
    }

    #[test]
    fn empty_pages_still_report_current_uidvalidity() {
        let store = store();
        assert_eq!(
            store
                .list_messages("a", "INBOX", None)
                .unwrap()
                .uid_validity,
            None
        );
        store
            .cache_folder_page("a", "INBOX", 12, &[], Some(&[]))
            .unwrap();
        let empty = store.list_messages("a", "INBOX", None).unwrap();
        assert_eq!(empty.uid_validity, Some(12));
        assert!(empty.messages.is_empty());
        store
            .cache_folder_page("a", "INBOX", 13, &[summary(20, 13)], Some(&[20]))
            .unwrap();
        let old_cursor = store.list_messages("a", "INBOX", Some(5)).unwrap();
        assert_eq!(old_cursor.uid_validity, Some(13));
        assert!(old_cursor.messages.is_empty());
    }

    #[test]
    fn full_uid_snapshot_prunes_deleted_messages_and_refresh_preserves_downloaded_body() {
        let store = store();
        store
            .cache_folder_page(
                "a",
                "INBOX",
                1,
                &[summary(1, 1), summary(2, 1)],
                Some(&[1, 2]),
            )
            .unwrap();
        store.cache_message_raw("a", "INBOX", 1, 1, RAW).unwrap();
        let mut refreshed = summary(1, 1);
        refreshed.unread = false;
        refreshed.flagged = true;
        store
            .cache_folder_page("a", "INBOX", 1, &[refreshed], Some(&[1]))
            .unwrap();
        let message = store
            .get_message_summary("a", "INBOX", 1, 1)
            .unwrap()
            .unwrap();
        assert_eq!(message.snippet, "Offline body");
        assert!(!message.unread && message.flagged);
        assert_eq!(
            store
                .get_message_raw("a", "INBOX", 1, 1)
                .unwrap()
                .as_deref(),
            Some(RAW)
        );
        assert!(
            store
                .get_message_summary("a", "INBOX", 2, 1)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn cache_and_draft_are_isolated_and_account_delete_cascades() {
        let store = store();
        for account in ["a", "b"] {
            store
                .cache_folder_page(account, "INBOX", 1, &[summary(1, 1)], Some(&[1]))
                .unwrap();
            store.save_draft(account, &draft()).unwrap();
        }
        store.cache_message_raw("a", "INBOX", 1, 1, RAW).unwrap();
        assert!(store.get_message_raw("b", "INBOX", 1, 1).unwrap().is_none());
        store.delete_account("a").unwrap();
        assert!(store.list_folders("a").unwrap().is_empty());
        assert!(store.get_message_raw("a", "INBOX", 1, 1).unwrap().is_none());
        assert!(store.get_draft("a").unwrap().is_none());
        assert_eq!(
            store
                .list_messages("b", "INBOX", None)
                .unwrap()
                .messages
                .len(),
            1
        );
        assert!(store.get_draft("b").unwrap().is_some());
    }

    #[test]
    fn pagination_uses_uid_cursor_and_remote_snapshot_for_more_uncached_messages() {
        let store = store();
        let uids = (1..=120).collect::<Vec<_>>();
        let first = (71..=120).map(|uid| summary(uid, 1)).collect::<Vec<_>>();
        store
            .cache_folder_page("a", "INBOX", 1, &first, Some(&uids))
            .unwrap();
        let page = store.list_messages("a", "INBOX", None).unwrap();
        assert_eq!(page.messages.len(), 50);
        assert_eq!(page.messages[0].uid, 120);
        assert_eq!(page.messages.last().unwrap().uid, 71);
        assert!(page.has_more);
        let older = (21..=70).map(|uid| summary(uid, 1)).collect::<Vec<_>>();
        store
            .cache_folder_page("a", "INBOX", 1, &older, None)
            .unwrap();
        let page = store.list_messages("a", "INBOX", Some(71)).unwrap();
        assert_eq!(page.messages.first().unwrap().uid, 70);
        assert!(page.has_more);
        let oldest = (1..=20).map(|uid| summary(uid, 1)).collect::<Vec<_>>();
        store
            .cache_folder_page("a", "INBOX", 1, &oldest, None)
            .unwrap();
        let page = store.list_messages("a", "INBOX", Some(21)).unwrap();
        assert_eq!(page.messages.len(), 20);
        assert!(!page.has_more);
    }

    #[test]
    fn removed_folder_cascades_its_messages_but_keeps_remaining_folder_cache() {
        let store = store();
        for folder in ["INBOX", "Old"] {
            store
                .cache_folder_page("a", folder, 1, &[summary(1, 1)], None)
                .unwrap();
        }
        store.replace_folders("a", &[folder("INBOX")]).unwrap();
        assert!(
            store
                .get_message_summary("a", "Old", 1, 1)
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .get_message_summary("a", "INBOX", 1, 1)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn identity_edit_purges_mail_atomically_and_preserves_draft() {
        let store = store();
        store
            .cache_folder_page("a", "INBOX", 1, &[summary(1, 1)], None)
            .unwrap();
        store.save_draft("a", &draft()).unwrap();
        let mut changed = account("a");
        changed.display_name = "New display name".into();
        changed.outgoing.host = "smtp2.example.com".into();
        store.update_account(&changed).unwrap();
        assert!(
            store
                .get_message_summary("a", "INBOX", 1, 1)
                .unwrap()
                .is_some()
        );
        changed.incoming.host = "imap2.example.com".into();
        store.update_account(&changed).unwrap();
        assert!(store.list_folders("a").unwrap().is_empty());
        assert!(store.get_draft("a").unwrap() == Some(draft()));
    }

    #[test]
    fn failed_account_update_rolls_back_cache_purge() {
        let store = store();
        store
            .cache_folder_page("a", "INBOX", 1, &[summary(1, 1)], None)
            .unwrap();
        store.execute_test_batch("CREATE TRIGGER deny_update BEFORE UPDATE ON accounts BEGIN SELECT RAISE(ABORT,'test failure'); END;");
        let mut changed = account("a");
        changed.incoming_username = "other-user".into();
        assert!(store.update_account(&changed).is_err());
        assert_eq!(store.get_account("a").unwrap(), Some(account("a")));
        assert!(
            store
                .get_message_summary("a", "INBOX", 1, 1)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn failed_new_validity_page_rolls_back_old_validity_and_body() {
        let store = store();
        store
            .cache_folder_page("a", "INBOX", 1, &[summary(1, 1)], None)
            .unwrap();
        store.cache_message_raw("a", "INBOX", 1, 1, RAW).unwrap();
        store.execute_test_batch("CREATE TRIGGER deny_message BEFORE INSERT ON messages BEGIN SELECT RAISE(ABORT,'test failure'); END;");
        assert!(
            store
                .cache_folder_page("a", "INBOX", 2, &[summary(1, 2)], None)
                .is_err()
        );
        assert_eq!(store.cached_folder_validity("a", "INBOX").unwrap(), Some(1));
        assert_eq!(
            store
                .get_message_raw("a", "INBOX", 1, 1)
                .unwrap()
                .as_deref(),
            Some(RAW)
        );
    }

    #[test]
    fn batch_flag_update_skips_missing_messages_and_removal_drops_cached_bodies() {
        let store = store();
        store
            .cache_folder_page(
                "a",
                "INBOX",
                1,
                &[summary(1, 1), summary(2, 1), summary(3, 1)],
                Some(&[1, 2, 3]),
            )
            .unwrap();
        store.cache_message_raw("a", "INBOX", 2, 1, RAW).unwrap();
        let changed = store
            .update_summaries("a", "INBOX", 1, &[1, 2, 99], |summary| {
                summary.flagged = true
            })
            .unwrap();
        assert_eq!(changed, 2);
        let flagged = store
            .get_message_detail("a", "INBOX", 2, 1)
            .unwrap()
            .unwrap();
        assert!(
            flagged.summary.flagged,
            "cached detail reflects the new flag"
        );
        assert!(
            !store
                .get_message_summary("a", "INBOX", 3, 1)
                .unwrap()
                .unwrap()
                .flagged
        );
        store.remove_messages("a", "INBOX", 1, &[1, 2]).unwrap();
        assert!(store.get_message_raw("a", "INBOX", 2, 1).unwrap().is_none());
        let page = store.list_messages("a", "INBOX", None).unwrap();
        assert_eq!(
            page.messages.iter().map(|m| m.uid).collect::<Vec<_>>(),
            vec![3]
        );
    }

    #[test]
    fn conditional_draft_clear_preserves_newer_edits_and_invalid_draft_does_not_overwrite() {
        let store = store();
        let original = draft();
        store.save_draft("a", &original).unwrap();
        let mut changed = original.clone();
        changed.body.push_str(" newer");
        store.save_draft("a", &changed).unwrap();
        assert!(!store.clear_draft_if_matches("a", &original).unwrap());
        let mut invalid = changed.clone();
        invalid.subject = "bad\r\nheader".into();
        assert!(store.save_draft("a", &invalid).is_err());
        assert!(store.get_draft("a").unwrap() == Some(changed.clone()));
        assert!(store.clear_draft_if_matches("a", &changed).unwrap());
        assert!(store.get_draft("a").unwrap().is_none());
    }
}
