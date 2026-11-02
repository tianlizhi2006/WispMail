import { createSignal, onCleanup, onMount, Show } from "solid-js";
import {
  desktopAvailable,
  discoverConfig,
  errorMessage,
  saveAccount,
  testAccountConnection,
} from "../../lib/api";
import type {
  Account,
  AccountInput,
  ConnectionReport,
  DiscoveredConfig,
  ServerConfig,
} from "../../lib/types";
import { Icon } from "../../ui/Icon";
import {
  accountInputForSubmission,
  createAccountDraft,
  createDiscoveryGuard,
  requiresOutgoingPassword,
  setPasswordSharing,
  shouldDiscoverConfig,
  type ConfigurationMode,
} from "./accountForm";

export function AccountEditor(props: {
  account?: Account;
  initialMode?: ConfigurationMode;
  onClose: () => void;
  onSaved: (account: Account) => void;
}) {
  const initial = props.account;
  const [draft, setDraft] = createSignal<AccountInput>(
    createAccountDraft(initial),
  );
  const [mode, setMode] = createSignal<ConfigurationMode>(
    initial ? "manual" : (props.initialMode ?? "automatic"),
  );
  const [discovered, setDiscovered] = createSignal<DiscoveredConfig>();
  const [lastDiscoveredEmail, setLastDiscoveredEmail] = createSignal(
    initial?.email ?? "",
  );
  const [busy, setBusy] = createSignal<"discover" | "test" | "save" | null>(
    null,
  );
  const [error, setError] = createSignal("");
  const [result, setResult] = createSignal<ConnectionReport>();
  let dialog!: HTMLDialogElement;
  let form!: HTMLFormElement;
  let emailInput!: HTMLInputElement;
  let passwordInput!: HTMLInputElement;
  let outgoingPasswordInput: HTMLInputElement | undefined;
  let alive = true;
  const discovery = createDiscoveryGuard();

  function clearSecrets() {
    if (passwordInput) passwordInput.value = "";
    if (outgoingPasswordInput) outgoingPasswordInput.value = "";
    setDraft((current) => ({ ...current, password: "", outgoingPassword: "" }));
  }

  onMount(() => dialog.showModal());
  onCleanup(() => {
    alive = false;
    discovery.invalidate();
    clearSecrets();
  });

  function close() {
    if (busy() === "test" || busy() === "save") return;
    clearSecrets();
    dialog.close();
    props.onClose();
  }

  function update(patch: Partial<AccountInput>) {
    if (patch.email !== undefined || patch.incoming || patch.outgoing)
      discovery.invalidate();
    setDraft((current) => ({ ...current, ...patch }));
    setResult(undefined);
    setError("");
    if (patch.email !== undefined) setDiscovered(undefined);
  }

  function changeMode(next: ConfigurationMode) {
    if (next === mode()) return;
    discovery.invalidate();
    setMode(next);
    setDiscovered(undefined);
    setLastDiscoveredEmail("");
    setResult(undefined);
    setError("");
    if (busy() === "discover") setBusy(null);
  }

  function changePasswordSharing(shared: boolean) {
    if (outgoingPasswordInput) outgoingPasswordInput.value = "";
    const changed = setPasswordSharing(draft(), shared);
    update({
      useSamePassword: changed.useSamePassword,
      outgoingPassword: changed.outgoingPassword,
    });
  }

  function updateServer(
    side: "incoming" | "outgoing",
    patch: Partial<ServerConfig>,
  ) {
    update({ [side]: { ...draft()[side], ...patch } });
  }

  function changeEncryption(
    side: "incoming" | "outgoing",
    encryption: "tls" | "starttls",
  ) {
    const current = draft()[side];
    const ports =
      side === "incoming"
        ? { tls: 993, starttls: 143 }
        : { tls: 465, starttls: 587 };
    const wasDefault =
      current.encryption !== "none" &&
      current.port === ports[current.encryption];
    updateServer(side, {
      encryption,
      ...(wasDefault ? { port: ports[encryption] } : {}),
    });
  }

  async function discover(force = false) {
    const email = draft().email.trim();
    if (
      !desktopAvailable ||
      (busy() && busy() !== "discover") ||
      !shouldDiscoverConfig(mode(), email, lastDiscoveredEmail(), force) ||
      !emailInput.validity.valid
    )
      return;
    const token = discovery.begin();
    setBusy("discover");
    setError("");
    setResult(undefined);
    try {
      const config = await discoverConfig(email);
      if (!alive || !discovery.canApply(token) || mode() !== "automatic")
        return;
      if (!config) {
        setError("请输入有效的邮箱地址。");
        return;
      }
      setDiscovered(config);
      setLastDiscoveredEmail(email);
      setDraft((current) => ({
        ...current,
        email,
        incoming: config.incoming,
        outgoing: config.outgoing,
      }));
    } catch (error) {
      if (alive && discovery.canApply(token) && mode() === "automatic")
        setError(errorMessage(error));
    } finally {
      if (alive && discovery.isLatest(token) && busy() === "discover")
        setBusy(null);
    }
  }

  async function submit(action: "test" | "save") {
    if (busy() || !form.reportValidity()) return;
    const input = accountInputForSubmission(draft());
    setBusy(action);
    setError("");
    setResult(undefined);
    try {
      if (action === "test") {
        const report = await testAccountConnection(input);
        if (alive) setResult(report);
      } else {
        const account = await saveAccount(input);
        if (alive) {
          clearSecrets();
          dialog.close();
          props.onSaved(account);
        }
      }
    } catch (error) {
      if (alive) setError(errorMessage(error));
    } finally {
      if (alive) setBusy(null);
    }
  }

  function serverFields(
    side: "incoming" | "outgoing",
    title: string,
    protocol: string,
  ) {
    return (
      <section class="server-fields" aria-label={title}>
        <div class="server-heading">
          <Icon name="server" />
          <h3>{title}</h3>
          <span class="tag">{protocol}</span>
        </div>
        <label>
          服务器地址
          <input
            name={`${side}-host`}
            required
            maxlength="253"
            autocomplete="off"
            spellcheck={false}
            placeholder={
              mode() === "manual"
                ? "mail.example.com"
                : side === "incoming"
                  ? "imap.example.com"
                  : "smtp.example.com"
            }
            value={draft()[side].host}
            onInput={(event) =>
              updateServer(side, { host: event.currentTarget.value })
            }
          />
        </label>
        <div class="server-options">
          <label>
            端口
            <input
              name={`${side}-port`}
              type="number"
              required
              min="1"
              max="65535"
              step="1"
              value={draft()[side].port || ""}
              onInput={(event) =>
                updateServer(side, { port: Number(event.currentTarget.value) })
              }
            />
          </label>
          <label>
            连接加密
            <select
              name={`${side}-encryption`}
              value={draft()[side].encryption}
              onChange={(event) =>
                changeEncryption(
                  side,
                  event.currentTarget.value as "tls" | "starttls",
                )
              }
            >
              <option value="tls">SSL / TLS</option>
              <option value="starttls">STARTTLS</option>
            </select>
          </label>
        </div>
        <p class="port-help">
          {side === "incoming"
            ? "常用端口：TLS 993 / STARTTLS 143"
            : "常用端口：TLS 465 / STARTTLS 587"}
        </p>
        <label class="server-username">
          登录用户名 <span class="optional">留空使用邮箱地址</span>
          <input
            name={`${side}-username`}
            type="text"
            maxlength="512"
            autocomplete="off"
            spellcheck={false}
            placeholder={draft().email || "user 或 DOMAIN\\user"}
            value={
              side === "incoming"
                ? draft().incomingUsername
                : draft().outgoingUsername
            }
            onInput={(event) =>
              update(
                side === "incoming"
                  ? { incomingUsername: event.currentTarget.value }
                  : { outgoingUsername: event.currentTarget.value },
              )
            }
          />
        </label>
        <Show
          when={side === "incoming"}
          fallback={
            <div class="outgoing-auth">
              <label class="password-sharing">
                <input
                  name="useSamePassword"
                  type="checkbox"
                  checked={draft().useSamePassword}
                  onChange={(event) =>
                    changePasswordSharing(event.currentTarget.checked)
                  }
                />
                <span>发信使用收信密码</span>
              </label>
              <Show when={!draft().useSamePassword}>
                <label>
                  发信密码 / 授权码{" "}
                  <span class="optional">
                    {!requiresOutgoingPassword(draft(), initial)
                      ? "留空保留原凭据"
                      : ""}
                  </span>
                  <input
                    ref={outgoingPasswordInput}
                    name="outgoingPassword"
                    type="password"
                    required={requiresOutgoingPassword(draft(), initial)}
                    maxlength="4096"
                    autocomplete="new-password"
                    placeholder={
                      requiresOutgoingPassword(draft(), initial)
                        ? "输入 SMTP 登录密码"
                        : "已安全保存，输入可替换"
                    }
                    value={draft().outgoingPassword ?? ""}
                    onInput={(event) =>
                      update({ outgoingPassword: event.currentTarget.value })
                    }
                  />
                </label>
              </Show>
            </div>
          }
        >
          <label class="incoming-password">
            收信密码 / 授权码{" "}
            <span class="optional">{initial ? "留空保留原凭据" : ""}</span>
            <input
              ref={passwordInput}
              name="password"
              type="password"
              required={!initial}
              maxlength="4096"
              autocomplete="new-password"
              placeholder={
                initial ? "已安全保存，输入可替换" : "输入密码或邮箱授权码"
              }
              value={draft().password ?? ""}
              onInput={(event) =>
                update({ password: event.currentTarget.value })
              }
            />
          </label>
        </Show>
      </section>
    );
  }

  return (
    <dialog
      ref={dialog}
      class="account-dialog"
      aria-labelledby="account-dialog-title"
      onCancel={(event) => {
        event.preventDefault();
        close();
      }}
    >
      <header class="dialog-header">
        <div>
          <span class="eyebrow">邮箱设置</span>
          <h2 id="account-dialog-title">
            {initial ? "编辑邮箱账户" : "连接一个邮箱"}
          </h2>
        </div>
        <button
          class="icon-button"
          type="button"
          aria-label="关闭账户设置"
          onClick={close}
          disabled={busy() === "test" || busy() === "save"}
        >
          <Icon name="close" />
        </button>
      </header>
      <form
        ref={form}
        onSubmit={(event) => {
          event.preventDefault();
          void submit("save");
        }}
      >
        <div class="dialog-body">
          <Show when={!desktopAvailable}>
            <p class="notice">
              当前为浏览器预览。请启动 WispMail
              桌面应用以识别配置、测试连接和保存账户。
            </p>
          </Show>
          <fieldset
            disabled={busy() === "test" || busy() === "save"}
            class="form-fields"
          >
            <div
              class="configuration-modes"
              role="group"
              aria-label="邮箱配置方式"
            >
              <button
                type="button"
                class="configuration-mode"
                classList={{
                  "configuration-mode--selected": mode() === "automatic",
                }}
                aria-pressed={mode() === "automatic"}
                onClick={() => changeMode("automatic")}
              >
                <Icon name="mail" />
                <span>
                  <strong>常见邮箱</strong>
                  <small>自动识别服务器配置</small>
                </span>
              </button>
              <button
                type="button"
                class="configuration-mode"
                classList={{
                  "configuration-mode--selected": mode() === "manual",
                }}
                aria-pressed={mode() === "manual"}
                onClick={() => changeMode("manual")}
              >
                <Icon name="server" />
                <span>
                  <strong>自建 / 域名邮箱</strong>
                  <small>手动填写服务器与登录信息</small>
                </span>
              </button>
            </div>
            <Show when={mode() === "manual"}>
              <p class="field-help manual-help">
                请填写邮箱服务商或管理员提供的配置。收发信可使用同一服务器，登录用户名也可以与邮箱地址不同。
              </p>
            </Show>
            <div class="email-row">
              <label>
                邮箱地址
                <input
                  ref={emailInput}
                  name="email"
                  type="email"
                  required
                  maxlength="254"
                  autocomplete="email"
                  spellcheck={false}
                  placeholder="you@example.com"
                  autofocus={!initial}
                  value={draft().email}
                  onInput={(event) =>
                    update({ email: event.currentTarget.value })
                  }
                  onBlur={() => void discover()}
                />
              </label>
              <Show when={mode() === "automatic"}>
                <button
                  class="button button--secondary"
                  type="button"
                  onClick={() => void discover(true)}
                  disabled={
                    !desktopAvailable || !draft().email || busy() === "discover"
                  }
                >
                  自动配置
                </button>
              </Show>
            </div>
            <Show when={discovered()}>
              {(config) => (
                <div
                  class={`provider-note ${config().source === "guess" ? "provider-note--guess" : ""}`}
                >
                  <strong>{config().provider ?? "自定义邮箱"}</strong>
                  <p>
                    {config().note ??
                      "已填写该服务商的服务器配置，你也可以手动调整。"}
                  </p>
                  <Show when={config().auth === "oauth2"}>
                    <p>
                      该服务商推荐官方授权登录，目前尚未接入。若你的账户支持应用专用密码，可使用下方表单；仅允许官方授权的账户暂时无法连接。
                    </p>
                  </Show>
                </div>
              )}
            </Show>
            <div class="display-name-field">
              <label>
                显示名称 <span class="optional">选填</span>
                <input
                  name="displayName"
                  autocomplete="name"
                  maxlength="100"
                  placeholder="你的姓名或账户备注"
                  value={draft().displayName}
                  onInput={(event) =>
                    update({ displayName: event.currentTarget.value })
                  }
                />
              </label>
            </div>
            <p class="field-help">
              {mode() === "manual"
                ? "服务器地址应与 TLS 证书中的主机名一致；端口请以服务商提供的配置为准。"
                : "QQ、网易等邮箱请先开启 IMAP / SMTP，再使用授权码登录。"}
            </p>
            <div class="servers-grid">
              {serverFields("incoming", "收信服务器", "IMAP")}
              {serverFields("outgoing", "发信服务器", "SMTP")}
            </div>
          </fieldset>
          <div class="credential-note">
            <Icon name="shield" />
            <span>凭据保存在系统凭据管理器中；仅使用加密连接。</span>
          </div>
          <div aria-live="polite" aria-atomic="true">
            <Show when={busy()}>
              <p class="working-state">
                <span class="spinner" />
                {busy() === "discover"
                  ? "正在识别服务器配置…"
                  : busy() === "test"
                    ? "正在分别验证 IMAP 和 SMTP 登录，请稍候…"
                    : "正在验证连接并安全保存账户，请稍候…"}
              </p>
            </Show>
            <Show when={result()}>
              {(report) => (
                <div class="connection-results">
                  <div
                    class={
                      report().incoming.success
                        ? "check-result check-result--success"
                        : "check-result check-result--error"
                    }
                  >
                    <strong>IMAP 收信</strong>
                    <span>{report().incoming.message}</span>
                  </div>
                  <div
                    class={
                      report().outgoing.success
                        ? "check-result check-result--success"
                        : "check-result check-result--error"
                    }
                  >
                    <strong>SMTP 发信</strong>
                    <span>{report().outgoing.message}</span>
                  </div>
                </div>
              )}
            </Show>
          </div>
          <Show when={error()}>
            <p class="error-message" role="alert">
              {error()}
            </p>
          </Show>
        </div>
        <footer class="dialog-footer">
          <span>保存时会再次验证收发信连接</span>
          <div class="button-group">
            <button
              class="button button--secondary"
              type="button"
              disabled={!!busy() || !desktopAvailable}
              onClick={() => void submit("test")}
            >
              测试连接
            </button>
            <button
              class="button button--primary"
              type="submit"
              disabled={!!busy() || !desktopAvailable}
            >
              {busy() === "save" ? "验证并保存中…" : "验证并保存"}
            </button>
          </div>
        </footer>
      </form>
    </dialog>
  );
}
