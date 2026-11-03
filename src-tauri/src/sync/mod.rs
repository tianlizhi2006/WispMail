//! 邮件服务器通信：加密登录验证、文件夹与邮件读取、整理操作及 SMTP 发送。

mod connection;
mod mailbox;
mod organize;
mod pool;
mod sender;

#[cfg(test)]
mod mail_tests;

pub use connection::{ConnectionCheck, ConnectionReport, test_connection};
pub use mailbox::{FolderSync, fetch_message, fetch_messages, list_folders, sync_folder};
pub use organize::{
    MAX_BATCH, MailFlag, MessageSet, archive_messages, delete_permanently, move_messages,
    set_flags, trash_messages,
};
pub use pool::ImapPool;
pub use sender::send_message;
