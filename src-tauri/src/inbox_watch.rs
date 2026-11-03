//! 新邮件实时到达：每个账户一条收件箱监视连接（IDLE 或轮询）。发现变化后同步收件箱、
//! 找出真正新到的未读邮件，通知界面刷新；窗口不在前台时弹出系统通知。

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, async_runtime::JoinHandle};
use tauri_plugin_notification::NotificationExt;
use tokio::{sync::Notify, time::sleep};

use crate::{
    commands::{AccountsState, with_accounts},
    mail::MessageSummary,
    mail_commands::sync_and_cache,
    sync::{self, INBOX, WatchTiming},
};

/// 连接失败后的重试间隔（秒）；连接稳定运行一段时间后从头计算。
const BACKOFF: [u64; 6] = [5, 15, 30, 60, 120, 300];
const STABLE_CONNECTION: Duration = Duration::from_secs(120);
/// 服务器常连续推送多条变化（EXISTS、RECENT、FETCH），稍等片刻合并为一次同步。
const COALESCE: Duration = Duration::from_millis(800);

/// 界面收到后刷新对应文件夹的列表。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailChanged {
    pub account_id: String,
    pub folder: String,
    pub new_count: usize,
}

pub const MAIL_CHANGED_EVENT: &str = "mail-changed";

#[derive(Default)]
pub struct InboxWatchers {
    tasks: Mutex<HashMap<String, Vec<JoinHandle<()>>>>,
}

impl InboxWatchers {
    /// 启动（或在账户设置变化后重启）某账户的收件箱监视。
    pub fn start(&self, app: &AppHandle, account_id: &str) {
        self.stop(account_id);
        let changed = Arc::new(Notify::new());
        let tasks = vec![
            tauri::async_runtime::spawn(watch(
                app.clone(),
                account_id.to_string(),
                Arc::clone(&changed),
            )),
            tauri::async_runtime::spawn(refresh_on_change(
                app.clone(),
                account_id.to_string(),
                changed,
            )),
        ];
        if let Ok(mut running) = self.tasks.lock() {
            running.insert(account_id.to_string(), tasks);
        }
    }

    pub fn stop(&self, account_id: &str) {
        let tasks = self
            .tasks
            .lock()
            .ok()
            .and_then(|mut running| running.remove(account_id));
        for task in tasks.into_iter().flatten() {
            task.abort();
        }
    }
}

/// 应用启动时为所有已保存的账户开始监视。
pub async fn start_all(app: AppHandle) {
    let state = app.state::<AccountsState>();
    let Ok(accounts) = with_accounts(&state, |service| service.list_accounts()).await else {
        return;
    };
    let watchers = app.state::<InboxWatchers>();
    for account in accounts {
        watchers.start(&app, &account.id);
    }
}

async fn watch(app: AppHandle, account_id: String, changed: Arc<Notify>) {
    let mut failures = 0usize;
    loop {
        let state = app.state::<AccountsState>();
        let id = account_id.clone();
        // 账户已删除或凭据不可用时停止；保存账户时会重新启动。
        let Ok((account, credentials)) =
            with_accounts(&state, move |service| service.load_connection(&id)).await
        else {
            return;
        };
        let started = Instant::now();
        let _ = sync::watch_inbox(&account, &credentials, WatchTiming::default(), || {
            changed.notify_one();
            true
        })
        .await;
        if started.elapsed() > STABLE_CONNECTION {
            failures = 0;
        }
        sleep(Duration::from_secs(
            BACKOFF[failures.min(BACKOFF.len() - 1)],
        ))
        .await;
        failures += 1;
    }
}

async fn refresh_on_change(app: AppHandle, account_id: String, changed: Arc<Notify>) {
    let mut known = cached_baseline(&app, &account_id).await;
    loop {
        changed.notified().await;
        sleep(COALESCE).await;
        let Ok(page) = sync_and_cache(&app, account_id.clone(), INBOX.into(), None, None).await
        else {
            continue;
        };
        let Some(validity) = page.uid_validity else {
            continue;
        };
        let (fresh, baseline) = new_messages(known, validity, &page.messages);
        known = Some(baseline);
        let _ = app.emit(
            MAIL_CHANGED_EVENT,
            MailChanged {
                account_id: account_id.clone(),
                folder: INBOX.into(),
                new_count: fresh.len(),
            },
        );
        if !fresh.is_empty() {
            notify_user(&app, &account_id, &fresh).await;
        }
    }
}

