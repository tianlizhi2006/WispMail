use super::*;
use crate::mail::{attachment_bytes, parse_message, parse_summary};

const PNG_BASE64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aNhwAAAAASUVORK5CYII=";

fn detail(raw: &str) -> crate::mail::MessageDetail {
    let summary = parse_summary(raw.as_bytes(), 1, 2, true, false, raw.len() as u32).unwrap();
    parse_message(raw.as_bytes(), summary).unwrap()
}

fn multipart(subtype: &str, boundary: &str, extra_headers: &str, parts: &[String]) -> String {
    let mut result = format!(
        "Content-Type: multipart/{subtype}; boundary=\"{boundary}\"\r\n{extra_headers}\r\n"
    );
    for part in parts {
        result.push_str(&format!("--{boundary}\r\n{part}\r\n"));
    }
    result.push_str(&format!("--{boundary}--\r\n"));
    result
}

fn html(content: &str, extra: &str) -> String {
    format!("Content-Type: text/html; charset=utf-8\r\n{extra}\r\n{content}")
}

fn image(id: &str, extra: &str, bytes: &[u8]) -> String {
    format!(
        "Content-Type: image/png\r\nContent-ID: <{id}>\r\nContent-Transfer-Encoding: base64\r\n{extra}\r\n{}",
        STANDARD.encode(bytes)
    )
}

fn png() -> Vec<u8> {
    STANDARD.decode(PNG_BASE64).unwrap()
}

#[test]
fn related_cid_retains_case_and_percent_bytes_and_original_download_index() {
    let raw = multipart(
        "related",
        "r",
        "",
        &[
            html("<img src=\"CID:Logo%2540ID%40EXAMPLE\">", ""),
            image(
                "Logo%40ID@EXAMPLE",
                "Content-Disposition: inline; filename=logo.png\r\n",
                &png(),
            ),
        ],
    );
    let message = detail(&raw);
    assert_eq!(message.inline_resources.len(), 1);
    let resource = &message.inline_resources[0];
    assert_eq!(resource.content_id.as_deref(), Some("Logo%40ID@EXAMPLE"));
    assert_eq!(resource.mime_type, "image/png");
    assert_eq!(STANDARD.decode(&resource.data_base64).unwrap(), png());
    let index = resource.attachment_index.unwrap();
    assert_eq!(attachment_bytes(raw.as_bytes(), index).unwrap().1, png());
    assert_eq!(message.attachments[index].name, "logo.png");
    assert!(
        message
            .html_body
            .unwrap()
            .contains("CID:Logo%2540ID%40EXAMPLE")
    );
}

#[test]
fn explicitly_attached_cid_image_renders_but_keeps_its_download_entry() {
    let raw = multipart(
        "related",
        "r",
        "",
        &[
            html("<img src=cid:report>", ""),
            image(
                "report",
                "Content-Disposition: attachment; filename=report.png\r\n",
                &png(),
            ),
        ],
    );
    let message = detail(&raw);
    assert_eq!(message.inline_resources.len(), 1);
    assert_eq!(message.inline_resources[0].attachment_index, None);
    assert_eq!(
        message.inline_resources[0].content_id.as_deref(),
        Some("report")
    );
    assert_eq!(message.attachments.len(), 1);
    let attachment = &message.attachments[0];
    assert_eq!(attachment.name, "report.png");
    assert_eq!(
        attachment_bytes(raw.as_bytes(), attachment.index)
            .unwrap()
            .1,
        png()
    );
}

#[test]
fn related_start_selects_actual_root_even_when_another_html_part_comes_first() {
    let mut raw = multipart(
        "related",
        "r",
        "",
        &[
            html("wrong root", "Content-ID: <unused@id>\r\n"),
            html(
                "<p>chosen root</p><img src=cid:logo>",
                "Content-ID: <Body@ID>\r\n",
            ),
            image("logo", "", &png()),
        ],
    );
    raw = raw.replacen("boundary=\"r\"", "boundary=\"r\"; start=\"<body@id>\"", 1);
    let message = detail(&raw);
    assert!(message.html_body.unwrap().contains("chosen root"));
    assert!(!message.text_body.contains("wrong root"));
    assert_eq!(message.inline_resources.len(), 1);
}

