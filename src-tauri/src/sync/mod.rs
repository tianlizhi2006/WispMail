//! 邮件服务器通信：加密登录验证、文件夹与邮件读取、已读标记及 SMTP 发送。

mod connection;
mod mailbox;
mod pool;
mod sender;

#[cfg(test)]
mod mail_tests;

pub use connection::{ConnectionCheck, ConnectionReport, test_connection};
pub use mailbox::{
    FolderSync, fetch_message, fetch_messages, list_folders, set_message_read, sync_folder,
};
pub use pool::ImapPool;
pub use sender::send_message;
