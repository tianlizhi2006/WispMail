use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};

pub const MAX_RAW_MESSAGE_BYTES: usize = 20 * 1024 * 1024;
pub const MAX_ATTACHMENT_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_BODY_BYTES: usize = 1024 * 1024;
pub const MESSAGE_PAGE_SIZE: usize = 50;
pub const MAX_ATTACHMENT_COUNT: usize = 20;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MailFolder {
    /// 服务器上的原始名称，用于所有 IMAP 操作。
    pub name: String,
    /// 界面显示的名称：特殊文件夹为统一中文名，其他为层级中的最后一段。
    pub display_name: String,
    pub selectable: bool,
    pub kind: String,
    /// 层级分隔符（如 "/" 或 "."）。
    #[serde(default)]
    pub delimiter: Option<String>,
    /// 显示顺序；旧缓存缺省为 0，保持原有顺序。
    #[serde(default)]
    pub order: u32,
    /// 缩进层级。
    #[serde(default)]
    pub depth: u8,
    /// 服务商内部的系统文件夹或空分组，默认不显示。
    #[serde(default)]
    pub hidden: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MessageSummary {
    pub uid: u32,
    pub uid_validity: u32,
    pub subject: String,
    pub from: String,
    pub to: Vec<String>,
    pub date: String,
    pub unread: bool,
    pub flagged: bool,
    pub size: u32,
    pub snippet: String,
    pub has_attachments: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MessageDetail {
    pub summary: MessageSummary,
    pub cc: Vec<String>,
    pub reply_to: Vec<String>,
    pub message_id: Option<String>,
    pub references: Vec<String>,
    pub text_body: String,
    pub html_body: Option<String>,
    #[serde(default)]
    pub inline_resources: Vec<InlineResource>,
    #[serde(default)]
    pub html_base_url: Option<String>,
    pub attachments: Vec<AttachmentInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InlineResource {
    pub content_id: Option<String>,
    pub content_location: Option<String>,
    pub mime_type: String,
    pub data_base64: String,
    /// 可由正文图片代替、从附件列表省略的原始索引。显式 attachment 始终为 None，
    /// 以保留下载入口；下载本身仍使用 attachments 中的原始 index。
    pub attachment_index: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentInfo {
    pub index: usize,
    pub name: String,
    pub mime_type: String,
    pub size: usize,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OutgoingAttachment {
    pub name: String,
    pub mime_type: String,
    pub data_base64: String,
}

/// 转发时带上的原邮件附件；超出发送限制或无法解码的附件只列出名称，不静默丢弃。
#[derive(Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ForwardAttachments {
    pub attachments: Vec<OutgoingAttachment>,
    pub skipped: Vec<String>,
}

/// 转发的原邮件（同一账户）。草稿只保存位置；发送时后端读取原文，
/// 把原邮件的 HTML 排版和内嵌图片附在用户文字下方。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ForwardSource {
    pub folder: String,
    pub uid: u32,
    pub uid_validity: u32,
    /// 撰写窗口里显示的原邮件说明（主题 · 发件人），不参与发送。
    pub label: String,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ComposeMessage {
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub body: String,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub attachments: Vec<OutgoingAttachment>,
    #[serde(default)]
    pub forward: Option<ForwardSource>,
}

/// 搜索结果：邮件摘要及其所在文件夹。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub folder: String,
    pub summary: MessageSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessagePage {
    pub messages: Vec<MessageSummary>,
    pub has_more: bool,
    pub uid_validity: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendResult {
    pub message_id: String,
    pub warning: Option<String>,
}

/// 发送时附件 MIME 类型必须是简单的 `type/subtype`，避免注入额外的邮件头参数。
pub fn valid_mime_type(value: &str) -> bool {
    value.len() <= 127
        && value.split_once('/').is_some_and(|(top, sub)| {
            !top.is_empty()
                && !sub.is_empty()
                && top
                    .bytes()
                    .chain(sub.bytes())
                    .all(|c| c.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&c))
        })
}

/// 草稿允许未输入完整的地址；两种模式都限制大小并拒绝邮件头注入。
pub fn validate_compose(message: &ComposeMessage, require_recipients: bool) -> Result<(), String> {
    let recipient_count = message.to.len() + message.cc.len() + message.bcc.len();
    if recipient_count > 50 || (require_recipients && recipient_count == 0) {
        return Err("请填写 1–50 个收件人（包括抄送和密送）。".into());
    }
    let mut recipient_bytes = 0usize;
    for address in message.to.iter().chain(&message.cc).chain(&message.bcc) {
        recipient_bytes = recipient_bytes
            .checked_add(address.len())
            .ok_or_else(|| "收件人内容过长。".to_string())?;
        // 编辑器在草稿中保留尚未拆分的一整行地址；发送时才逐个规范化并校验。
        if address.chars().any(char::is_control)
            || (require_recipients && address.len() > 512)
            || (!require_recipients && recipient_bytes > 50 * 512)
        {
            return Err("收件人地址过长或包含控制字符。".into());
        }
        if require_recipients && address.parse::<lettre::message::Mailbox>().is_err() {
            return Err("收件人地址不完整或格式无效，请检查收件人、抄送及密送。".into());
        }
    }
    if message.subject.len() > 998 || message.subject.chars().any(char::is_control) {
        return Err("主题不能包含控制字符，且不能超过 998 字节。".into());
    }
    if message.body.len() > MAX_BODY_BYTES || message.body.contains('\0') {
        return Err("正文不能超过 1 MB，也不能包含空字符。".into());
    }
    if message.references.len() > 100 {
        return Err("邮件引用链过长，请新建邮件后重试。".into());
    }
    for id in message.references.iter().chain(message.in_reply_to.iter()) {
        if id.is_empty() || id.len() > 998 || id.chars().any(char::is_control) {
            return Err("回复邮件标识无效。".into());
        }
    }
    if let Some(source) = &message.forward
        && (source.folder.is_empty()
            || source.folder.len() > 1000
            || source.folder.chars().any(char::is_control)
            || source.uid == 0
            || source.label.len() > 2000
            || source.label.chars().any(char::is_control))
    {
        return Err("转发的原邮件信息无效，请重新打开原邮件后转发。".into());
    }
    if message.attachments.len() > MAX_ATTACHMENT_COUNT {
        return Err(format!("单封邮件最多添加 {MAX_ATTACHMENT_COUNT} 个附件。"));
    }
    let mut decoded_total = 0usize;
    for attachment in &message.attachments {
        if attachment.name.is_empty()
            || attachment.name.chars().count() > 255
            || attachment.name.chars().any(char::is_control)
        {
            return Err("附件名称为空、过长或包含控制字符。".into());
        }
        let valid_mime = valid_mime_type(&attachment.mime_type);
        if !valid_mime {
            return Err("附件类型无效。".into());
        }
        if attachment.data_base64.len() > MAX_ATTACHMENT_BYTES.div_ceil(3) * 4 {
            return Err("附件总大小不能超过 10 MB。".into());
        }
        let data = STANDARD
            .decode(&attachment.data_base64)
            .map_err(|_| "附件内容损坏，请重新选择文件。".to_string())?;
        decoded_total = decoded_total
            .checked_add(data.len())
            .ok_or("附件总大小不能超过 10 MB。")?;
        if decoded_total > MAX_ATTACHMENT_BYTES {
            return Err("附件总大小不能超过 10 MB。".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draft_allows_partial_recipient_but_send_rejects_it() {
        let draft = ComposeMessage {
            to: vec!["unfinished@".into()],
            ..Default::default()
        };
        assert!(validate_compose(&draft, false).is_ok());
        assert!(validate_compose(&draft, true).is_err());
    }

    #[test]
    fn draft_preserves_a_complete_input_row_with_many_recipients() {
        let addresses = (1..=30)
            .map(|index| format!("recipient-{index}@example.com"))
            .collect::<Vec<_>>();
        let row = addresses.join(", ");
        assert!(row.len() > 512);
        let mut message = ComposeMessage {
            to: vec![row],
            ..Default::default()
        };
        assert!(validate_compose(&message, false).is_ok());
        assert!(validate_compose(&message, true).is_err());
        message.to = addresses;
        assert!(validate_compose(&message, true).is_ok());
    }

    #[test]
    fn draft_recipient_limit_applies_to_all_fields_and_still_blocks_header_injection() {
        let mut message = ComposeMessage {
            to: vec!["x".repeat(50 * 512)],
            ..Default::default()
        };
        assert!(validate_compose(&message, false).is_ok());
        message.cc = vec!["x".into()];
        assert!(validate_compose(&message, false).is_err());
        message.to = vec!["first@example.com,\r\nBcc: second@example.com".into()];
        message.cc.clear();
        assert!(validate_compose(&message, false).is_err());
    }

    #[test]
    fn accepts_bcc_only_mail_and_rejects_header_injection() {
        let mut message = ComposeMessage {
            bcc: vec!["user@example.com".into()],
            ..Default::default()
        };
        assert!(validate_compose(&message, true).is_ok());
        message.subject = "Hi\r\nBcc: other@example.com".into();
        assert!(validate_compose(&message, false).is_err());
    }

    #[test]
    fn validates_attachment_data_and_limits_before_saving_drafts() {
        let mut message = ComposeMessage::default();
        message.attachments.push(OutgoingAttachment {
            name: "data.txt".into(),
            mime_type: "text/plain".into(),
            data_base64: "%%%".into(),
        });
        assert!(validate_compose(&message, false).is_err());
        message.attachments[0].data_base64 = STANDARD.encode(b"hello");
        assert!(validate_compose(&message, false).is_ok());
        message.body = "x".repeat(MAX_BODY_BYTES + 1);
        assert!(validate_compose(&message, false).is_err());
    }
}
