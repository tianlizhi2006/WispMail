//! SMTP 单次投递；只有确认成功后才归档。归档失败不能触发重新投递。

use std::future::Future;

use base64::{Engine, engine::general_purpose::STANDARD};
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Attachment, Mailbox, MultiPart, SinglePart, header::ContentType},
};
use tokio::time::timeout;

use crate::{
    account::{Account, AccountCredentials},
    mail::{
        ComposeMessage, ForwardBodies, MAX_ATTACHMENT_BYTES, MAX_RAW_MESSAGE_BYTES, SendResult,
        valid_mime_type, validate_compose,
    },
};

use super::{
    connection::{ImapSession, open_imap, smtp_transport},
    mailbox::{OPERATION_TIMEOUT, bounded, finish, list_in_session},
    organize::SpecialFolder,
};

pub async fn send_message(
    account: &Account,
    credentials: &AccountCredentials,
    compose: &ComposeMessage,
    forward: Option<&ForwardBodies>,
) -> Result<SendResult, String> {
    let (message, message_id) = build_message(account, compose, forward)?;
    let raw = message.formatted();
    if raw.len() > MAX_RAW_MESSAGE_BYTES {
        return Err("编码后的邮件超过 20 MiB，请减少附件或正文。".into());
    }
    let transport = smtp_transport(account, credentials)?;
    send_prepared(&transport, &message, &raw, message_id, async {
        let mut session = open_imap(account, credentials).await?;
        let result = archive_in_session(&mut session, &raw).await;
        finish(session, result).await
    })
    .await
}

pub(super) async fn send_prepared(
    transport: &AsyncSmtpTransport<Tokio1Executor>,
    message: &Message,
    raw: &[u8],
    message_id: String,
    archive: impl Future<Output = Result<(), String>>,
) -> Result<SendResult, String> {
    deliver(transport, message, raw).await?;
    // SMTP 已明确成功。此后的任何错误只能作为归档警告返回；绝不再次调用 deliver。
    let archive = bounded(archive).await;
    Ok(SendResult {
        message_id,
        warning: archive
            .err()
            .map(|error| format!("邮件已发送，但未能保存到已发送文件夹：{error} 请勿重复发送。")),
    })
}

pub(super) async fn deliver(
    transport: &AsyncSmtpTransport<Tokio1Executor>,
    message: &Message,
    raw: &[u8],
) -> Result<(), String> {
    // 不重试：连接中断可能发生在服务器已接受 DATA 之后。
    match timeout(OPERATION_TIMEOUT, transport.send_raw(message.envelope(), raw)).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(_)) => Err("SMTP 未确认发送完成，邮件可能已被服务器接受。请先检查服务器的已发送邮件或向收件人确认，再决定是否重试。".into()),
        Err(_) => Err("发送超时，投递结果不确定。请先检查服务器的已发送邮件或向收件人确认，避免重复发送。".into()),
    }
}

pub(super) async fn archive_in_session(
    session: &mut ImapSession,
    raw: &[u8],
) -> Result<(), String> {
    let folders = list_in_session(session).await?;
    let name = if let Some(folder) = SpecialFolder::Sent.find(&folders) {
        folder.name.clone()
    } else {
        session
            .create("Sent")
            .await
            .map_err(|_| "服务器不允许创建 Sent 文件夹。".to_string())?;
        "Sent".to_string()
    };
    session
        .append(name, Some("(\\Seen)"), None, raw)
        .await
        .map_err(|_| "服务器未确认已发送邮件的归档，副本可能已存在。".to_string())
}

/// 正文部件：纯文本，或转发时的 alternative（纯文本 + HTML，HTML 与内嵌图片组成 related）。
enum Body {
    Single(SinglePart),
    Multi(MultiPart),
}

impl Body {
    fn add_to(self, parts: MultiPart) -> MultiPart {
        match self {
            Body::Single(part) => parts.singlepart(part),
            Body::Multi(part) => parts.multipart(part),
        }
    }
}

fn body(compose: &ComposeMessage, forward: Option<&ForwardBodies>) -> Result<Body, String> {
    let Some(forward) = forward else {
        return Ok(Body::Single(SinglePart::plain(compose.body.clone())));
    };
    let text = SinglePart::plain(forward.text.clone());
    let Some(html) = &forward.html else {
        return Ok(Body::Single(text));
    };
    let html = SinglePart::html(html.clone());
    let html = if forward.inline.is_empty() {
        Body::Single(html)
    } else {
        let mut related = MultiPart::related().singlepart(html);
        for image in &forward.inline {
            let mime = if valid_mime_type(&image.mime_type) {
                image.mime_type.as_str()
            } else {
                "application/octet-stream"
            };
            let content_type =
                ContentType::parse(mime).map_err(|_| "内嵌图片类型无效。".to_string())?;
            related = related.singlepart(
                Attachment::new_inline(image.content_id.clone())
                    .body(image.data.clone(), content_type),
            );
        }
        Body::Multi(related)
    };
    Ok(Body::Multi(
        html.add_to(MultiPart::alternative().singlepart(text)),
    ))
}

