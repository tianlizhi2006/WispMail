use mail_parser::{Address, Encoding, Message, MessageParser, MessagePart, MimeHeaders, PartType};

use base64::{Engine, engine::general_purpose::STANDARD};

use super::{
    AttachmentInfo, ForwardAttachments, MAX_ATTACHMENT_BYTES, MAX_ATTACHMENT_COUNT,
    MAX_RAW_MESSAGE_BYTES, MessageDetail, MessageSummary, OutgoingAttachment, valid_mime_type,
};

fn parsed(raw: &[u8]) -> Result<Message<'_>, String> {
    if raw.len() > MAX_RAW_MESSAGE_BYTES {
        return Err("邮件超过 20 MB，当前版本无法下载或解析。".into());
    }
    let message = MessageParser::default()
        .parse(raw)
        .ok_or_else(|| "邮件内容为空或 MIME 格式无法识别。".to_string())?;
    if message.parts.is_empty() || message.parts.len() > 2048 {
        return Err("邮件 MIME 结构为空或过于复杂。".into());
    }
    Ok(message)
}

fn header_text(value: &str, max_chars: usize) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(max_chars)
        .collect()
}

fn addresses(value: Option<&Address<'_>>) -> Vec<String> {
    value
        .map(|list| {
            list.iter()
                .filter_map(|address| address.address.as_deref())
                .filter(|address| address.len() <= 512 && !address.chars().any(char::is_control))
                .take(100)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn sender(message: &Message<'_>) -> String {
    let Some(sender) = message.from().and_then(Address::first) else {
        return "未知发件人".into();
    };
    let address = sender
        .address
        .as_deref()
        .map(|value| header_text(value, 512))
        .unwrap_or_default();
    match sender.name.as_deref().filter(|name| !name.is_empty()) {
        Some(name) if !address.is_empty() => format!("{} <{address}>", header_text(name, 256)),
        Some(name) => header_text(name, 256),
        None if !address.is_empty() => address,
        None => "未知发件人".into(),
    }
}

pub fn parse_summary(
    raw_headers: &[u8],
    uid: u32,
    uid_validity: u32,
    unread: bool,
    flagged: bool,
    size: u32,
) -> Result<MessageSummary, String> {
    let message = parsed(raw_headers)?;
    Ok(MessageSummary {
        uid,
        uid_validity,
        subject: message
            .subject()
            .map(|text| header_text(text, 998))
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| "（无主题）".into()),
        from: sender(&message),
        to: addresses(message.to()),
        date: message
            .date()
            .map(|date| date.to_rfc3339())
            .unwrap_or_default(),
        unread,
        flagged,
        size,
        snippet: String::new(),
        // 只有邮件头时 mixed 是提示；下载正文后用实际附件列表修正。
        has_attachments: message.attachment_count() > 0
            || message.is_content_type("multipart", "mixed"),
    })
}

pub fn parse_message(raw: &[u8], mut summary: MessageSummary) -> Result<MessageDetail, String> {
    let message = parsed(raw)?;
    let rendered = super::rendering::render_bodies(raw, &message);
    let attachments = message
        .attachments()
        .enumerate()
        .map(|(index, part)| {
            let mut info = attachment_info(part, index);
            if matches!(part.body, PartType::Text(_) | PartType::Html(_))
                && let Ok(bytes) = decoded_attachment(raw, part)
            {
                info.size = bytes.len();
            }
            info
        })
        .collect::<Vec<_>>();
    summary.has_attachments = !attachments.is_empty();
    summary.snippet = rendered
        .text_body
        .split_whitespace()
        .take(80)
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(160)
        .collect();
    let references = if let Some(list) = message.references().as_text_list() {
        list.iter()
            .map(|id| header_text(id, 998))
            .take(100)
            .collect()
    } else {
        message
            .references()
            .as_text()
            .map(|id| vec![header_text(id, 998)])
            .unwrap_or_default()
    };
    Ok(MessageDetail {
        summary,
        cc: addresses(message.cc()),
        reply_to: addresses(message.reply_to()),
        message_id: message.message_id().map(|id| header_text(id, 998)),
        references,
        text_body: rendered.text_body,
        html_body: rendered.html_body,
        inline_resources: rendered.inline_resources,
        html_base_url: rendered.html_base_url,
        attachments,
    })
}

pub fn safe_attachment_name(name: &str, index: usize) -> String {
    let leaf = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let filtered: String = leaf.chars().filter(|c| !c.is_control() && !matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')).take(160).collect();
    let mut name = filtered.trim_matches([' ', '.']).to_string();
    if name.is_empty() {
        return format!("attachment-{}", index + 1);
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    ) {
        name.insert(0, '_');
    }
    name
}

fn attachment_info(part: &MessagePart<'_>, index: usize) -> AttachmentInfo {
    let mime_type = part
        .content_type()
        .and_then(|content| {
            content
                .c_subtype
                .as_deref()
                .map(|sub| format!("{}/{sub}", content.c_type))
        })
        .unwrap_or_else(|| "application/octet-stream".into());
    AttachmentInfo {
        index,
        name: safe_attachment_name(part.attachment_name().unwrap_or_default(), index),
        mime_type: header_text(&mime_type, 127),
        size: part.len(),
    }
}

pub fn attachment_bytes(raw: &[u8], index: usize) -> Result<(AttachmentInfo, Vec<u8>), String> {
    let message = parsed(raw)?;
    let part = u32::try_from(index)
        .ok()
        .and_then(|index| message.attachment(index))
        .ok_or_else(|| "附件不存在，请刷新邮件后重试。".to_string())?;
    let bytes = decoded_attachment(raw, part)?;
    if bytes.len() > MAX_ATTACHMENT_BYTES {
        return Err("单个附件超过 10 MB，当前版本无法保存。".into());
    }
    let mut info = attachment_info(part, index);
    info.size = bytes.len();
    Ok((info, bytes))
}

/// 按显示顺序取出要转发的附件，直到达到发送限制（数量与总大小）。
/// 放不下或已损坏的附件记入 `skipped`，由界面提示用户。
pub fn forward_attachments(raw: &[u8], indices: &[usize]) -> Result<ForwardAttachments, String> {
    let message = parsed(raw)?;
    let mut result = ForwardAttachments::default();
    let mut total = 0usize;
    let mut seen = std::collections::HashSet::new();
    for &index in indices.iter().take(256) {
        if !seen.insert(index) {
            continue;
        }
        let Some(part) = u32::try_from(index)
            .ok()
            .and_then(|index| message.attachment(index))
        else {
            continue;
        };
        let info = attachment_info(part, index);
        let bytes = match decoded_attachment(raw, part) {
            Ok(bytes)
                if result.attachments.len() < MAX_ATTACHMENT_COUNT
                    && total + bytes.len() <= MAX_ATTACHMENT_BYTES =>
            {
                bytes
            }
            _ => {
                result.skipped.push(info.name);
                continue;
            }
        };
        total += bytes.len();
        let mime_type = if valid_mime_type(&info.mime_type) {
            info.mime_type
        } else {
            "application/octet-stream".into()
        };
        result.attachments.push(OutgoingAttachment {
            name: info.name,
            mime_type,
            data_base64: STANDARD.encode(bytes),
        });
    }
    Ok(result)
}

pub(super) fn decoded_attachment(raw: &[u8], part: &MessagePart<'_>) -> Result<Vec<u8>, String> {
    if part.is_encoding_problem {
        return Err("附件传输编码损坏，无法安全保存原文件。".into());
    }
    let bytes = raw
        .get(part.offset_body as usize..part.offset_end as usize)
        .ok_or_else(|| "附件内容范围无效。".to_string())?;
    // MIME 解析器会把 text/* 附件转成 Unicode；保存文件必须只解传输编码，保留原字符集字节。
    match part.encoding {
        Encoding::None => Some(bytes.to_vec()),
        Encoding::Base64 => mail_parser::decoders::base64::base64_decode(bytes),
        Encoding::QuotedPrintable => {
            mail_parser::decoders::quoted_printable::quoted_printable_decode(bytes)
        }
    }
    .ok_or_else(|| "附件传输编码损坏，无法安全保存原文件。".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::MAX_BODY_BYTES;

    fn detail(raw: &[u8]) -> MessageDetail {
        parse_message(
            raw,
            parse_summary(raw, 7, 42, true, false, raw.len() as u32).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn decodes_utf8_headers_addresses_and_thread_ids() {
        let raw = b"From: =?UTF-8?B?5byg5LiJ?= <sender@example.com>\r\nTo: a@example.com\r\nCc: b@example.com\r\nReply-To: reply@example.com\r\nSubject: =?UTF-8?B?5L2g5aW9?=\r\nMessage-ID: <one@example.com>\r\nReferences: <zero@example.com>\r\nDate: Mon, 2 Nov 2026 10:00:00 +0800\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHello world";
        let message = detail(raw);
        assert_eq!(message.summary.subject, "你好");
        assert_eq!(message.summary.from, "张三 <sender@example.com>");
        assert_eq!(message.reply_to, vec!["reply@example.com"]);
        assert_eq!(message.message_id.as_deref(), Some("one@example.com"));
        assert_eq!(message.references, vec!["zero@example.com"]);
        assert_eq!(message.text_body, "Hello world");
        assert!(message.html_body.is_none());
    }

    #[test]
    fn decodes_gbk_body() {
        let raw = b"Subject: GBK\r\nContent-Type: text/plain; charset=gbk\r\nContent-Transfer-Encoding: base64\r\n\r\nxOO6ww==";
        assert_eq!(detail(raw).text_body.trim(), "你好");
    }

    #[test]
    fn html_only_and_html_only_alternative_have_readable_plain_text() {
        for raw in [
            b"Subject: HTML\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>Only HTML &amp; text</p>".as_slice(),
            b"Subject: HTML\r\nContent-Type: multipart/alternative; boundary=b\r\n\r\n--b\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>Only HTML &amp; text</p>\r\n--b--\r\n".as_slice(),
        ] {
            let message = detail(raw);
            assert!(message.text_body.contains("Only HTML & text"));
            assert!(!message.text_body.contains("<p>"));
            assert!(message.html_body.is_some());
            assert!(!message.summary.snippet.is_empty());
        }
    }

    #[test]
    fn html_above_compose_limit_is_returned_complete_for_reading() {
        let raw = format!(
            "Subject: HTML\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>Readable {}</p>",
            "你".repeat(MAX_BODY_BYTES / 3 + 100)
        );
        let message = detail(raw.as_bytes());
        assert!(message.html_body.as_deref().unwrap().ends_with("</p>"));
        assert!(message.html_body.as_deref().unwrap().len() > MAX_BODY_BYTES);
        assert!(message.text_body.starts_with("Readable "));
        assert_eq!(
            message.text_body.matches('你').count(),
            MAX_BODY_BYTES / 3 + 100
        );
    }

    #[test]
    fn parses_multipart_html_and_decoded_attachment_without_executing_html() {
        let raw = b"Subject: multipart\r\nContent-Type: multipart/mixed; boundary=outer\r\n\r\n--outer\r\nContent-Type: multipart/alternative; boundary=inner\r\n\r\n--inner\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nplain text\r\n--inner\r\nContent-Type: text/html\r\n\r\n<p>html</p><script>bad()</script>\r\n--inner--\r\n--outer\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=\"../report.bin\"\r\nContent-Transfer-Encoding: base64\r\n\r\nAAECAw==\r\n--outer--\r\n";
        let message = detail(raw);
        assert!(message.text_body.contains("plain text"));
        assert!(message.html_body.as_deref().unwrap().contains("<script>"));
        assert_eq!(message.attachments[0].name, "report.bin");
        assert_eq!(attachment_bytes(raw, 0).unwrap().1, vec![0, 1, 2, 3]);
        assert!(attachment_bytes(raw, 1).is_err());
    }

    #[test]
    fn safe_names_remove_paths_devices_and_control_characters() {
        assert_eq!(safe_attachment_name("C:\\temp\\a.txt", 0), "a.txt");
        assert_eq!(safe_attachment_name("../../CON.txt", 0), "_CON.txt");
        assert_eq!(safe_attachment_name("...", 0), "attachment-1");
        assert_eq!(safe_attachment_name("a\u{202e}\0?.txt", 0), "a.txt");
    }

    #[test]
    fn text_attachments_keep_original_charset_bytes_and_decode_filename() {
        let raw = b"Subject: GBK attachment\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nBody\r\n--b\r\nContent-Type: text/plain; charset=gbk\r\nContent-Disposition: attachment; filename*=utf-8''%E6%96%87%E4%BB%B6.txt\r\nContent-Transfer-Encoding: base64\r\n\r\nxOO6ww==\r\n--b--\r\n";
        let (info, bytes) = attachment_bytes(raw, 0).unwrap();
        assert_eq!(info.name, "文件.txt");
        assert_eq!(bytes, vec![0xc4, 0xe3, 0xba, 0xc3]);
        assert_eq!(info.size, 4);
        assert_eq!(detail(raw).attachments[0].size, 4);
    }

    #[test]
    fn malformed_attachment_encoding_is_not_silently_saved_as_a_corrupt_file() {
        let raw = b"Subject: broken\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nBody\r\n--b\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=broken.bin\r\nContent-Transfer-Encoding: base64\r\n\r\n%%%%\r\n--b--\r\n";
        assert!(attachment_bytes(raw, 0).is_err());
        assert!(detail(raw).text_body.contains("Body"));
    }

    #[test]
    fn forwarding_keeps_valid_attachments_and_lists_the_rest_as_skipped() {
        let big = STANDARD.encode(vec![7u8; MAX_ATTACHMENT_BYTES - 2]);
        let raw = format!(
            "Subject: fwd\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nBody\r\n\
             --b\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=a.pdf\r\nContent-Transfer-Encoding: base64\r\n\r\nAAECAw==\r\n\
             --b\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=broken.bin\r\nContent-Transfer-Encoding: base64\r\n\r\n%%%%\r\n\
             --b\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=big.bin\r\nContent-Transfer-Encoding: base64\r\n\r\n{big}\r\n--b--\r\n"
        );
        let result = forward_attachments(raw.as_bytes(), &[0, 0, 1, 2, 99]).unwrap();
        assert_eq!(result.attachments.len(), 1);
        assert_eq!(result.attachments[0].name, "a.pdf");
        assert_eq!(result.attachments[0].mime_type, "application/pdf");
        assert_eq!(result.attachments[0].data_base64, "AAECAw==");
        // 损坏的附件和会让总大小超过 10 MB 的附件都提示给用户，不静默丢弃。
        assert_eq!(result.skipped, vec!["broken.bin", "big.bin"]);
    }

    #[test]
    fn binary_attachment_without_transfer_encoding_excludes_mime_boundary_newline() {
        let raw = b"Content-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=raw.bin\r\nContent-Transfer-Encoding: binary\r\n\r\n\0\x01ABC\r\n--b--\r\n";
        assert_eq!(attachment_bytes(raw, 0).unwrap().1, b"\0\x01ABC");
    }

    #[test]
    fn raw_message_limit_remains_and_accepted_long_utf8_body_is_complete() {
        assert!(parse_summary(b"", 1, 1, false, false, 0).is_err());
        assert!(parse_summary(&vec![0; MAX_RAW_MESSAGE_BYTES + 1], 1, 1, false, false, 0).is_err());
        let text = "你".repeat(MAX_BODY_BYTES);
        let raw = format!("Content-Type: text/plain; charset=utf-8\r\n\r\n{text}");
        assert_eq!(detail(raw.as_bytes()).text_body, text);
    }
}