/// 找出比已知最大 UID 更新、且仍未读的邮件，并返回新的基线 (UIDVALIDITY, 最大 UID)。
/// 第一次同步或收件箱被重建（UIDVALIDITY 变化）时只建立基线，不把旧邮件当作新邮件。
fn new_messages(
    known: Option<(u32, u32)>,
    validity: u32,
    messages: &[MessageSummary],
) -> (Vec<MessageSummary>, (u32, u32)) {
    let previous = known
        .filter(|(known_validity, _)| *known_validity == validity)
        .map(|(_, max)| max);
    let fresh = previous
        .map(|max| {
            messages
                .iter()
                .filter(|message| message.uid > max && message.unread)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let max = messages
        .iter()
        .map(|message| message.uid)
        .chain(previous)
        .max()
        .unwrap_or(0);
    (fresh, (validity, max))
}

/// 本地缓存中收件箱的最新邮件，作为启动时的基线：离线期间到达的邮件在第一次同步后提醒。
async fn cached_baseline(app: &AppHandle, account_id: &str) -> Option<(u32, u32)> {
    let state = app.state::<AccountsState>();
    let id = account_id.to_string();
    let page = with_accounts(&state, move |service| {
        service
            .store()
            .list_messages(&id, INBOX, None)
            .map_err(|error| error.to_string())
    })
    .await
    .ok()?;
    let max = page.messages.iter().map(|message| message.uid).max()?;
    Some((page.uid_validity?, max))
}

async fn notify_user(app: &AppHandle, account_id: &str, fresh: &[MessageSummary]) {
    // 正在使用 WispMail 时列表会自动更新，不再弹出打扰的通知。
    let focused = app
        .get_webview_window("main")
        .and_then(|window| window.is_focused().ok())
        .unwrap_or(false);
    if focused {
        return;
    }
    let state = app.state::<AccountsState>();
    let id = account_id.to_string();
    let email = with_accounts(&state, move |service| service.get_account(&id))
        .await
        .map(|account| account.email)
        .unwrap_or_default();
    let (title, body) = notification_text(&email, fresh);
    let _ = app.notification().builder().title(title).body(body).show();
}

fn notification_text(email: &str, fresh: &[MessageSummary]) -> (String, String) {
    let newest = fresh.iter().max_by_key(|message| message.uid);
    let line = newest
        .map(|message| format!("{}：{}", sender_name(&message.from), message.subject))
        .unwrap_or_default();
    let title = if email.is_empty() {
        "新邮件".to_string()
    } else {
        format!("新邮件 · {email}")
    };
    let body = if fresh.len() > 1 {
        format!("收到 {} 封新邮件\n最新 {line}", fresh.len())
    } else {
        line
    };
    (title, body.chars().take(240).collect())
}

/// "张三 <a@example.com>" 显示为 "张三"；没有名字时显示地址。
fn sender_name(from: &str) -> String {
    let name = from
        .split('<')
        .next()
        .unwrap_or_default()
        .trim()
        .trim_matches('"')
        .trim();
    if name.is_empty() {
        from.trim_matches(['<', '>', ' ']).to_string()
    } else {
        name.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(uid: u32, unread: bool) -> MessageSummary {
        MessageSummary {
            uid,
            uid_validity: 7,
            from: format!("\"发件人{uid}\" <s{uid}@example.com>"),
            to: vec![],
            subject: format!("主题{uid}"),
            snippet: String::new(),
            date: String::new(),
            unread,
            flagged: false,
            has_attachments: false,
            size: 1,
        }
    }

    #[test]
    fn only_unread_messages_above_the_known_uid_are_new() {
        let page = [message(12, true), message(11, false), message(10, true)];
        let (fresh, baseline) = new_messages(Some((7, 10)), 7, &page);
        assert_eq!(
            fresh.iter().map(|m| m.uid).collect::<Vec<_>>(),
            vec![12],
            "11 was already read elsewhere, 10 was known"
        );
        assert_eq!(baseline, (7, 12));
        // 删除邮件后列表变短，基线不能倒退，否则之后会重复提醒。
        let (fresh, baseline) = new_messages(Some((7, 12)), 7, &[message(10, true)]);
        assert!(fresh.is_empty());
        assert_eq!(baseline, (7, 12));
    }

    #[test]
    fn first_sync_and_rebuilt_mailbox_only_set_a_baseline() {
        let page = [message(5, true), message(4, true)];
        let (fresh, baseline) = new_messages(None, 7, &page);
        assert!(fresh.is_empty());
        assert_eq!(baseline, (7, 5));
        let (fresh, baseline) = new_messages(Some((6, 100)), 7, &page);
        assert!(
            fresh.is_empty(),
            "UIDVALIDITY changed: old UIDs mean nothing"
        );
        assert_eq!(baseline, (7, 5));
        // 空收件箱也建立基线，之后的第一封邮件能被提醒。
        let (_, baseline) = new_messages(None, 7, &[]);
        let (fresh, _) = new_messages(Some(baseline), 7, &[message(1, true)]);
        assert_eq!(fresh.len(), 1);
    }

    #[test]
    fn notification_names_the_newest_sender_and_counts_the_rest() {
        let (title, body) = notification_text("me@example.com", &[message(3, true)]);
        assert_eq!(title, "新邮件 · me@example.com");
        assert_eq!(body, "发件人3：主题3");
        let (_, body) = notification_text("", &[message(3, true), message(9, true)]);
        assert_eq!(body, "收到 2 封新邮件\n最新 发件人9：主题9");
        assert_eq!(sender_name("<only@example.com>"), "only@example.com");
    }
}
