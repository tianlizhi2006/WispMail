//! 仅使用临时回环服务器，不连接外部邮箱，也不发送真实邮件。

use std::time::Duration;

use base64::{Engine, engine::general_purpose::STANDARD};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    net::TcpListener,
    time::timeout,
};
use tokio_native_tls::{TlsConnector, native_tls};

use crate::{
    account::{Account, AuthMethod, Encryption, IncomingProtocol, ServerConfig},
    mail::{ComposeMessage, OutgoingAttachment},
};

use super::{
    connection::{ImapSession, open_imap_with_connector, smtp_transport_with_parameters},
    mailbox::{fetch_in_session, finish, list_in_session, sync_in_session},
    organize::{
        MailFlag, MessageSet, delete_permanently_in_session, move_in_session, set_flag_in_session,
    },
    sender::{archive_in_session, build_message, send_prepared},
};

async fn set_read_in_session(
    session: &mut ImapSession,
    folder: &str,
    uid: u32,
    uid_validity: u32,
    read: bool,
) -> Result<(), String> {
    let uids = [uid];
    let set = MessageSet {
        folder,
        uid_validity,
        uids: &uids,
    };
    set_flag_in_session(session, set, MailFlag::Seen, read).await
}

const HEADER: &str = "From: Sender <sender@example.com>\r\nTo: reader@example.com\r\nSubject: Fixture mail\r\nDate: Mon, 2 Nov 2026 08:00:00 +0000\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n";

fn account() -> Account {
    Account {
        id: "loopback-test".into(),
        email: "sender@example.com".into(),
        display_name: "Sender".into(),
        incoming_username: "reader-user".into(),
        outgoing_username: "sender-user".into(),
        use_same_password: false,
        incoming_protocol: IncomingProtocol::Imap,
        incoming: ServerConfig {
            host: "127.0.0.1".into(),
            port: 993,
            encryption: Encryption::Tls,
        },
        outgoing: ServerConfig {
            host: "127.0.0.1".into(),
            port: 465,
            encryption: Encryption::Tls,
        },
        auth: AuthMethod::Password,
    }
}

fn acceptor() -> tokio_native_tls::TlsAcceptor {
    native_tls::TlsAcceptor::new(
        native_tls::Identity::from_pkcs8(
            include_bytes!("test-fixtures/server.pem"),
            include_bytes!("test-fixtures/server-key.pem"),
        )
        .unwrap(),
    )
    .unwrap()
    .into()
}

fn connector() -> TlsConnector {
    native_tls::TlsConnector::builder()
        .add_root_certificate(
            native_tls::Certificate::from_pem(include_bytes!("test-fixtures/root.pem")).unwrap(),
        )
        .build()
        .unwrap()
        .into()
}

fn smtp_parameters() -> lettre::transport::smtp::client::TlsParameters {
    use lettre::transport::smtp::client::{Certificate, TlsParameters};
    TlsParameters::builder("127.0.0.1".into())
        .add_root_certificate(
            Certificate::from_pem(include_bytes!("test-fixtures/root.pem")).unwrap(),
        )
        .build_native()
        .unwrap()
}

async fn line<T: AsyncRead + Unpin>(stream: &mut BufReader<T>) -> String {
    let mut line = String::new();
    assert!(
        timeout(Duration::from_secs(5), stream.read_line(&mut line))
            .await
            .unwrap()
            .unwrap()
            > 0
    );
    line
}

async fn write<T: AsyncWrite + Unpin>(stream: &mut T, text: &str) {
    stream.write_all(text.as_bytes()).await.unwrap();
}

type Peer = BufReader<tokio_native_tls::TlsStream<tokio::net::TcpStream>>;

async fn imap_login(listener: TcpListener) -> Peer {
    let (stream, _) = listener.accept().await.unwrap();
    let mut stream = acceptor().accept(stream).await.unwrap();
    write(&mut stream, "* OK loopback fixture\r\n").await;
    let mut peer = BufReader::new(stream);
    let login = line(&mut peer).await;
    assert!(login.ends_with(" LOGIN \"reader-user\" \"incoming-secret\"\r\n"));
    let tag = login.split_whitespace().next().unwrap();
    write(peer.get_mut(), &format!("{tag} OK authenticated\r\n")).await;
    peer
}

