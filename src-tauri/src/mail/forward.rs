//! 转发正文：用户写的文字在上，原邮件（含 HTML 排版和 CID 内嵌图片）在下。
//! 原邮件只在发送时读取，撰写窗口和草稿里只保存它的位置。

use base64::{Engine, engine::general_purpose::STANDARD};

use super::{MessageDetail, valid_mime_type};

/// 随转发一起发送、通过 `cid:` 被原 HTML 引用的图片。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineImage {
    pub content_id: String,
    pub mime_type: String,
    pub data: Vec<u8>,
}

/// 发送用的转发正文：纯文本版本总是存在；原邮件有 HTML 时再提供 HTML 版本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardBodies {
    pub text: String,
    pub html: Option<String>,
    pub inline: Vec<InlineImage>,
}

const SEPARATOR: &str = "---------- 转发的邮件 ----------";

pub fn forward_bodies(user_text: &str, original: &MessageDetail) -> ForwardBodies {
    let summary = &original.summary;
    let mut header = vec![
        ("发件人", summary.from.clone()),
        ("日期", summary.date.replacen('T', " ", 1)),
        ("主题", summary.subject.clone()),
        ("收件人", summary.to.join(", ")),
    ];
    if !original.cc.is_empty() {
        header.push(("抄送", original.cc.join(", ")));
    }
    let header_text = header
        .iter()
        .map(|(name, value)| format!("{name}：{value}"))
        .collect::<Vec<_>>()
        .join("\n");
    let text = format!(
        "{user_text}\n\n{SEPARATOR}\n{header_text}\n\n{}",
        original.text_body
    );
    let Some(original_html) = original.html_body.as_deref() else {
        return ForwardBodies {
            text,
            html: None,
            inline: Vec::new(),
        };
    };
    let (styles, body) = split_document(original_html);
    let header_html = header
        .iter()
        .map(|(name, value)| format!("{name}：{}", escape(value)))
        .collect::<Vec<_>>()
        .join("<br>");
    let html = format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\">{styles}</head><body>\
         <div style=\"white-space:pre-wrap\">{}</div><br>\
         <div style=\"color:#555;font-size:13px\">{SEPARATOR}<br>{header_html}</div><br>\
         {body}</body></html>",
        escape(user_text)
    );
    let lower = body.to_ascii_lowercase();
    let mut inline: Vec<InlineImage> = Vec::new();
    for resource in &original.inline_resources {
        let Some(content_id) = resource.content_id.as_deref() else {
            continue;
        };
        // 只带上原 HTML 真正引用的图片；Content-ID 必须能安全写入邮件头。
        let referenced = lower.contains(&format!("cid:{}", content_id.to_ascii_lowercase()));
        let header_safe = content_id
            .chars()
            .all(|c| c.is_ascii_graphic() && !matches!(c, '<' | '>' | '"' | '\\'));
        if !referenced || !header_safe || inline.iter().any(|image| image.content_id == content_id)
        {
            continue;
        }
        let Ok(data) = STANDARD.decode(&resource.data_base64) else {
            continue;
        };
        inline.push(InlineImage {
            content_id: content_id.to_string(),
            mime_type: if valid_mime_type(&resource.mime_type) {
                resource.mime_type.clone()
            } else {
                "application/octet-stream".into()
            },
            data,
        });
    }
    ForwardBodies {
        text,
        html: Some(html),
        inline,
    }
}

fn escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

/// 把原邮件的完整 HTML 文档拆成 `<head>` 里的样式和 `<body>` 内容，便于嵌入新文档。
/// 同时去掉脚本：收件人的客户端本来也不会执行，转发时不再携带。
fn split_document(html: &str) -> (String, String) {
    let html = remove_elements(html, "script");
    let lower = html.to_ascii_lowercase();
    // 匹配的都是 ASCII 标记，小写副本与原文的字节位置一致，切片总在字符边界上。
    let body_start = lower
        .find("<body")
        .and_then(|start| lower[start..].find('>').map(|offset| start + offset + 1));
    let Some(body_start) = body_start else {
        return (String::new(), html);
    };
    let body_end = lower
        .rfind("</body")
        .filter(|end| *end >= body_start)
        .unwrap_or(html.len());
    let mut styles = String::new();
    let head = &lower[..body_start];
    let mut cursor = 0;
    while let Some(start) = head[cursor..].find("<style").map(|offset| cursor + offset) {
        let Some(end) = head[start..]
            .find("</style>")
            .map(|offset| start + offset + "</style>".len())
        else {
            break;
        };
        styles.push_str(&html[start..end]);
        cursor = end;
    }
    (styles, html[body_start..body_end].to_string())
}

