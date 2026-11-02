//! 按 MIME 树选择正文及其关联资源，不使用附件列表推测 HTML 的资源作用域。

use std::collections::HashSet;

use base64::{Engine, engine::general_purpose::STANDARD};
use mail_parser::{HeaderName, Message, MessagePart, MimeHeaders, PartType};
use url::Url;

use super::{InlineResource, mime::decoded_attachment};

pub(super) struct RenderedBodies {
    pub text_body: String,
    pub html_body: Option<String>,
    pub inline_resources: Vec<InlineResource>,
    pub html_base_url: Option<String>,
}

pub(super) fn render_bodies(raw: &[u8], message: &Message<'_>) -> RenderedBodies {
    let html_id = body_part(message, 0, true);
    let html_body = html_id.and_then(|id| match &message.parts[id].body {
        PartType::Html(html) => Some(html.to_string()),
        _ => None,
    });
    let mut text_body = text_body(message);
    if text_body.trim().is_empty()
        && let Some(html) = &html_body
    {
        text_body = mail_parser::decoders::html::html_to_text(html);
    }
    let parents = parent_map(message);
    let html_base_url = html_id
        .and_then(|id| part_urls(message, &parents, id).0)
        .map(String::from);
    let inline_resources = html_id
        .map(|id| inline_resources(raw, message, &parents, id))
        .unwrap_or_default();
    RenderedBodies {
        text_body,
        html_body,
        inline_resources,
        html_base_url,
    }
}

fn is_attachment(part: &MessagePart<'_>) -> bool {
    part.content_disposition()
        .is_some_and(|disposition| disposition.c_type.eq_ignore_ascii_case("attachment"))
}

fn content_id(part: &MessagePart<'_>) -> Option<String> {
    clean_identifier(part.content_id()?)
}

fn clean_identifier(value: &str) -> Option<String> {
    let value = value
        .trim()
        .strip_prefix('<')
        .and_then(|value| value.strip_suffix('>'))
        .unwrap_or(value.trim())
        .trim();
    (!value.is_empty() && value.len() <= 4096 && !value.chars().any(char::is_control))
        .then(|| value.to_string())
}

fn related_root(message: &Message<'_>, part: &MessagePart<'_>, children: &[u32]) -> Option<usize> {
    let start = part
        .content_type()
        .and_then(|content| content.attribute("start"));
    let Some(start) = start else {
        return children.first().map(|id| *id as usize);
    };
    let start = clean_identifier(start)?;
    let candidates = children
        .iter()
        .filter_map(|id| {
            let index = *id as usize;
            Some((index, content_id(message.parts.get(index)?)?))
        })
        .collect::<Vec<_>>();
    if let Some((index, _)) = candidates.iter().find(|(_, id)| *id == start) {
        return Some(*index);
    }
    // 常见邮件生成器会改变 Content-ID 大小写；仅在匹配唯一时兼容。
    let mut matches = candidates
        .iter()
        .filter(|(_, id)| id.eq_ignore_ascii_case(&start));
    let index = matches.next()?.0;
    matches.next().is_none().then_some(index)
}

/// HTML 选择遵循 related 的 start/首部件、alternative 的最后一个可显示版本。
/// 普通 mixed 从前向后找正文，转发 message/rfc822 与显式附件不会成为正文。
fn body_part(message: &Message<'_>, root: usize, html: bool) -> Option<usize> {
    let mut pending = vec![(root, false)];
    let mut visited = HashSet::new();
    while let Some((id, related_root_part)) = pending.pop() {
        let Some(part) = message.parts.get(id) else {
            continue;
        };
        if !visited.insert(id) || (is_attachment(part) && !related_root_part) {
            continue;
        }
        match &part.body {
            PartType::Html(_) if html => return Some(id),
            PartType::Text(_) if !html => return Some(id),
            PartType::Multipart(children) => {
                if part.is_content_type("multipart", "related") {
                    if let Some(root) = related_root(message, part, children) {
                        pending.push((root, true));
                    }
                } else if part.is_content_type("multipart", "alternative") {
                    pending.extend(children.iter().map(|id| (*id as usize, false)));
                } else {
                    pending.extend(children.iter().rev().map(|id| (*id as usize, false)));
                }
            }
            _ => {}
        }
    }
    None
}

