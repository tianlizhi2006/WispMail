//! SQLite 账户配置存储。密码和授权码只由 account 层交给系统凭据管理器。

use std::{path::Path, time::Duration};

use rusqlite::{Connection, OptionalExtension, params};

use crate::account::Account;

mod mail;

const SCHEMA_VERSION: i64 = 4;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("无法访问本地数据库：{0}")]
    Database(#[from] rusqlite::Error),
    #[error("账户配置格式无效：{0}")]
    Serialization(#[from] serde_json::Error),
    #[error("无法创建应用数据目录：{0}")]
    Io(#[from] std::io::Error),
    #[error("数据库来自更新版本的 WispMail，请升级应用后再打开。")]
    NewerSchema,
    #[error("账户不存在，可能已被删除。")]
    NotFound,
    #[error("邮件缓存不存在或已失效，请刷新文件夹。")]
    MailNotFound,
    #[error("{0}")]
    InvalidInput(String),
}

/// 写入搜索索引的各列（rowid、发件人、收件人与抄送、主题、正文）；正文最多索引 200 KB。
fn search_columns(row: &str) -> String {
    format!(
        "{row}rowid,
        coalesce(json_extract({row}summary, '$.from'), ''),
        coalesce((SELECT group_concat(value, ', ') FROM json_each({row}summary, '$.to')), '')
            || ' ' || coalesce((SELECT group_concat(value, ', ') FROM json_each({row}detail, '$.cc')), ''),
        coalesce(json_extract({row}summary, '$.subject'), ''),
        substr(coalesce(json_extract({row}detail, '$.textBody'), json_extract({row}summary, '$.snippet'), ''), 1, 200000)"
    )
}

pub struct Store {
    connection: Connection,
}

fn decode_account(json: &str) -> Result<Account, StoreError> {
    let mut account: Account = serde_json::from_str(json)?;
    account.normalize_usernames();
    Ok(account)
}

impl Store {
    /// 打开应用数据目录中的数据库，并事务性地初始化当前版本结构。
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        Self::initialize(Connection::open(path)?)
    }

    fn initialize(mut connection: Connection) -> Result<Self, StoreError> {
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "foreign_keys", true)?;
        // WAL 让写入正文缓存时不阻塞读取，NORMAL 同步在 WAL 下仍保证数据库一致性，
        // 只是断电时可能丢失最后几笔缓存写入（可从服务器重新下载）。
        connection
            .pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get::<_, String>(0))?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(StoreError::NewerSchema);
        }
        if version < 2 {
            let transaction = connection.transaction()?;
            transaction.execute_batch(
                "CREATE TABLE IF NOT EXISTS accounts (
                    id TEXT PRIMARY KEY NOT NULL,
                    email TEXT NOT NULL COLLATE NOCASE UNIQUE,
                    configuration TEXT NOT NULL
                );
                CREATE TABLE IF NOT EXISTS mail_folders (
                    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
                    name TEXT NOT NULL,
                    configuration TEXT NOT NULL,
                    uid_validity INTEGER,
                    remote_uids TEXT,
                    PRIMARY KEY(account_id, name)
                );
                CREATE TABLE IF NOT EXISTS messages (
                    account_id TEXT NOT NULL,
                    folder TEXT NOT NULL,
                    uid_validity INTEGER NOT NULL,
                    uid INTEGER NOT NULL,
                    summary TEXT NOT NULL,
                    raw BLOB,
                    PRIMARY KEY(account_id, folder, uid_validity, uid),
                    FOREIGN KEY(account_id, folder) REFERENCES mail_folders(account_id, name) ON DELETE CASCADE
                );
                CREATE TABLE IF NOT EXISTS drafts (
                    account_id TEXT PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
                    configuration TEXT NOT NULL
                );
                PRAGMA user_version = 2;",
            )?;
            transaction.commit()?;
        }
        if version < 3 {
            // v3：
            // - 用最小远端 UID 代替完整 UID 快照判断"是否还有更早邮件"，
            //   避免每次翻页都反序列化最多十万个 UID；
            // - detail 缓存已解析的正文 JSON，再次打开邮件无需重新解析 MIME。
            let transaction = connection.transaction()?;
            transaction.execute_batch(
                "ALTER TABLE mail_folders ADD COLUMN remote_min_uid INTEGER;
                UPDATE mail_folders
                    SET remote_min_uid = (SELECT MIN(value) FROM json_each(remote_uids))
                    WHERE remote_uids IS NOT NULL;
                UPDATE mail_folders SET remote_uids = NULL;
                ALTER TABLE messages ADD COLUMN detail TEXT;
                PRAGMA user_version = 3;",
            )?;
            transaction.commit()?;
        }
        if version < 4 {
            // v4：本地全文搜索。trigram 分词按连续三个字符建立索引，中文没有空格也能
            // 按任意片段查找；两个字以内的词由查询改用 LIKE 匹配。
            // 索引由触发器维护：缓存的任何写入、删除（含删除账户、文件夹的级联删除）
            // 都会自动同步，正文以已下载并解析的纯文本为准，未下载时使用摘要。
            let transaction = connection.transaction()?;
            transaction.execute_batch(&format!(
                "CREATE VIRTUAL TABLE message_search USING fts5(
                    sender, recipients, subject, body, tokenize = 'trigram'
                );
                CREATE TRIGGER message_search_insert AFTER INSERT ON messages BEGIN
                    INSERT INTO message_search(rowid, sender, recipients, subject, body)
                    VALUES ({new});
                END;
                CREATE TRIGGER message_search_delete AFTER DELETE ON messages BEGIN
                    DELETE FROM message_search WHERE rowid = old.rowid;
                END;
                -- 只在可搜索的内容变化时重建索引；修改已读、星标不触发。
                CREATE TRIGGER message_search_update AFTER UPDATE OF summary, detail ON messages
                WHEN old.detail IS NOT new.detail
                    OR json_extract(old.summary, '$.from') IS NOT json_extract(new.summary, '$.from')
                    OR json_extract(old.summary, '$.to') IS NOT json_extract(new.summary, '$.to')
                    OR json_extract(old.summary, '$.subject') IS NOT json_extract(new.summary, '$.subject')
                    OR json_extract(old.summary, '$.snippet') IS NOT json_extract(new.summary, '$.snippet')
                BEGIN
                    DELETE FROM message_search WHERE rowid = old.rowid;
                    INSERT INTO message_search(rowid, sender, recipients, subject, body)
                    VALUES ({new});
                END;
                INSERT INTO message_search(rowid, sender, recipients, subject, body)
                    SELECT {existing} FROM messages;
                PRAGMA user_version = 4;",
                new = search_columns("new."),
                existing = search_columns(""),
            ))?;
            transaction.commit()?;
        }
        Ok(Self { connection })
    }

    #[cfg(test)]
    pub(crate) fn in_memory() -> Self {
        Self::initialize(Connection::open_in_memory().unwrap()).unwrap()
    }

    pub fn list_accounts(&self) -> Result<Vec<Account>, StoreError> {
        let mut statement = self
            .connection
            .prepare("SELECT configuration FROM accounts ORDER BY rowid")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|row| decode_account(&row?)).collect()
    }

    pub fn get_account(&self, id: &str) -> Result<Option<Account>, StoreError> {
        let json: Option<String> = self
            .connection
            .query_row(
                "SELECT configuration FROM accounts WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        json.map(|value| decode_account(&value)).transpose()
    }

    pub fn email_owner(&self, email: &str) -> Result<Option<String>, StoreError> {
        Ok(self
            .connection
            .query_row(
                "SELECT id FROM accounts WHERE email = ?1 COLLATE NOCASE",
                [email],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn insert_account(&self, account: &Account) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO accounts (id, email, configuration) VALUES (?1, ?2, ?3)",
            params![account.id, account.email, serde_json::to_string(account)?],
        )?;
        Ok(())
    }

    pub fn update_account(&self, account: &Account) -> Result<(), StoreError> {
        let previous = self.get_account(&account.id)?.ok_or(StoreError::NotFound)?;
        let transaction = self.connection.unchecked_transaction()?;
        if previous.email != account.email
            || previous.incoming != account.incoming
            || previous.incoming_protocol != account.incoming_protocol
            || previous.incoming_login() != account.incoming_login()
        {
            transaction.execute(
                "DELETE FROM mail_folders WHERE account_id = ?1",
                [&account.id],
            )?;
        }
        let changed = transaction.execute(
            "UPDATE accounts SET email = ?2, configuration = ?3 WHERE id = ?1",
            params![account.id, account.email, serde_json::to_string(account)?],
        )?;
        if changed == 0 {
            return Err(StoreError::NotFound);
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn delete_account(&self, id: &str) -> Result<(), StoreError> {
        if self
            .connection
            .execute("DELETE FROM accounts WHERE id = ?1", [id])?
            == 0
        {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn execute_test_batch(&self, sql: &str) {
        self.connection.execute_batch(sql).unwrap();
    }
}