async fn logout(peer: &mut Peer) {
    let command = line(peer).await;
    assert!(command.ends_with(" LOGOUT\r\n"));
    let tag = command.split_whitespace().next().unwrap();
    write(
        peer.get_mut(),
        &format!("* BYE closing\r\n{tag} OK logout\r\n"),
    )
    .await;
}

async fn scripted_imap(listener: TcpListener, steps: Vec<(String, String)>) {
    let mut peer = imap_login(listener).await;
    for (expected, response) in steps {
        let command = line(&mut peer).await;
        assert!(
            command.ends_with(&format!(" {expected}\r\n")),
            "unexpected command: {command}"
        );
        let tag = command.split_whitespace().next().unwrap();
        write(peer.get_mut(), &response.replace("{tag}", tag)).await;
    }
    logout(&mut peer).await;
}

async fn session(port: u16) -> ImapSession {
    let mut account = account();
    account.incoming.port = port;
    timeout(
        Duration::from_secs(5),
        open_imap_with_connector(&account, "incoming-secret", &connector()),
    )
    .await
    .unwrap()
    .unwrap()
}

fn selected(exists: usize, validity: u32) -> String {
    format!(
        "* {exists} EXISTS\r\n* OK [UIDVALIDITY {validity}] valid\r\n{{tag}} OK [READ-WRITE] selected\r\n"
    )
}