fn text_body(message: &Message<'_>) -> String {
    let mut pieces = Vec::new();
    let mut pending = vec![(0usize, false)];
    let mut visited = HashSet::new();
    while let Some((id, related_root_part)) = pending.pop() {
        let Some(part) = message.parts.get(id) else {
            continue;
        };
        if !visited.insert(id) || (is_attachment(part) && !related_root_part) {
            continue;
        }
        match &part.body {
            PartType::Text(text) => pieces.push(text.to_string()),
            PartType::Html(html) => pieces.push(mail_parser::decoders::html::html_to_text(html)),
            PartType::Multipart(children) => {
                if part.is_content_type("multipart", "related") {
                    if let Some(root) = related_root(message, part, children) {
                        pending.push((root, true));
                    }
                } else if part.is_content_type("multipart", "alternative") {
                    if let Some(body) =
                        body_part(message, id, false).or_else(|| body_part(message, id, true))
                    {
                        pending.push((body, true));
                    }
                } else {
                    pending.extend(children.iter().rev().map(|id| (*id as usize, false)));
                }
            }
            _ => {}
        }
    }
    pieces.join("\n\n")
}

fn parent_map(message: &Message<'_>) -> Vec<Option<usize>> {
    let mut parents = vec![None; message.parts.len()];
    for (id, part) in message.parts.iter().enumerate() {
        if let PartType::Multipart(children) = &part.body {
            for child in children {
                if let Some(parent) = parents.get_mut(*child as usize) {
                    *parent = Some(id);
                }
            }
        }
    }
    parents
}

fn ancestors(parents: &[Option<usize>], id: usize) -> Vec<usize> {
    let mut result = vec![id];
    let mut current = id;
    while let Some(Some(parent)) = parents.get(current) {
        if result.contains(parent) {
            break;
        }
        result.push(*parent);
        current = *parent;
    }
    result
}

fn clean_location(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && value.len() <= 8192 && !value.chars().any(char::is_control))
        .then(|| value.to_string())
}

fn base_header(part: &MessagePart<'_>) -> Option<String> {
    part.headers().iter().find_map(|header| match &header.name {
        HeaderName::Other(name) if name.eq_ignore_ascii_case("Content-Base") => {
            header.value.as_text().and_then(clean_location)
        }
        _ => None,
    })
}

fn resolve_url(base: Option<&Url>, value: &str) -> Option<Url> {
    let url = Url::parse(value)
        .ok()
        .or_else(|| base.and_then(|base| base.join(value).ok()))?;
    matches!(url.scheme(), "http" | "https").then_some(url)
}

/// 返回正文的有效 base 和当前部件的规范化 Content-Location。
fn part_urls(
    message: &Message<'_>,
    parents: &[Option<usize>],
    id: usize,
) -> (Option<Url>, Option<String>) {
    let mut base = None;
    let mut leaf_location = None;
    for current in ancestors(parents, id).into_iter().rev() {
        let part = &message.parts[current];
        let explicit_base = base_header(part).and_then(|value| resolve_url(base.as_ref(), &value));
        let location = part.content_location().and_then(clean_location);
        let resolved = location
            .as_deref()
            .and_then(|value| resolve_url(explicit_base.as_ref().or(base.as_ref()), value));
        if current == id {
            leaf_location = resolved
                .as_ref()
                .map(|url| url.as_str().to_string())
                .or(location);
        }
        base = explicit_base.or(resolved).or(base);
    }
    (base, leaf_location)
}

fn scope_images(message: &Message<'_>, scope: usize) -> Vec<usize> {
    let mut images = Vec::new();
    let mut pending = vec![scope];
    let mut visited = HashSet::new();
    while let Some(id) = pending.pop() {
        let Some(part) = message.parts.get(id) else {
            continue;
        };
        if !visited.insert(id) {
            continue;
        }
        match &part.body {
            PartType::Multipart(children) => {
                if id != scope
                    && (part.is_content_type("multipart", "related") || is_attachment(part))
                {
                    continue;
                }
                pending.extend(children.iter().rev().map(|id| *id as usize));
            }
            // 不进入独立的转发邮件，也不把文本附件当作图片。
            PartType::Binary(_) | PartType::InlineBinary(_) => images.push(id),
            _ => {}
        }
    }
    images
}

