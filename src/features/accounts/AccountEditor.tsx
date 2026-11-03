import {
  createMemo,
  createSignal,
  For,
  onCleanup,
  onMount,
  Show,
} from "solid-js";
import {
  cancelOAuthSignIn,
  desktopAvailable,
  discoverConfig,
  errorMessage,
  oauthProviders,
  openEmailLink,
  saveAccount,
  signInOAuth,
  testAccountConnection,
} from "../../lib/api";
import type {
  Account,
  AccountInput,
  ConnectionReport,
  DiscoveredConfig,
  OAuthProviderInfo,
  ServerConfig,
} from "../../lib/types";
import { Icon } from "../../ui/Icon";
import {
  accountInputForSubmission,
  authCodeSubmission,
  createAccountDraft,
  createDiscoveryGuard,
  requiresOutgoingPassword,
  initialModeFor,
  oauthEditSubmission,
  setPasswordSharing,
  shouldDiscoverConfig,
  type ConfigurationMode,
} from "./accountForm";
import {
  AUTH_CODE_PROVIDERS,
  authCodeProviderFor,
  emailForProvider,
  type AuthCodeProvider,
} from "./authCodeProviders";

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
    initialModeFor(initial) ?? props.initialMode ?? "automatic",
  );
  /** 授权码邮箱：按地址域名识别；尚未输入地址时使用点选的服务商。 */
  const [chosenProvider, setChosenProvider] = createSignal<AuthCodeProvider>();
  const detectedProvider = createMemo(() => authCodeProviderFor(draft().email));
  const guide = () => detectedProvider() ?? chosenProvider();
  const [discovered, setDiscovered] = createSignal<DiscoveredConfig>();
  const [lastDiscoveredEmail, setLastDiscoveredEmail] = createSignal(
    initial?.email ?? "",
  );
  const [busy, setBusy] = createSignal<
    "discover" | "test" | "save" | "signin" | null
  >(null);
  /** 微软账户登录；undefined 表示尚未从后端读取。 */
  const [microsoft, setMicrosoft] = createSignal<OAuthProviderInfo>();
  const newOAuthAccount = () => mode() === "oauth" && !initial;
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

  onMount(() => {
    dialog.showModal();
    if (desktopAvailable)
      void oauthProviders()
        .then((providers) => {
          if (alive)
            setMicrosoft(
              providers.find((provider) => provider.id === "microsoft"),
            );
        })
        .catch(() => undefined);
  });
  onCleanup(() => {
    alive = false;
    discovery.invalidate();
    clearSecrets();
    if (busy() === "signin") void cancelOAuthSignIn().catch(() => undefined);
  });

  function close() {
    if (busy() === "test" || busy() === "save") return;
    // 关闭窗口即放弃浏览器登录，不会保存账户。
    if (busy() === "signin") void cancelOAuthSignIn().catch(() => undefined);
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
    if (next === "authcode") {
      // 授权码邮箱用邮箱地址登录，收发信共用同一个授权码。
      if (outgoingPasswordInput) outgoingPasswordInput.value = "";
      setDraft((current) => ({
        ...current,
        incomingUsername: "",
        outgoingUsername: "",
        useSamePassword: true,
        outgoingPassword: "",
      }));
      queueMicrotask(() => void discover());
    }
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
      if (!alive || !discovery.canApply(token) || mode() === "manual") return;
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
      if (alive && discovery.canApply(token) && mode() !== "manual")
        setError(errorMessage(error));
    } finally {
      if (alive && discovery.isLatest(token) && busy() === "discover")
        setBusy(null);
    }
  }

  function chooseProvider(provider: AuthCodeProvider) {
    setChosenProvider(provider);
    const email = emailForProvider(draft().email, provider);
    if (email && email !== draft().email) {
      update({ email });
      queueMicrotask(() => void discover());
    } else emailInput?.focus();
  }

  async function signIn() {
    if (busy() || !desktopAvailable) return;
    setBusy("signin");
    setError("");
    setResult(undefined);
    try {
      const account = await signInOAuth("microsoft", initial?.id);
      if (alive) {
        dialog.close();
        props.onSaved(account);
      }
    } catch (error) {
      if (alive) setError(errorMessage(error));
    } finally {
      if (alive) setBusy(null);
    }
  }

  async function submit(action: "test" | "save") {
    if (busy()) return;
    if (newOAuthAccount()) {
      if (action === "save") await signIn();
      return;
    }
    if (mode() === "authcode") {
      const email = draft().email.trim();
      if (email && !authCodeProviderFor(email)) {
        setError(
          "这不是 QQ 邮箱或网易邮箱的地址。其他邮箱请选择上方的「常见邮箱」或「自建 / 域名邮箱」。",
        );
        return;
      }
      // 服务器配置不在表单中显示：提交前确保已按当前地址自动填写。
      if (email && (lastDiscoveredEmail() !== email || !draft().incoming.host))
        await discover(true);
    }
    if (!form.reportValidity()) return;
    if (mode() === "authcode" && !draft().incoming.host) return;
    const input =
      mode() === "authcode"
        ? authCodeSubmission(draft())
        : mode() === "oauth"
          ? oauthEditSubmission(draft())
          : accountInputForSubmission(draft());
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

  function authCodeFields() {
    const encryptionLabel = (server: ServerConfig) =>
      server.encryption === "starttls" ? "STARTTLS" : "SSL";
    return (
      <>
        <div class="auth-providers" role="group" aria-label="选择邮箱服务商">
          <For each={AUTH_CODE_PROVIDERS}>
            {(provider) => (
              <button
                type="button"
                class="auth-provider"
                classList={{
                  "auth-provider--selected": guide()?.id === provider.id,
                }}
                aria-pressed={guide()?.id === provider.id}
                onClick={() => chooseProvider(provider)}
              >
                {provider.name}
              </button>
            )}
          </For>
        </div>
        <div class="auth-fields">
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
              placeholder={
                guide()
                  ? `name@${guide()!.domains[0]}`
                  : "name@qq.com 或 name@163.com"
              }
              autofocus={!initial}
              value={draft().email}
              onInput={(event) => update({ email: event.currentTarget.value })}
              onBlur={() => void discover()}
            />
          </label>
          <Show when={draft().email.includes("@") && !detectedProvider()}>
            <p class="field-help field-help--warning">
              这不是 QQ
              邮箱或网易邮箱的地址。其他邮箱请选择上方的「常见邮箱」或「自建 /
              域名邮箱」。
            </p>
          </Show>
          <Show when={guide()}>
            {(provider) => (
              <div class="auth-guide">
                <strong>如何获取{provider().name}授权码</strong>
                <ol>
                  <For each={provider().steps}>{(step) => <li>{step}</li>}</For>
                </ol>
                <button
                  type="button"
                  class="button button--secondary"
                  disabled={!desktopAvailable}
                  onClick={() =>
                    void openEmailLink(provider().webUrl).catch((error) =>
                      setError(errorMessage(error)),
                    )
                  }
                >
                  在浏览器中打开{provider().name}
                </button>
              </div>
            )}
          </Show>
          <label class="incoming-password">
            授权码{" "}
            <span class="optional">{initial ? "留空保留原授权码" : ""}</span>
            <input
              ref={passwordInput}
              name="password"
              type="password"
              required={!initial}
              maxlength="4096"
              autocomplete="new-password"
              spellcheck={false}
              placeholder={
                initial
                  ? "已安全保存，输入可替换"
                  : (guide()?.codeHint ?? "邮箱生成的授权码，不是登录密码")
              }
              value={draft().password ?? ""}
              onInput={(event) =>
                update({ password: event.currentTarget.value })
              }
            />
          </label>
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
          <Show when={detectedProvider() && draft().incoming.host}>
            <p class="server-summary">
              <Icon name="server" />
              <span>
                已自动填写服务器：收信 {draft().incoming.host}:
                {draft().incoming.port}（{encryptionLabel(draft().incoming)}）·
                发信 {draft().outgoing.host}:{draft().outgoing.port}（
                {encryptionLabel(draft().outgoing)}）
              </span>
            </p>
          </Show>
        </div>
      </>
    );
  }

  function oauthFields() {
    const configured = () => microsoft()?.configured ?? false;
    return (
      <div class="auth-fields oauth-fields">
        <div class="auth-guide">
          <strong>使用微软账户登录</strong>
          <p>
            适用于 Outlook.com、Hotmail、Live 邮箱，以及 Microsoft 365
            工作或学校账户。
          </p>
          <ol>
            <li>点击下方按钮，在系统浏览器中打开微软登录页面。</li>
            <li>登录账户，并同意 WispMail 读取和发送你的邮件。</li>
            <li>浏览器显示“登录完成”后回到 WispMail，账户会自动验证并保存。</li>
          </ol>
          <p class="field-help">
            WispMail
            不会看到你的微软密码，只保存微软颁发的授权；你可以随时在微软账户的“隐私”设置中撤销。
          </p>
        </div>
        <Show when={initial}>
          {(account) => (
            <>
              <label>
                邮箱地址
                <input
                  name="email"
                  type="email"
                  readonly
                  value={account().email}
                />
              </label>
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
            </>
          )}
        </Show>
        <Show when={microsoft() && !configured()}>
          <p class="field-help field-help--warning">
            此版本未内置微软应用 ID，暂时无法使用微软账户登录。构建方法见
            docs/OAUTH.md。
          </p>
        </Show>
        <button
          type="button"
          class="button button--primary oauth-button"
          disabled={!desktopAvailable || !configured() || !!busy()}
          onClick={() => void signIn()}
        >
          <Icon name="shield" />
          {initial ? "重新登录微软账户" : "在浏览器中登录微软账户"}
        </button>
      </div>
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
            disabled={
              busy() === "test" || busy() === "save" || busy() === "signin"
            }
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
                  "configuration-mode--selected": mode() === "authcode",
                }}
                aria-pressed={mode() === "authcode"}
                onClick={() => changeMode("authcode")}
              >
                <Icon name="shield" />
                <span>
                  <strong>QQ / 网易邮箱</strong>
                  <small>使用授权码登录</small>
                </span>
              </button>
              <button
                type="button"
                class="configuration-mode"
                classList={{
                  "configuration-mode--selected": mode() === "oauth",
                }}
                aria-pressed={mode() === "oauth"}
                onClick={() => changeMode("oauth")}
              >
                <Icon name="globe" />
                <span>
                  <strong>Outlook / 微软</strong>
                  <small>在浏览器中授权登录</small>
                </span>
              </button>
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
                  <small>手动填写服务器配置</small>
                </span>
              </button>
            </div>
            <Show
              when={mode() === "automatic" || mode() === "manual"}
              fallback={
                <Show when={mode() === "oauth"} fallback={authCodeFields()}>
                  {oauthFields()}
                </Show>
              }
            >
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
                      !desktopAvailable ||
                      !draft().email ||
                      busy() === "discover"
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
                      <Show
                        when={config().provider === "Outlook"}
                        fallback={
                          <p>
                            该服务商推荐官方授权登录，目前尚未接入。若你的账户支持应用专用密码，可使用下方表单；仅允许官方授权的账户暂时无法连接。
                          </p>
                        }
                      >
                        <p>微软邮箱已不再支持密码登录，请使用微软账户授权。</p>
                        <button
                          type="button"
                          class="button button--secondary"
                          onClick={() => changeMode("oauth")}
                        >
                          改用微软账户登录
                        </button>
                      </Show>
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
            </Show>
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
                    : busy() === "signin"
                      ? "请在打开的浏览器窗口中登录并同意授权，完成后会自动验证并保存…"
                      : "正在验证连接并安全保存账户，请稍候…"}
                <Show when={busy() === "signin"}>
                  <button
                    type="button"
                    class="button button--secondary"
                    onClick={() =>
                      void cancelOAuthSignIn().catch((error) =>
                        setError(errorMessage(error)),
                      )
                    }
                  >
                    取消登录
                  </button>
                </Show>
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
          <span>
            {newOAuthAccount()
              ? "登录完成后会自动验证收发信连接"
              : "保存时会再次验证收发信连接"}
          </span>
          <Show when={!newOAuthAccount()}>
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
          </Show>
        </footer>
      </form>
    </dialog>
  );
}