#[tokio::test]
async fn lists_special_use_and_decodes_names_while_preserving_protocol_name() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(scripted_imap(listener, vec![("LIST \"\" *".into(),
        "* LIST (\\Sent) \"/\" \"&XfJT0ZAB-\"\r\n* LIST (\\Noselect) \"/\" \"Folders\"\r\n* LIST () \"/\" \"INBOX\"\r\n{tag} OK list\r\n".into())]));
    let mut session = session(port).await;
    let folders = list_in_session(&mut session).await.unwrap();
    assert_eq!(folders[0].name, "INBOX");
    let sent = folders.iter().find(|folder| folder.kind == "sent").unwrap();
    assert_eq!(sent.name, "&XfJT0ZAB-");
    assert_eq!(sent.display_name, "已发送");
    assert!(
        !folders
            .iter()
            .find(|folder| folder.name == "Folders")
            .unwrap()
            .selectable
    );
    finish(session, Ok(())).await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn sync_fetches_only_latest_fifty_headers_and_supports_uid_cursor() {
    for (before, first, last, has_more) in [(None, 52, 3, true), (Some(3), 2, 1, false)] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let search = (1..=52)
            .map(|uid| uid.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        let wanted = (last..=first)
            .rev()
            .map(|uid| uid.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let fetch = format!(
            "* 1 FETCH (UID {first} FLAGS (\\Flagged) RFC822.SIZE 200 BODY[HEADER]<0> {{{}}}\r\n{HEADER})\r\n{{tag}} OK fetched\r\n",
            HEADER.len()
        );
        let server = tokio::spawn(scripted_imap(
            listener,
            vec![
                ("SELECT \"INBOX\"".into(), selected(52, 77)),
                (
                    "UID SEARCH ALL".into(),
                    format!("* SEARCH {search}\r\n{{tag}} OK search\r\n"),
                ),
                (
                    format!(
                        "UID FETCH {wanted} (UID FLAGS RFC822.SIZE BODY.PEEK[HEADER]<0.65536>)"
                    ),
                    fetch,
                ),
            ],
        ));
        let mut session = session(port).await;
        let sync = sync_in_session(&mut session, "INBOX", before)
            .await
            .unwrap();
        assert_eq!(sync.uid_validity, 77);
        assert_eq!(sync.all_uids.len(), 52);
        assert_eq!(sync.messages[0].uid, first);
        assert_eq!(sync.messages[0].subject, "Fixture mail");
        assert!(sync.messages[0].unread && sync.messages[0].flagged);
        assert_eq!(sync.has_more, has_more);
        finish(session, Ok(())).await.unwrap();
        server.await.unwrap();
    }
}

#[tokio::test]
async fn body_read_checks_uidvalidity_and_size_then_uses_peek() {
    let raw = format!("{HEADER}fixture body\r\n");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(scripted_imap(
        listener,
        vec![
            ("SELECT \"INBOX\"".into(), selected(1, 77)),
            (
                "UID FETCH 42 (UID RFC822.SIZE)".into(),
                format!(
                    "* 1 FETCH (UID 42 RFC822.SIZE {})\r\n{{tag}} OK size\r\n",
                    raw.len()
                ),
            ),
            (
                "UID FETCH 42 (UID BODY.PEEK[]<0.20971521>)".into(),
                format!(
                    "* 1 FETCH (UID 42 BODY[]<0> {{{}}}\r\n{raw})\r\n{{tag}} OK body\r\n",
                    raw.len()
                ),
            ),
        ],
    ));
    let mut session = session(port).await;
    assert_eq!(
        fetch_in_session(&mut session, "INBOX", 42, 77)
            .await
            .unwrap(),
        raw.as_bytes()
    );
    finish(session, Ok(())).await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn changed_uidvalidity_blocks_read_and_flag_mutation() {
    for mutate in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(scripted_imap(
            listener,
            vec![("SELECT \"INBOX\"".into(), selected(1, 78))],
        ));
        let mut session = session(port).await;
        let error = if mutate {
            set_read_in_session(&mut session, "INBOX", 42, 77, true)
                .await
                .unwrap_err()
        } else {
            fetch_in_session(&mut session, "INBOX", 42, 77)
                .await
                .unwrap_err()
        };
        assert!(error.contains("UIDVALIDITY"));
        finish(session, Ok(())).await.unwrap();
        server.await.unwrap();
    }
}

#[tokio::test]
async fn oversized_message_is_rejected_before_body_download() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(scripted_imap(
        listener,
        vec![
            ("SELECT \"INBOX\"".into(), selected(1, 77)),
            (
                "UID FETCH 42 (UID RFC822.SIZE)".into(),
                "* 1 FETCH (UID 42 RFC822.SIZE 20971521)\r\n{tag} OK size\r\n".into(),
            ),
        ],
    ));
    let mut session = session(port).await;
    assert!(
        fetch_in_session(&mut session, "INBOX", 42, 77)
            .await
            .unwrap_err()
            .contains("20 MiB")
    );
    finish(session, Ok(())).await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn explicit_read_change_stores_seen_and_verifies_server_state() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(scripted_imap(
        listener,
        vec![
            ("SELECT \"INBOX\"".into(), selected(1, 77)),
            (
                "UID STORE 42 +FLAGS.SILENT (\\Seen)".into(),
                "{tag} OK stored\r\n".into(),
            ),
            (
                "UID FETCH 42 (UID FLAGS)".into(),
                "* 1 FETCH (UID 42 FLAGS (\\Seen))\r\n{tag} OK flags\r\n".into(),
            ),
        ],
    ));
    let mut session = session(port).await;
    set_read_in_session(&mut session, "INBOX", 42, 77, true)
        .await
        .unwrap();
    finish(session, Ok(())).await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn batch_star_is_rejected_unless_every_message_confirms_the_flag() {
    for (fetched, ok) in [
        (
            "* 1 FETCH (UID 41 FLAGS (\\Flagged))\r\n* 2 FETCH (UID 42 FLAGS (\\Seen \\Flagged))\r\n",
            true,
        ),
        // 42 已被其他客户端删除：STORE 返回 OK，但确认阶段只能看到 41。
        ("* 1 FETCH (UID 41 FLAGS (\\Flagged))\r\n", false),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(scripted_imap(
            listener,
            vec![
                ("SELECT \"INBOX\"".into(), selected(2, 77)),
                (
                    "UID STORE 41,42 +FLAGS.SILENT (\\Flagged)".into(),
                    "{tag} OK stored\r\n".into(),
                ),
                (
                    "UID FETCH 41,42 (UID FLAGS)".into(),
                    format!("{fetched}{{tag}} OK flags\r\n"),
                ),
            ],
        ));
        let mut session = session(port).await;
        let uids = [41, 42];
        let set = MessageSet {
            folder: "INBOX",
            uid_validity: 77,
            uids: &uids,
        };
        let result = set_flag_in_session(&mut session, set, MailFlag::Flagged, true).await;
        assert_eq!(result.is_ok(), ok, "{result:?}");
        finish(session, Ok(())).await.unwrap();
        server.await.unwrap();
    }
}

fn capability(extra: &str) -> String {
    format!("* CAPABILITY IMAP4rev1{extra}\r\n{{tag}} OK capability\r\n")
}

#[tokio::test]
async fn move_uses_atomic_uid_move_when_supported() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(scripted_imap(
        listener,
        vec![
            ("CAPABILITY".into(), capability(" MOVE UIDPLUS")),
            ("SELECT \"INBOX\"".into(), selected(2, 77)),
            (
                "UID MOVE 41,42 \"Archive\"".into(),
                "{tag} OK moved\r\n".into(),
            ),
        ],
    ));
    let mut session = session(port).await;
    let uids = [41, 42];
    let set = MessageSet {
        folder: "INBOX",
        uid_validity: 77,
        uids: &uids,
    };
    move_in_session(&mut session, set, "Archive").await.unwrap();
    finish(session, Ok(())).await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn move_without_move_capability_copies_then_expunges_only_moved_uids() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(scripted_imap(
        listener,
        vec![
            ("CAPABILITY".into(), capability(" UIDPLUS")),
            ("SELECT \"INBOX\"".into(), selected(1, 77)),
            ("UID COPY 42 \"Trash\"".into(), "{tag} OK copied\r\n".into()),
            (
                "UID STORE 42 +FLAGS.SILENT (\\Deleted)".into(),
                "{tag} OK stored\r\n".into(),
            ),
            (
                "UID EXPUNGE 42".into(),
                "* 1 EXPUNGE\r\n{tag} OK expunged\r\n".into(),
            ),
        ],
    ));
    let mut session = session(port).await;
    let uids = [42];
    let set = MessageSet {
        folder: "INBOX",
        uid_validity: 77,
        uids: &uids,
    };
    move_in_session(&mut session, set, "Trash").await.unwrap();
    finish(session, Ok(())).await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn failed_copy_never_marks_the_original_deleted() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(scripted_imap(
        listener,
        vec![
            ("CAPABILITY".into(), capability("")),
            ("SELECT \"INBOX\"".into(), selected(1, 77)),
            (
                "UID COPY 42 \"Trash\"".into(),
                "{tag} NO [TRYCREATE] no such mailbox\r\n".into(),
            ),
        ],
    ));
    let mut session = session(port).await;
    let uids = [42];
    let set = MessageSet {
        folder: "INBOX",
        uid_validity: 77,
        uids: &uids,
    };
    assert!(move_in_session(&mut session, set, "Trash").await.is_err());
    finish(session, Ok(())).await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn organizing_checks_uidvalidity_before_changing_anything() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(scripted_imap(
        listener,
        vec![
            ("CAPABILITY".into(), capability(" MOVE UIDPLUS")),
            ("SELECT \"INBOX\"".into(), selected(1, 78)),
        ],
    ));
    let mut session = session(port).await;
    let uids = [42];
    let set = MessageSet {
        folder: "INBOX",
        uid_validity: 77,
        uids: &uids,
    };
    let error = delete_permanently_in_session(&mut session, set)
        .await
        .unwrap_err();
    assert!(error.contains("UIDVALIDITY"));
    finish(session, Ok(())).await.unwrap();
    server.await.unwrap();
}