fn remove_elements(html: &str, tag: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut result = String::with_capacity(html.len());
    let mut cursor = 0;
    while let Some(start) = lower[cursor..].find(&open).map(|offset| cursor + offset) {
        result.push_str(&html[cursor..start]);
        cursor = lower[start..]
            .find(&close)
            .map(|offset| start + offset + close.len())
            .unwrap_or(html.len());
    }
    result.push_str(&html[cursor..]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::{InlineResource, MessageSummary};

    fn original(html: Option<&str>) -> MessageDetail {
        MessageDetail {
            summary: MessageSummary {
                uid: 1,
                uid_validity: 1,
                from: "Sender <s@example.com>".into(),
                to: vec!["a@example.com".into()],
                subject: "Report <Q3>".into(),
                snippet: String::new(),
                date: "2026-11-02T14:30:00+08:00".into(),
                unread: false,
                flagged: false,
                has_attachments: false,
                size: 1,
            },
            cc: vec![],
            reply_to: vec![],
            message_id: None,
            references: vec![],
            text_body: "Original text".into(),
            html_body: html.map(String::from),
            inline_resources: vec![
                InlineResource {
                    content_id: Some("logo@x".into()),
                    content_location: None,
                    mime_type: "image/png".into(),
                    data_base64: STANDARD.encode(b"png"),
                    attachment_index: Some(0),
                },
                InlineResource {
                    content_id: Some("unused@x".into()),
                    content_location: None,
                    mime_type: "image/png".into(),
                    data_base64: STANDARD.encode(b"unused"),
                    attachment_index: Some(1),
                },
            ],
            html_base_url: None,
            attachments: vec![],
        }
    }

    #[test]
    fn html_original_keeps_layout_styles_and_referenced_inline_images() {
        let html = "<html><head><style>.c{color:red}</style><script>bad()</script></head>\
                    <BODY class=x><p class=c>Hi</p><img src=\"CID:logo@x\"><script>evil()</script></BODY></html>";
        let bodies = forward_bodies("FYI <see below>", &original(Some(html)));
        let out = bodies.html.unwrap();
        assert!(out.contains("<style>.c{color:red}</style>"));
        assert!(out.contains("<p class=c>Hi</p><img src=\"CID:logo@x\">"));
        assert!(out.contains("FYI &lt;see below&gt;"));
        assert!(out.contains("主题：Report &lt;Q3&gt;"));
        assert!(out.contains("日期：2026-11-02 14:30:00+08:00"));
        assert!(!out.contains("bad()") && !out.contains("evil()"));
        assert_eq!(
            bodies.inline,
            vec![InlineImage {
                content_id: "logo@x".into(),
                mime_type: "image/png".into(),
                data: b"png".to_vec(),
            }]
        );
        assert!(
            bodies
                .text
                .starts_with("FYI <see below>\n\n---------- 转发的邮件")
        );
        assert!(bodies.text.ends_with("Original text"));
    }

    #[test]
    fn plain_text_original_and_html_fragment_are_supported() {
        let plain = forward_bodies("note", &original(None));
        assert!(plain.html.is_none() && plain.inline.is_empty());
        assert!(plain.text.contains("发件人：Sender <s@example.com>"));
        let fragment = forward_bodies("note", &original(Some("<p>Only a fragment</p>")));
        assert!(
            fragment
                .html
                .unwrap()
                .contains("<p>Only a fragment</p></body>")
        );
    }
}