/// 转发时附上的原邮件部分（用户文字为空）编码后的字节数，供撰写窗口估算邮件大小。
/// 与发送时使用同一套 MIME 构造，结果与实际编码一致。
pub fn forwarded_part_size(forward: &ForwardBodies) -> Result<usize, String> {
    Ok(match body(&ComposeMessage::default(), Some(forward))? {
        Body::Single(part) => part.formatted().len(),
        Body::Multi(part) => part.formatted().len(),
    })
}

pub(super) fn build_message(
    account: &Account,
    compose: &ComposeMessage,
    forward: Option<&ForwardBodies>,
) -> Result<(Message, String), String> {
    validate_compose(compose, true)?;
    if account.email.chars().any(char::is_control)
        || account.display_name.chars().any(char::is_control)
    {
        return Err("发件人信息格式无效。".into());
    }
    let sender = Mailbox::new(
        (!account.display_name.is_empty()).then(|| account.display_name.clone()),
        account
            .email
            .parse()
            .map_err(|_| "发件人邮箱地址无效。".to_string())?,
    );
    let message_id = format!("<{}@wispmail.local>", uuid::Uuid::new_v4());
    let mut builder = Message::builder()
        .from(sender)
        .subject(&compose.subject)
        .message_id(Some(message_id.clone()));
    for recipient in &compose.to {
        builder = builder.to(parse_recipient(recipient)?);
    }
    for recipient in &compose.cc {
        builder = builder.cc(parse_recipient(recipient)?);
    }
    for recipient in &compose.bcc {
        builder = builder.bcc(parse_recipient(recipient)?);
    }
    if let Some(id) = &compose.in_reply_to {
        builder = builder.in_reply_to(normalize_message_id(id)?);
    }
    if !compose.references.is_empty() {
        let references = compose
            .references
            .iter()
            .map(|id| normalize_message_id(id))
            .collect::<Result<Vec<_>, _>>()?;
        builder = builder.references(references.join(" "));
    }
    let body = body(compose, forward)?;
    let message = if compose.attachments.is_empty() {
        match body {
            Body::Single(part) => builder.singlepart(part),
            Body::Multi(part) => builder.multipart(part),
        }
    } else {
        let mut parts = match body {
            Body::Single(part) => MultiPart::mixed().singlepart(part),
            Body::Multi(part) => MultiPart::mixed().multipart(part),
        };
        let mut total = 0usize;
        for attachment in &compose.attachments {
            let bytes = STANDARD
                .decode(&attachment.data_base64)
                .map_err(|_| "附件内容格式无效，请重新添加附件。".to_string())?;
            total = total
                .checked_add(bytes.len())
                .ok_or_else(|| "附件总大小超出限制。".to_string())?;
            if total > MAX_ATTACHMENT_BYTES {
                return Err("附件总大小不能超过 10 MiB。".into());
            }
            let content_type = ContentType::parse(&attachment.mime_type)
                .map_err(|_| "附件 MIME 类型无效。".to_string())?;
            parts = parts
                .singlepart(Attachment::new(attachment.name.clone()).body(bytes, content_type));
        }
        builder.multipart(parts)
    }
    .map_err(|_| "无法生成邮件，请检查收件人和内容。".to_string())?;
    // Lettre 默认从 MIME 中移除 Bcc，但 envelope 保留全部收件人。
    Ok((message, message_id))
}

fn parse_recipient(value: &str) -> Result<Mailbox, String> {
    if value.chars().any(char::is_control) {
        return Err("收件人地址不能包含控制字符。".into());
    }
    value
        .parse()
        .map_err(|_| "收件人、抄送或密送地址无效。".to_string())
}

fn normalize_message_id(value: &str) -> Result<String, String> {
    // MIME parser returns bare IDs; callers may also supply the RFC angle form.
    let inner = value
        .strip_prefix('<')
        .and_then(|id| id.strip_suffix('>'))
        .unwrap_or(value);
    let valid_parts = inner
        .split_once('@')
        .is_some_and(|(left, right)| !left.is_empty() && !right.is_empty() && !right.contains('@'));
    if inner.len() > 996
        || !valid_parts
        || inner.chars().any(|character| {
            !character.is_ascii() || character.is_whitespace() || character.is_control()
        })
        || inner.contains(['<', '>', '(', ')', '"', '\\'])
    {
        return Err("回复邮件的 Message-ID 或 References 格式无效。".into());
    }
    Ok(format!("<{inner}>"))
}