fn compose() -> ComposeMessage {
    ComposeMessage {
        to: vec!["to@example.com".into()],
        cc: vec!["cc@example.com".into()],
        bcc: vec!["hidden@example.com".into()],
        subject: "测试邮件 Fixture".into(),
        body: "hello\r\n.dot at start\r\n".into(),
        in_reply_to: Some("original@example.com".into()),
        references: vec![
            "<earlier@example.com>".into(),
            "original@example.com".into(),
        ],
        attachments: vec![OutgoingAttachment {
            name: "example.txt".into(),
            mime_type: "text/plain".into(),
            data_base64: STANDARD.encode(b"fixture-attachment"),
        }],
        forward: None,
    }
}

async fn smtp_receive(listener: TcpListener, acknowledge: bool) -> String {
    let (stream, _) = listener.accept().await.unwrap();
    let mut stream = acceptor().accept(stream).await.unwrap();
    write(&mut stream, "220 localhost ESMTP\r\n").await;
    let mut peer = BufReader::new(stream);
    assert!(line(&mut peer).await.starts_with("EHLO "));
    write(peer.get_mut(), "250-localhost\r\n250 AUTH PLAIN\r\n").await;
    assert_eq!(
        line(&mut peer).await,
        format!(
            "AUTH PLAIN {}\r\n",
            STANDARD.encode(b"\0sender-user\0outgoing-secret")
        )
    );
    write(peer.get_mut(), "235 authenticated\r\n").await;
    assert!(
        line(&mut peer)
            .await
            .starts_with("MAIL FROM:<sender@example.com>")
    );
    write(peer.get_mut(), "250 sender accepted\r\n").await;
    let mut recipients = Vec::new();
    for _ in 0..3 {
        recipients.push(line(&mut peer).await);
        write(peer.get_mut(), "250 recipient accepted\r\n").await;
    }
    assert!(
        recipients
            .iter()
            .any(|recipient| recipient.contains("hidden@example.com"))
    );
    assert_eq!(line(&mut peer).await, "DATA\r\n");
    write(peer.get_mut(), "354 send data\r\n").await;
    let mut raw = String::new();
    loop {
        let value = line(&mut peer).await;
        if value == ".\r\n" {
            break;
        }
        raw.push_str(
            value
                .strip_prefix('.')
                .filter(|_| value.starts_with(".."))
                .unwrap_or(&value),
        );
    }
    if !acknowledge {
        return raw;
    }
    write(peer.get_mut(), "250 queued\r\n").await;
    assert_eq!(line(&mut peer).await, "QUIT\r\n");
    write(peer.get_mut(), "221 bye\r\n").await;
    raw
}