#[test]
fn selected_alternative_related_branch_cannot_use_same_cid_from_other_branch() {
    let old = multipart(
        "related",
        "old",
        "",
        &[
            html("old <img src=cid:same>", ""),
            image("same", "", b"\x89PNG\r\n\x1a\nold bytes"),
        ],
    );
    let preferred = multipart(
        "related",
        "new",
        "",
        &[
            html("preferred <img src=cid:same>", ""),
            image("same", "", &png()),
        ],
    );
    let message = detail(&multipart("alternative", "a", "", &[old, preferred]));
    assert!(message.html_body.unwrap().starts_with("preferred"));
    assert_eq!(message.inline_resources.len(), 1);
    assert_eq!(
        STANDARD
            .decode(&message.inline_resources[0].data_base64)
            .unwrap(),
        png()
    );
    assert!(!message.text_body.contains("old"));
}

#[test]
fn nested_related_prefers_nearest_cid_and_uses_outer_resources_without_crossing_siblings() {
    let inner = multipart(
        "related",
        "inner",
        "",
        &[
            html("<img src=cid:same><img src=cid:outer-only>", ""),
            image("same", "", &png()),
        ],
    );
    let unrelated = multipart(
        "related",
        "other",
        "",
        &[
            html("<img src=cid:unrelated>", ""),
            image("unrelated", "", &png()),
        ],
    );
    let raw = multipart(
        "related",
        "outer",
        "",
        &[
            inner,
            image("same", "", b"\x89PNG\r\n\x1a\nouter bytes"),
            image("outer-only", "", &png()),
            unrelated,
        ],
    );
    let resources = detail(&raw).inline_resources;
    assert_eq!(resources.len(), 2);
    assert_eq!(resources[0].content_id.as_deref(), Some("same"));
    assert_eq!(STANDARD.decode(&resources[0].data_base64).unwrap(), png());
    assert_eq!(resources[1].content_id.as_deref(), Some("outer-only"));
}

#[test]
fn content_location_resolves_against_related_base_and_html_location() {
    let raw = multipart(
        "related",
        "r",
        "Content-Base: https://EXAMPLE.test/news/\r\n",
        &[
            html(
                "<img src=images/logo.png>",
                "Content-Location: email.html\r\n",
            ),
            image("logo", "Content-Location: images/logo.png\r\n", &png()),
        ],
    );
    let message = detail(&raw);
    assert_eq!(
        message.html_base_url.as_deref(),
        Some("https://example.test/news/email.html")
    );
    assert_eq!(
        message.inline_resources[0].content_location.as_deref(),
        Some("https://example.test/news/images/logo.png")
    );
}

#[test]
fn html_content_base_takes_precedence_and_relative_locations_work_without_a_base() {
    let raw = multipart(
        "related",
        "r",
        "",
        &[
            html(
                "<img src=logo.png>",
                "Content-Base: https://example.test/assets/\r\nContent-Location: page.html\r\n",
            ),
            image("logo", "Content-Location: logo.png\r\n", &png()),
        ],
    );
    let message = detail(&raw);
    assert_eq!(
        message.html_base_url.as_deref(),
        Some("https://example.test/assets/")
    );
    assert_eq!(
        message.inline_resources[0].content_location.as_deref(),
        Some("logo.png")
    );
}

#[test]
fn quoted_printable_image_uses_original_binary_bytes() {
    let body = png()
        .iter()
        .map(|byte| format!("={byte:02X}"))
        .collect::<String>();
    let picture = format!(
        "Content-Type: image/png\r\nContent-ID: <picture>\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n{body}"
    );
    let raw = multipart(
        "related",
        "r",
        "",
        &[html("<img src=cid:picture>", ""), picture],
    );
    assert_eq!(
        STANDARD
            .decode(&detail(&raw).inline_resources[0].data_base64)
            .unwrap(),
        png()
    );
}

#[test]
fn octet_stream_with_png_magic_is_usable_but_html_disguised_as_png_is_not() {
    let mut binary = image("binary", "", &png());
    binary = binary.replacen("image/png", "application/octet-stream", 1);
    let fake = image("fake", "", b"<html><script>bad()</script></html>");
    let raw = multipart(
        "related",
        "r",
        "",
        &[
            html("<img src=cid:binary><img src=cid:fake>", ""),
            binary,
            fake,
        ],
    );
    let message = detail(&raw);
    assert_eq!(message.inline_resources.len(), 1);
    assert_eq!(
        message.inline_resources[0].content_id.as_deref(),
        Some("binary")
    );
    assert_eq!(message.attachments.len(), 2);
}

