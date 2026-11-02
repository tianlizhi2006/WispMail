import { createMemo, createSignal, For, onMount, Show } from "solid-js";
import {
  desktopAvailable,
  errorMessage,
  getAppVersion,
  listAccounts,
} from "./lib/api";
import type { Account } from "./lib/types";
import { AccountEditor } from "./features/accounts/AccountEditor";
import { DeleteAccountDialog } from "./features/accounts/DeleteAccountDialog";
import type { ConfigurationMode } from "./features/accounts/accountForm";
import { Icon } from "./ui/Icon";
import { MailWorkspace } from "./features/messages/MailWorkspace";
import "./App.css";

function App() {
  const [version, setVersion] = createSignal("WispMail");
  const [accounts, setAccounts] = createSignal<Account[]>([]);
  const [selectedId, setSelectedId] = createSignal<string>();
  const [loading, setLoading] = createSignal(desktopAvailable);
  const [loadError, setLoadError] = createSignal("");
  const [notification, setNotification] = createSignal("");
  const [editor, setEditor] = createSignal<{
    account?: Account;
    mode?: ConfigurationMode;
  }>();
  const [deleting, setDeleting] = createSignal<Account>();
  const [view, setView] = createSignal<"mail" | "accounts">("mail");
  const selectedAccount = createMemo(() =>
    accounts().find((account) => account.id === selectedId()),
  );

  async function load() {
    if (!desktopAvailable) return;
    setLoading(true);
    setLoadError("");
    try {
      const loaded = await listAccounts();
      setAccounts(loaded);
      if (!loaded.some((account) => account.id === selectedId()))
        setSelectedId(loaded[0]?.id);
    } catch (error) {
      setLoadError(errorMessage(error));
    } finally {
      setLoading(false);
    }
  }

  onMount(() => {
    if (!desktopAvailable) return;
    void load();
    void getAppVersion()
      .then(setVersion)
      .catch(() => {});
  });

  function saved(account: Account) {
    const editing = accounts().some((item) => item.id === account.id);
    setAccounts((items) =>
      editing
        ? items.map((item) => (item.id === account.id ? account : item))
        : [...items, account],
    );
    setSelectedId(account.id);
    setEditor(undefined);
    setView("mail");
    setNotification(
      editing
        ? "账户设置已更新，收发信连接验证通过。"
        : "账户已添加，收发信连接验证通过。",
    );
  }

  function removed(id: string) {
    const remaining = accounts().filter((account) => account.id !== id);
    setAccounts(remaining);
    if (selectedId() === id) setSelectedId(remaining[0]?.id);
    setDeleting(undefined);
    setNotification("已移除账户及本机保存的登录凭据。");
  }

  const accountManagement = () => (
    <div class="app">
      <aside class="sidebar">
        <div class="brand">
          <span class="brand-symbol">
            <Icon name="mail" />
          </span>
          <div>
            <strong>WispMail</strong>
            <span>让邮件，轻一点</span>
          </div>
        </div>
        <div class="sidebar-section">
          <span class="section-label">工作空间</span>
          <div class="navigation-item navigation-item--active">
            <Icon name="settings" />
            <span>邮箱账户</span>
            <span class="count">{accounts().length}</span>
          </div>
          <button
            type="button"
            class="navigation-item navigation-mail-link"
            disabled={!selectedAccount()}
            onClick={() => setView("mail")}
          >
            <Icon name="inbox" />
            <span>收件箱</span>
          </button>
        </div>
        <nav class="account-navigation" aria-label="邮箱账户">
          <div class="section-label account-list-title">
            <span>我的邮箱</span>
            <button
              class="icon-button"
              title="添加邮箱"
              aria-label="添加邮箱"
              disabled={loading() || !!loadError()}
              onClick={() => setEditor({})}
            >
              <Icon name="plus" />
            </button>
          </div>
          <Show
            when={accounts().length > 0}
            fallback={
              <p class="sidebar-empty">
                {loading()
                  ? "正在读取账户…"
                  : loadError()
                    ? "账户暂时无法读取"
                    : "添加你的第一个邮箱，\n从这里开始。"}
              </p>
            }
          >
            <For each={accounts()}>
              {(account) => (
                <button
                  class="account-nav-item"
                  classList={{
                    "account-nav-item--selected": selectedId() === account.id,
                  }}
                  aria-current={
                    selectedId() === account.id ? "true" : undefined
                  }
                  onClick={() => {
                    setSelectedId(account.id);
                    setNotification("");
                  }}
                >
                  <span class="account-avatar">
                    {(account.displayName || account.email)
                      .slice(0, 1)
                      .toUpperCase()}
                  </span>
                  <span class="account-nav-text">
                    <strong>
                      {account.displayName || account.email.split("@")[0]}
                    </strong>
                    <span>{account.email}</span>
                  </span>
                </button>
              )}
            </For>
          </Show>
        </nav>
        <div class="sidebar-bottom">
          <Icon name="shield" />
          <p>
            连接你的邮箱服务商
            <br />
            <span>凭据由系统安全保管</span>
          </p>
        </div>
      </aside>
      <main class="workspace">
        <header class="workspace-header">
          <div>
            <span class="eyebrow">你的邮件，从这里开始</span>
            <h1>邮箱账户</h1>
          </div>
          <button
            class="button button--primary"
            disabled={loading() || !!loadError()}
            onClick={() => setEditor({})}
          >
            <Icon name="plus" />
            添加邮箱
          </button>
        </header>
        <div class="workspace-body">
          <Show when={!desktopAvailable}>
            <div class="notice browser-notice">
              <strong>浏览器预览模式</strong>
              <span>账户连接与安全存储需要在桌面应用中使用。</span>
            </div>
          </Show>
          <Show when={notification()}>
            <div class="success-notice" role="status">
              <Icon name="check" />
              <span>{notification()}</span>
              <button
                class="icon-button"
                aria-label="关闭提示"
                onClick={() => setNotification("")}
              >
                <Icon name="close" />
              </button>
            </div>
          </Show>
          <Show when={loadError()}>
            <div class="load-error" role="alert">
              <strong>暂时无法读取账户</strong>
              <p>{loadError()}</p>
              <button
                class="button button--secondary"
                disabled={loading()}
                onClick={() => void load()}
              >
                {loading() ? "正在重试…" : "重试"}
              </button>
            </div>
          </Show>
          <Show
            when={!loading() && !loadError()}
            fallback={
              <Show when={loading()}>
                <div class="loading-empty" role="status">
                  <span class="spinner" />
                  正在读取已保存的邮箱…
                </div>
              </Show>
            }
          >
            <Show
              when={selectedAccount()}
              fallback={
                <section class="welcome-card">
                  <div class="welcome-art">
                    <div class="welcome-orbit" />
                    <span class="welcome-mail">
                      <Icon name="mail" />
                    </span>
                    <span class="welcome-check">
                      <Icon name="check" />
                    </span>
                  </div>
                  <span class="eyebrow">轻巧 · 私密 · 专注</span>
                  <h2>所有邮箱，一个轻盈的开始。</h2>
                  <p>
                    连接你的日常邮箱或公司邮箱。
                    <br />
                    输入地址，WispMail 会帮你填写服务器配置。
                  </p>
                  <button
                    class="button button--primary welcome-button"
                    onClick={() => setEditor({})}
                  >
                    连接第一个邮箱
                    <Icon name="arrow" />
                  </button>
                  <button
                    class="manual-entry"
                    onClick={() => setEditor({ mode: "manual" })}
                  >
                    连接自建 / 域名邮箱
                  </button>
                  <div class="provider-list">
                    <span>QQ 邮箱</span>
                    <span>网易邮箱</span>
                    <span>iCloud</span>
                    <span>自建 / 域名邮箱</span>
                  </div>
                </section>
              }
            >
              {(account) => (
                <section class="account-detail">
                  <header class="account-detail-header">
                    <span class="account-avatar account-avatar--large">
                      {(account().displayName || account().email)
                        .slice(0, 1)
                        .toUpperCase()}
                    </span>
                    <div>
                      <h2>
                        {account().displayName || account().email.split("@")[0]}
                      </h2>
                      <p>{account().email}</p>
                    </div>
                    <span class="saved-badge">
                      <Icon name="check" />
                      配置已保存
                    </span>
                  </header>
                  <div class="detail-servers">
                    <For
                      each={[
                        {
                          label: "收信服务器",
                          protocol: "IMAP",
                          server: account().incoming,
                          username:
                            account().incomingUsername || account().email,
                        },
                        {
                          label: "发信服务器",
                          protocol: "SMTP",
                          server: account().outgoing,
                          username:
                            account().outgoingUsername || account().email,
                        },
                      ]}
                    >
                      {(item) => (
                        <section>
                          <div class="server-heading">
                            <Icon name="server" />
                            <h3>{item.label}</h3>
                            <span class="tag">{item.protocol}</span>
                          </div>
                          <dl>
                            <div>
                              <dt>服务器</dt>
                              <dd>{item.server.host}</dd>
                            </div>
                            <div>
                              <dt>用户名</dt>
                              <dd>{item.username}</dd>
                            </div>
                            <div>
                              <dt>端口</dt>
                              <dd>{item.server.port}</dd>
                            </div>
                            <div>
                              <dt>加密</dt>
                              <dd>
                                {item.server.encryption === "tls"
                                  ? "SSL / TLS"
                                  : "STARTTLS"}
                              </dd>
                            </div>
                          </dl>
                        </section>
                      )}
                    </For>
                  </div>
                  <div class="account-security">
                    <Icon name="shield" />
                    <div>
                      <strong>登录凭据已安全保存</strong>
                      <p>
                        {account().useSamePassword !== false
                          ? "收发信共用一个密码。"
                          : "收发信使用独立密码。"}
                        凭据保存在系统凭据管理器中，可在编辑账户时替换。
                      </p>
                    </div>
                  </div>
                  <footer class="account-detail-footer">
                    <button
                      class="button button--secondary"
                      onClick={() => setEditor({ account: account() })}
                    >
                      <Icon name="settings" />
                      编辑账户
                    </button>
                    <button
                      class="button button--quiet-danger"
                      onClick={() => setDeleting(account())}
                    >
                      <Icon name="trash" />
                      移除账户
                    </button>
                  </footer>
                </section>
              )}
            </Show>
            <div class="next-step">
              <span class="next-step-icon">
                <Icon name="inbox" />
              </span>
              <div>
                <strong>连接后，即可收发邮件</strong>
                <p>
                  在收件箱中同步和阅读邮件，也可以撰写、回复并发送带附件的邮件。
                </p>
              </div>
              <Show when={selectedAccount()}>
                <button
                  type="button"
                  class="button button--secondary"
                  onClick={() => setView("mail")}
                >
                  打开邮箱
                  <Icon name="arrow" />
                </button>
              </Show>
            </div>
          </Show>
        </div>
        <footer class="statusbar">
          <span>
            <span class="status-dot" />
            {desktopAvailable ? "本地账户管理" : "界面预览"}
          </span>
          <span>{version()}</span>
        </footer>
      </main>
      <Show when={editor()} keyed>
        {(state) => (
          <AccountEditor
            account={state.account}
            initialMode={state.mode}
            onClose={() => setEditor(undefined)}
            onSaved={saved}
          />
        )}
      </Show>
      <Show when={deleting()} keyed>
        {(account) => (
          <DeleteAccountDialog
            account={account}
            onClose={() => setDeleting(undefined)}
            onDeleted={removed}
          />
        )}
      </Show>
    </div>
  );

  return (
    <Show
      when={view() === "mail" ? selectedAccount() : undefined}
      keyed
      fallback={accountManagement()}
    >
      {(account) => (
        <MailWorkspace
          account={account}
          accounts={accounts()}
          onAccountChange={setSelectedId}
          onSettings={() => setView("accounts")}
        />
      )}
    </Show>
  );
}

export default App;