fn inline_resources(
    raw: &[u8],
    message: &Message<'_>,
    parents: &[Option<usize>],
    html_id: usize,
) -> Vec<InlineResource> {
    let chain = ancestors(parents, html_id);
    let mut scopes = chain
        .iter()
        .copied()
        .filter(|id| message.parts[*id].is_content_type("multipart", "related"))
        .collect::<Vec<_>>();
    if scopes.is_empty() {
        scopes.push(*chain.last().unwrap_or(&html_id));
    }
    let mut resources = Vec::new();
    let mut seen_parts = HashSet::new();
    let mut seen_ids = HashSet::new();
    let mut seen_locations = HashSet::new();
    for scope in scopes {
        for id in scope_images(message, scope) {
            if !seen_parts.insert(id) {
                continue;
            }
            let part = &message.parts[id];
            let mut content_id = content_id(part);
            let mut content_location = part_urls(message, parents, id).1;
            if content_id.is_none() && content_location.is_none() {
                continue;
            }
            let Ok(bytes) = decoded_attachment(raw, part) else {
                continue;
            };
            let Some(mime_type) = verified_image_type(part, &bytes) else {
                continue;
            };
            // 内层 related 优先；重复别名不允许覆盖正在显示的正文资源。
            if content_id
                .as_ref()
                .is_some_and(|id| !seen_ids.insert(id.to_ascii_lowercase()))
            {
                content_id = None;
            }
            if content_location
                .as_ref()
                .is_some_and(|location| !seen_locations.insert(location.clone()))
            {
                content_location = None;
            }
            if content_id.is_none() && content_location.is_none() {
                continue;
            }
            resources.push(InlineResource {
                content_id,
                content_location,
                mime_type: mime_type.into(),
                data_base64: STANDARD.encode(bytes),
                attachment_index: (!is_attachment(part))
                    .then(|| {
                        message
                            .attachments
                            .iter()
                            .position(|part| *part as usize == id)
                    })
                    .flatten(),
            });
        }
    }
    resources
}

fn verified_image_type(part: &MessagePart<'_>, bytes: &[u8]) -> Option<&'static str> {
    let content = part.content_type()?;
    if !content.c_type.eq_ignore_ascii_case("image")
        && !part.is_content_type("application", "octet-stream")
    {
        return None;
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else if bytes.starts_with(b"BM") && bytes.len() >= 14 {
        Some("image/bmp")
    } else if bytes.starts_with(b"\0\0\x01\0") && bytes.len() >= 6 {
        Some("image/x-icon")
    } else if bytes.get(4..8) == Some(b"ftyp")
        && bytes.get(8..32.min(bytes.len())).is_some_and(|brands| {
            brands
                .as_chunks::<4>()
                .0
                .iter()
                .any(|brand| brand == b"avif" || brand == b"avis")
        })
    {
        Some("image/avif")
    } else if safe_svg(bytes) {
        Some("image/svg+xml")
    } else {
        None
    }
}

/// SVG 只返回给 img 的 data URL；不将它当成可注入 DOM 的 SVG 文档。
/// 保守拒绝脚本、外部实体及 foreignObject；浏览器的 image 上下文另行禁止活动内容。
fn safe_svg(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let lowercase = text.to_ascii_lowercase();
    if [
        "<script",
        "<foreignobject",
        "<!doctype",
        "<!entity",
        "javascript:",
        "vbscript:",
        "<?xml-stylesheet",
    ]
    .iter()
    .any(|needle| lowercase.contains(needle))
    {
        return false;
    }
    let mut root = text.trim_start_matches('\u{feff}').trim_start();
    loop {
        if root.starts_with("<!--") {
            let Some(end) = root.find("-->") else {
                return false;
            };
            root = root[end + 3..].trim_start();
        } else if root.starts_with("<?xml")
            && root.as_bytes().get(5).is_some_and(u8::is_ascii_whitespace)
        {
            let Some(end) = root.find("?>") else {
                return false;
            };
            root = root[end + 2..].trim_start();
        } else {
            break;
        }
    }
    root.strip_prefix("<svg")
        .and_then(|value| value.as_bytes().first())
        .is_some_and(|byte| byte.is_ascii_whitespace() || matches!(byte, b'>' | b'/'))
}

#[cfg(test)]
mod tests;
