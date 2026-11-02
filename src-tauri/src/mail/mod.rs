//! 邮件领域模型、MIME 解析与发送/草稿输入校验。

mod mime;
mod models;
mod rendering;

pub use mime::{attachment_bytes, parse_message, parse_summary, safe_attachment_name};
pub use models::*;