#[test]
fn forwarded_message_and_html_attachment_cannot_supply_the_displayed_body_or_images() {
    let forwarded = multipart(
        "related",
        "f",
        "",
        &[
            html("forwarded", ""),
            image("same", "", b"\x89PNG\r\n\x1a\nwrong"),
        ],
    );
    let raw = multipart(
        "mixed",
        "m",
        "",
        &[
            html(
                "attached html",
                "Content-Disposition: attachment; filename=old.html\r\n",
            ),
            format!(
                "Content-Type: message/rfc822\r\nContent-Disposition: attachment; filename=old.eml\r\n\r\n{forwarded}"
            ),
            html("actual body <img src=cid:same>", ""),
            image(
                "same",
                "Content-Disposition: attachment; filename=logo.png\r\n",
                &png(),
            ),
        ],
    );
    let message = detail(&raw);
    assert!(message.html_body.unwrap().starts_with("actual body"));
    assert!(!message.text_body.contains("forwarded"));
    assert!(!message.text_body.contains("attached html"));
    assert_eq!(message.inline_resources.len(), 1);
    assert_eq!(
        STANDARD
            .decode(&message.inline_resources[0].data_base64)
            .unwrap(),
        png()
    );
}

#[test]
fn raster_signatures_are_canonicalized_and_svg_is_a_conservative_image_resource() {
    for (bytes, expected) in [
        (b"\xff\xd8\xff\xe0jpeg".as_slice(), "image/jpeg"),
        (b"GIF89a\x01\0\x01\0".as_slice(), "image/gif"),
        (b"RIFF\x10\0\0\0WEBPVP8 ".as_slice(), "image/webp"),
        (b"BM\0\0\0\0\0\0\0\0\0\0\0\0".as_slice(), "image/bmp"),
        (b"\0\0\x01\0\x01\0".as_slice(), "image/x-icon"),
        (
            b"\0\0\0\x20ftypavif\0\0\0\0avifmif1".as_slice(),
            "image/avif",
        ),
    ] {
        let raw = multipart(
            "related",
            "raster",
            "",
            &[html("<img src=cid:image>", ""), image("image", "", bytes)],
        );
        let resource = detail(&raw).inline_resources.remove(0);
        assert_eq!(resource.mime_type, expected);
        assert_eq!(STANDARD.decode(resource.data_base64).unwrap(), bytes);
    }
    let safe = b"<?xml version=\"1.0\"?><svg xmlns=\"http://www.w3.org/2000/svg\"><rect width=\"10\" height=\"10\"/></svg>";
    let raw = multipart(
        "related",
        "r",
        "",
        &[
            html("<img src=cid:safe><img src=cid:unsafe>", ""),
            image("safe", "", safe).replacen("image/png", "image/svg+xml", 1),
            image("unsafe", "", b"<svg><script>bad()</script></svg>").replacen(
                "image/png",
                "image/svg+xml",
                1,
            ),
        ],
    );
    let message = detail(&raw);
    assert_eq!(message.inline_resources.len(), 1);
    assert_eq!(message.inline_resources[0].mime_type, "image/svg+xml");
    assert_eq!(
        STANDARD
            .decode(&message.inline_resources[0].data_base64)
            .unwrap(),
        safe
    );
    assert_eq!(message.attachments.len(), 2);
    assert!(!safe_svg(b"<html><svg></svg></html>"));
    assert!(!safe_svg(
        b"<!DOCTYPE svg SYSTEM 'https://bad.test/x'><svg/>"
    ));
}

#[test]
fn resource_scan_does_not_depend_on_attachment_enumeration() {
    let raw = multipart(
        "related",
        "r",
        "",
        &[html("<img src=cid:logo>", ""), image("logo", "", &png())],
    );
    let mut message = mail_parser::MessageParser::default()
        .parse(raw.as_bytes())
        .unwrap();
    message.attachments.clear();
    let rendered = render_bodies(raw.as_bytes(), &message);
    assert_eq!(rendered.inline_resources.len(), 1);
    assert_eq!(rendered.inline_resources[0].attachment_index, None);
}
