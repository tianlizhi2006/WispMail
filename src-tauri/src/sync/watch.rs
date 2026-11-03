//! 新邮件实时到达：为收件箱保持一条独立连接。服务器支持 IDLE 时由服务器主动推送变化；
//! 不支持时定期用 STATUS 比较邮件数和 UIDNEXT。这里只负责发现"收件箱有变化"，
//! 具体同步、去重和通知由上层完成。

use std::time::Duration;

use async_imap::extensions::idle::IdleResponse;
use tokio::time::{sleep, timeout};

use crate::account::{Account, AccountCredentials};

use super::{
    connection::{ImapSession, open_imap},
    mailbox::{OPERATION_TIMEOUT, bounded, finish},
};

pub const INBOX: &str = "INBOX";

/// IDLE 的刷新周期与轮询间隔。
#[derive(Debug, Clone, Copy)]
pub struct WatchTiming {
    /// 定期结束并重新发起 IDLE（RFC 2177 建议不超过 29 分钟）。较短的周期也能
    /// 更快发现已经悄悄断开的连接。
    pub idle_refresh: Duration,
    /// 服务器不支持 IDLE 时的检查间隔。
    pub poll_interval: Duration,
}

impl Default for WatchTiming {
    fn default() -> Self {
        Self {
            idle_refresh: Duration::from_secs(9 * 60),
            poll_interval: Duration::from_secs(3 * 60),
        }
    }
}

/// 连接并持续监视收件箱，直到 `on_change` 返回 false（返回 Ok）或连接出错（返回 Err，
/// 由调用方稍后重连）。连接建立后会先回调一次，补上断线期间可能错过的变化。
pub async fn watch_inbox(
    account: &Account,
    credentials: &AccountCredentials,
    timing: WatchTiming,
    on_change: impl FnMut() -> bool,
) -> Result<(), String> {
    let session = bounded(open_imap(account, credentials)).await?;
    watch_session(session, timing, on_change).await
}

pub(super) async fn watch_session(
    mut session: ImapSession,
    timing: WatchTiming,
    mut on_change: impl FnMut() -> bool,
) -> Result<(), String> {
    let supports_idle = bounded(async {
        session
            .capabilities()
            .await
            .map(|capabilities| capabilities.has_str("IDLE"))
            .map_err(|_| "服务器未返回能力列表。".to_string())
    })
    .await;
    match supports_idle {
        Ok(true) => idle_loop(session, timing, &mut on_change).await,
        Ok(false) => {
            let result = poll_loop(&mut session, timing, &mut on_change).await;
            finish(session, result).await
        }
        Err(error) => finish(session, Err(error)).await,
    }
}

async fn idle_loop(
    mut session: ImapSession,
    timing: WatchTiming,
    on_change: &mut impl FnMut() -> bool,
) -> Result<(), String> {
    // EXAMINE 以只读方式打开收件箱，不会改变邮件的 \Recent 等状态。
    if let Err(error) = bounded(async {
        session
            .examine(INBOX)
            .await
            .map(|_| ())
            .map_err(|_| "无法打开收件箱。".to_string())
    })
    .await
    {
        return finish(session, Err(error)).await;
    }
    if !on_change() {
        return finish(session, Ok(())).await;
    }
    loop {
        let mut idle = session.idle();
        timeout(OPERATION_TIMEOUT, idle.init())
            .await
            .map_err(|_| "服务器没有响应 IDLE。".to_string())?
            .map_err(|_| "服务器拒绝了 IDLE。".to_string())?;
        let changed = {
            let (wait, interrupt) = idle.wait_with_timeout(timing.idle_refresh);
            let response = wait.await.map_err(|_| "实时连接已断开。".to_string())?;
            drop(interrupt);
            matches!(response, IdleResponse::NewData(_))
        };
        session = timeout(OPERATION_TIMEOUT, idle.done())
            .await
            .map_err(|_| "结束 IDLE 时服务器没有响应。".to_string())?
            .map_err(|_| "结束 IDLE 失败。".to_string())?;
        if changed && !on_change() {
            return finish(session, Ok(())).await;
        }
    }
}

async fn poll_loop(
    session: &mut ImapSession,
    timing: WatchTiming,
    on_change: &mut impl FnMut() -> bool,
) -> Result<(), String> {
    let mut last = status(session).await?;
    if !on_change() {
        return Ok(());
    }
    loop {
        sleep(timing.poll_interval).await;
        let current = status(session).await?;
        if current != last {
            last = current;
            if !on_change() {
                return Ok(());
            }
        }
    }
}

/// 收件箱的邮件数与下一个 UID：任何一项变化都说明有新邮件或邮件被删除。
async fn status(session: &mut ImapSession) -> Result<(u32, Option<u32>), String> {
    bounded(async {
        session
            .status(INBOX, "(MESSAGES UIDNEXT)")
            .await
            .map(|mailbox| (mailbox.exists, mailbox.uid_next))
            .map_err(|_| "无法检查收件箱状态。".to_string())
    })
    .await
}