#[tokio::test]
async fn smtp_data_contains_message_and_attachment_but_not_bcc_header() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut account = account();
    account.outgoing.port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(smtp_receive(listener, true));
    let (message, id) = build_message(&account, &compose(), None).unwrap();
    let raw = message.formatted();
    let transport =
        smtp_transport_with_parameters(&account, "outgoing-secret", smtp_parameters()).unwrap();
    let result = send_prepared(&transport, &message, &raw, id.clone(), async {
        Err("fixture archive unavailable".to_string())
    })
    .await
    .unwrap();
    assert_eq!(result.message_id, id);
    assert!(result.warning.unwrap().contains("邮件已发送"));
    let received = server.await.unwrap();
    // Lettre adds one CRLF before the SMTP dot terminator; it is transport framing.
    assert_eq!(received.strip_suffix("\r\n").unwrap().as_bytes(), raw);
    assert!(!received.contains("Bcc:"));
    assert!(received.contains("In-Reply-To: <original@example.com>"));
    let parsed = mail_parser::MessageParser::default()
        .parse(received.as_bytes())
        .unwrap();
    assert_eq!(parsed.subject(), Some("测试邮件 Fixture"));
    assert_eq!(parsed.message_id(), Some(id.trim_matches(['<', '>'])));
    assert!(parsed.body_text(0).unwrap().contains(".dot at start"));
    assert_eq!(parsed.attachment_count(), 1);
    assert_eq!(
        parsed.attachment(0).unwrap().contents(),
        b"fixture-attachment"
    );
}

#[tokio::test]
async fn disconnected_after_data_reports_uncertainty_without_retry_or_archiving() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut account = account();
    account.outgoing.port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(smtp_receive(listener, false));
    let (message, id) = build_message(&account, &compose(), None).unwrap();
    let raw = message.formatted();
    let transport =
        smtp_transport_with_parameters(&account, "outgoing-secret", smtp_parameters()).unwrap();
    let error = send_prepared(&transport, &message, &raw, id, async {
        panic!("must not archive a message whose delivery was not confirmed")
    })
    .await
    .unwrap_err();
    assert!(error.contains("可能已被服务器接受"));
    assert!(!error.contains("outgoing-secret"));
    assert!(!server.await.unwrap().is_empty());
}

