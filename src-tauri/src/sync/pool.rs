//! 每个账户保留一条空闲 IMAP 连接。连续打开邮件、标记已读时免去重复的
//! TCP / TLS / 登录握手，这是本地浏览时最主要的等待来源。
//!
//! 调用方已按账户串行化网络操作（`ConnectionPermit`），因此每个账户只需一条连接。

use std::{
    collections::HashMap,
    future::Future,
    sync::Mutex,
    time::{Duration, Instant},
};

use tokio::time::timeout;

use crate::account::{Account, AccountCredentials};

use super::{
    connection::{ImapSession, open_imap},
    mailbox::OPERATION_TIMEOUT,
};

/// 超过此时长未使用的连接直接丢弃；多数服务器和 NAT 会在数分钟后静默断开空闲连接。
const IDLE_LIMIT: Duration = Duration::from_secs(240);
/// 空闲超过此时长的连接先用 NOOP 快速探活，避免在已断开的连接上等待完整超时。
const CHECK_AFTER: Duration = Duration::from_secs(20);
const CHECK_TIMEOUT: Duration = Duration::from_secs(5);

struct IdleSession {
    account: Account,
    session: ImapSession,
    since: Instant,
}

#[derive(Default)]
pub struct ImapPool {
    idle: Mutex<HashMap<String, IdleSession>>,
}

impl ImapPool {
    /// 账户被编辑或删除后调用，确保不会继续使用旧配置或旧凭据登录的连接。
    pub fn forget(&self, account_id: &str) {
        if let Ok(mut idle) = self.idle.lock() {
            idle.remove(account_id);
        }
    }

    fn take(&self, account: &Account) -> Option<(ImapSession, Duration)> {
        let entry = self.idle.lock().ok()?.remove(&account.id)?;
        let age = entry.since.elapsed();
        (entry.account == *account && age < IDLE_LIMIT).then_some((entry.session, age))
    }

    fn put(&self, account: &Account, session: ImapSession) {
        if let Ok(mut idle) = self.idle.lock() {
            idle.insert(
                account.id.clone(),
                IdleSession {
                    account: account.clone(),
                    session,
                    since: Instant::now(),
                },
            );
        }
    }

    async fn reusable(&self, account: &Account) -> Option<ImapSession> {
        let (mut session, age) = self.take(account)?;
        if age >= CHECK_AFTER && !matches!(timeout(CHECK_TIMEOUT, session.noop()).await, Ok(Ok(())))
        {
            return None;
        }
        Some(session)
    }

    /// 在复用或新建的连接上执行操作。只有成功的连接才放回池中，出错的连接可能
    /// 处于协议不同步状态，直接丢弃。
    ///
    /// 操作必须幂等：复用的连接失败时（可能已被服务器静默断开）会用新连接重试一次。
    pub(super) async fn run<T, F, Fut>(
        &self,
        account: &Account,
        credentials: &AccountCredentials,
        mut operation: F,
    ) -> Result<T, String>
    where
        F: FnMut(ImapSession) -> Fut,
        Fut: Future<Output = (ImapSession, Result<T, String>)>,
    {
        if let Some(session) = self.reusable(account).await
            && let Ok((session, Ok(value))) = timeout(OPERATION_TIMEOUT, operation(session)).await
        {
            self.put(account, session);
            return Ok(value);
        }
        let (session, result) = timeout(OPERATION_TIMEOUT, async {
            let session = open_imap(account, credentials).await?;
            Ok::<_, String>(operation(session).await)
        })
        .await
        .map_err(|_| "邮件服务器操作超时，请检查网络后重试。".to_string())??;
        if result.is_ok() {
            self.put(account, session);
        }
        result
    }
}