#[tokio::test]
async fn sent_archive_appends_same_mime_to_special_use_folder() {
    let (message, _) = build_message(&account(), &compose(), None).unwrap();
    let raw = message.formatted();
    let expected = raw.clone();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let mut peer = imap_login(listener).await;
        let command = line(&mut peer).await;
        assert!(command.ends_with(" LIST \"\" *\r\n"));
        let tag = command.split_whitespace().next().unwrap();
        write(
            peer.get_mut(),
            &format!("* LIST (\\Sent) \"/\" \"&XfJT0ZAB-\"\r\n{tag} OK list\r\n"),
        )
        .await;
        let append = line(&mut peer).await;
        assert!(append.ends_with(&format!(
            " APPEND \"&XfJT0ZAB-\" (\\Seen) {{{}}}\r\n",
            expected.len()
        )));
        let tag = append.split_whitespace().next().unwrap();
        write(peer.get_mut(), "+ ready\r\n").await;
        let mut uploaded = vec![0; expected.len() + 2];
        peer.read_exact(&mut uploaded).await.unwrap();
        assert_eq!(&uploaded[..expected.len()], expected);
        assert_eq!(&uploaded[expected.len()..], b"\r\n");
        write(peer.get_mut(), &format!("{tag} OK appended\r\n")).await;
        logout(&mut peer).await;
    });
    let mut session = session(port).await;
    archive_in_session(&mut session, &raw).await.unwrap();
    finish(session, Ok(())).await.unwrap();
    server.await.unwrap();
}

#[test]
fn rejects_reply_header_injection_before_any_network_io() {
    let mut message = compose();
    message.in_reply_to = Some("<good@example.com>\r\nBcc: injected@example.com".into());
    assert!(build_message(&account(), &message, None).is_err());
    message.in_reply_to = Some("missing-at-sign".into());
    assert!(build_message(&account(), &message, None).is_err());
}

#[test]
fn forward_sends_html_with_cid_images_inline_and_keeps_attachments() {
    use crate::mail::{ForwardBodies, InlineImage, parse_message, parse_summary};
    let png = include_bytes!("../../icons/32x32.png").to_vec();
    let forward = ForwardBodies {
        text: "FYI\n\n---------- 转发的邮件 ----------\nOriginal text".into(),
        html: Some(
            "<!DOCTYPE html><html><body><p>FYI</p><img src=\"cid:logo@x\"></body></html>".into(),
        ),
        inline: vec![InlineImage {
            content_id: "logo@x".into(),
            mime_type: "image/png".into(),
            data: png.clone(),
        }],
    };
    let (message, _) = build_message(&account(), &compose(), Some(&forward)).unwrap();
    let raw = message.formatted();
    let text = String::from_utf8_lossy(&raw);
    for expected in [
        "multipart/mixed",
        "multipart/alternative",
        "multipart/related",
        "Content-ID: <logo@x>",
    ] {
        assert!(text.contains(expected), "missing {expected}");
    }
    // 用阅读器自己的解析流程验证：HTML 正文、内嵌图片和普通附件各归其位。
    let summary = parse_summary(&raw, 1, 1, false, false, raw.len() as u32).unwrap();
    let detail = parse_message(&raw, summary).unwrap();
    assert!(detail.html_body.unwrap().contains("cid:logo@x"));
    assert!(detail.text_body.contains("Original text"));
    let image = &detail.inline_resources[0];
    assert_eq!(image.content_id.as_deref(), Some("logo@x"));
    assert_eq!(STANDARD.decode(&image.data_base64).unwrap(), png);
    assert!(
        detail
            .attachments
            .iter()
            .any(|item| item.name == "example.txt")
    );
}

#[test]
fn forwarded_part_size_matches_the_built_message_except_for_headers() {
    use crate::mail::{ForwardBodies, InlineImage};
    use crate::sync::forwarded_part_size;
    let forward = ForwardBodies {
        text: "\n\n---------- 转发的邮件 ----------\n原文".into(),
        html: Some(format!("<p>{}</p><img src=\"cid:a@b\">", "长".repeat(5000))),
        inline: vec![InlineImage {
            content_id: "a@b".into(),
            mime_type: "image/png".into(),
            data: include_bytes!("../../icons/32x32.png").to_vec(),
        }],
    };
    let estimate = forwarded_part_size(&forward).unwrap();
    let mut message = compose();
    message.body.clear();
    message.attachments.clear();
    let (built, _) = build_message(&account(), &message, Some(&forward)).unwrap();
    let headers = built.formatted().len() - estimate;
    // 剩下的只有邮件头（发件人、收件人、主题、Message-ID 等），与正文大小无关。
    assert!((200..1500).contains(&headers), "header overhead {headers}");
}
