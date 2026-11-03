/** Development-only IPC fixture. This entry is not imported by the production app. */
import { render } from "solid-js/web";
import type {
  Account,
  AccountInput,
  ComposeMessage,
  MailFolder,
  MessageDetail,
  MessagePage,
  MessageSummary,
} from "../src/lib/types";
import "../src/styles/global.css";
import { completeHtmlFixture, inlineResources } from "./mail-render-fixture";

const clone = <T,>(value: T): T => structuredClone(value);
const pause = (milliseconds: number) => new Promise<void>((resolve) => setTimeout(resolve, milliseconds));
const PAGE_SIZE = 2; // A small page exposes the real component's "load more" flow.
const root = document.getElementById("root")!;
const stateOutput = document.getElementById("fixture-state")!;
const countsOutput = document.getElementById("fixture-counts")!;
const logOutput = document.getElementById("fixture-log")!;
const networkButton = document.getElementById("fixture-network")!;
const calls = new Map<string, number>();
const history: string[] = [];
const drafts = new Map<string, ComposeMessage>();
const bodies = new Map<string, MessageDetail>();
const cache = new Map<string, { generation: number; messages: MessageSummary[] }>();
const folderCache = new Map<string, MailFolder[]>();
let online = true;
let delayBody = false;
let activeAccount = "fixture-work";
let lastSent: { accountId: string; message: ComposeMessage; messageId: string } | null = null;
let sentCount = 0;
let oauthCancel: (() => void) | undefined;
let dispose: (() => void) | undefined;

const accounts: Account[] = [
  { id: "fixture-work", email: "alice@example.test", displayName: "模拟工作邮箱", incomingUsername: "DOMAIN\\alice", outgoingUsername: "smtp-alice", useSamePassword: false },
  { id: "fixture-personal", email: "bob@example.test", displayName: "模拟个人邮箱", incomingUsername: "bob", outgoingUsername: "bob", useSamePassword: true },
].map((account) => ({
  ...account,
  incomingProtocol: "imap",
  incoming: { host: "mail.example.test", port: 993, encryption: "tls" },
  outgoing: { host: "mail.example.test", port: 587, encryption: "starttls" },
  auth: "password",
}));

const folders: MailFolder[] = [
  { name: "INBOX", displayName: "收件箱", selectable: true, kind: "inbox" },
  { name: "Sent", displayName: "已发送", selectable: true, kind: "sent" },
];
interface FixtureMailbox { generation: number; incoming: MessageDetail[]; sent: MessageDetail[] }
const mailboxes = new Map<string, FixtureMailbox>();
const boxKey = (id: string, folder: string) => `${id}|${folder}`;
const bodyKey = (id: string, folder: string, uid: number, validity: number) => `${boxKey(id, folder)}|${validity}|${uid}`;

const hostileHtml = `<!doctype html><html><head>
<style>@import url('https://tracker.invalid/test.css'); body { background-image: url('https://tracker.invalid/background'); }</style>
<link rel="stylesheet" href="https://tracker.invalid/style.css"></head><body>
<h2>项目进度 · HTML 与附件安全样例</h2>
<p>这是应该保留的中文正文，包含 <strong>加粗</strong>、<em>强调</em> 和普通表格。</p>
<table><tr><th>任务</th><th>进度</th></tr><tr><td>自建邮箱收发</td><td>正在验证</td></tr></table>
<script>parent.document.getElementById('fixture-script-status').textContent='危险：脚本执行';</script>
<img src="https://tracker.invalid/pixel" onerror="alert('fixture')" alt="远程追踪图片应被移除">
<svg onload="alert('fixture')"><a href="javascript:alert('fixture')"><text>SVG 活动内容</text></a></svg>
<form action="https://tracker.invalid/submit"><input name="secret"><button>不应出现的表单</button></form>
<a href="javascript:alert('fixture')">危险链接应只保留文字</a>
<iframe src="https://tracker.invalid/frame"></iframe>
<p style="background:url(https://tracker.invalid/style-pixel)">行内远程样式应被移除。</p>
<p>请使用下方测试附件按钮；夹具只模拟用户取消保存。</p></body></html>`.replace(/https:\/\/tracker\.invalid\//g, `${location.origin}/tests/fixture-blocked/`);

function seedDetails(account: Account, generation: number): MessageDetail[] {
  return [
    { uid: 104, subject: "图文邮件完整渲染样例", body: "完整图文排版样例：邮件头 CSS、响应式表格、CID 内嵌图、Content-Location、背景图和本机 HTTP 图片。", html: completeHtmlFixture(location.origin), attached: true },
    { uid: 103, subject: "项目进度与附件 · HTML 安全样例", body: "项目进度：自建邮箱收发正在验证。此邮件包含 HTML 和一个测试附件。", html: hostileHtml, attached: true },
    { uid: 102, subject: "周会纪要 · 纯文本阅读", body: "这是一封纯文本模拟邮件。\n\n1. 检查账户切换。\n2. 检查回复与全部回复。\n3. 离线时已缓存正文仍可阅读。\n\n所有地址均使用 example.test。", html: null, attached: false },
    { uid: 101, subject: "历史邮件 · 加载更多", body: "第三封模拟邮件，用于验证 UID 分页与延迟响应。", html: null, attached: false },
  ].map((fixture) => ({
    summary: {
      uid: fixture.uid, uidValidity: generation,
      subject: `${account.id === "fixture-personal" ? "个人 · " : ""}${fixture.subject}`,
      from: "测试同事 <colleague@example.test>", to: [account.email, "team@example.test"],
      date: `2026-11-02T0${fixture.uid - 98}:30:00Z`,
      unread: true, flagged: fixture.uid === 103, size: fixture.attached ? 4096 : 600,
      snippet: fixture.body.slice(0, 70), hasAttachments: fixture.attached,
    },
    cc: ["observer@example.test"], replyTo: ["reply@example.test"],
    messageId: `fixture-${fixture.uid}@example.test`, references: ["earlier@example.test"],
    textBody: fixture.body, htmlBody: fixture.html,
    inlineResources: fixture.uid === 104 ? clone(inlineResources) : [],
    htmlBaseUrl: fixture.uid === 104 ? `${location.origin}/tests/mail-assets/newsletter.html` : null,
    attachments: fixture.uid === 104 ? [
      { index: 0, name: "横幅.png", mimeType: "image/png", size: 12000 },
      { index: 1, name: "配图.png", mimeType: "image/png", size: 500 },
      { index: 2, name: "背景.png", mimeType: "image/png", size: 300 },
      { index: 3, name: "项目说明.txt", mimeType: "text/plain", size: 48 },
    ] : fixture.attached ? [{ index: 0, name: "项目说明.txt", mimeType: "text/plain", size: 48 }] : [],
  }));
}

function seedAccount(account: Account, generation = 77) {
  const incoming = seedDetails(account, generation);
  mailboxes.set(account.id, { generation, incoming, sent: [] });
  folderCache.set(account.id, clone(folders));
  cache.set(boxKey(account.id, "INBOX"), { generation, messages: incoming.map((detail) => clone(detail.summary)) });
  cache.set(boxKey(account.id, "Sent"), { generation, messages: [] });
  // Two messages are already cached so offline QA can start without network actions.
  for (const detail of incoming.slice(0, 2)) bodies.set(bodyKey(account.id, "INBOX", detail.summary.uid, generation), clone(detail));
}
for (const account of accounts) seedAccount(account);

function updateDiagnostics(note?: string) {
  if (note) {
    history.unshift(note);
    history.splice(25);
  }
  const mailbox = mailboxes.get(activeAccount);
  stateOutput.textContent = `状态：${online ? "模拟在线" : "模拟离线"} · 当前测试账户：${accounts.find((account) => account.id === activeAccount)?.displayName ?? activeAccount} · UIDVALIDITY：${mailbox?.generation ?? "—"} · 收件箱 ${mailbox?.incoming.length ?? 0} 封 · 正文延迟 ${delayBody ? "4 秒" : "0.15 秒"}。控制后请在应用内同步或切换文件夹。`;
  networkButton.textContent = online ? "模拟离线" : "恢复在线";
  countsOutput.textContent = `IPC 调用计数：${[...calls.values()].reduce((sum, count) => sum + count, 0)} · 模拟发送 ${sentCount} 次 · 草稿 ${drafts.size} 份 · ${[...calls].map(([command, count]) => `${command}:${count}`).join(" / ")}`;
  logOutput.textContent = JSON.stringify({
    lastSent,
    drafts: [...drafts].map(([accountId, draft]) => ({ accountId, subject: draft.subject, bodyLength: draft.body.length })),
    history,
  }, null, 2);
}

function assertOnline() {
  if (!online) throw new Error("模拟离线：无法连接邮件服务器，本地缓存和草稿仍可使用。");
}
function requireBox(id: string) {
  const mailbox = mailboxes.get(id);
  if (!mailbox) throw new Error("模拟账户不存在。");
  return mailbox;
}
function source(id: string, folder: string): MessageDetail[] {
  const mailbox = requireBox(id);
  if (folder === "INBOX") return mailbox.incoming;
  if (folder === "Sent") return mailbox.sent;
  throw new Error("模拟文件夹不存在。");
}
function page(messages: MessageSummary[], generation: number | null, before?: number): MessagePage {
  const eligible = messages.filter((message) => before === undefined || message.uid < before).sort((a, b) => b.uid - a.uid);
  return { messages: clone(eligible.slice(0, PAGE_SIZE)), hasMore: eligible.length > PAGE_SIZE, uidValidity: generation };
}
function remoteDetail(id: string, folder: string, uid: number, validity: number) {
  const mailbox = requireBox(id);
  if (mailbox.generation !== validity) throw new Error("服务器 UIDVALIDITY 已改变，请重新同步文件夹。");
  const detail = source(id, folder).find((detail) => detail.summary.uid === uid);
  if (!detail) throw new Error("模拟邮件已被删除，请同步文件夹。");
  return detail;
}

type Args = Record<string, unknown>;
async function mockInvoke(command: string, args: Args = {}): Promise<unknown> {
  calls.set(command, (calls.get(command) ?? 0) + 1);
  const accountId = String(args.accountId ?? "");
  if (accountId && accounts.some((account) => account.id === accountId)) activeAccount = accountId;
  updateDiagnostics(`${command}${accountId ? ` · ${accountId}` : ""}`);
  const folder = String(args.folder ?? "INBOX");
  const uid = Number(args.uid);
  const validity = Number(args.uidValidity);
  const before = args.beforeUid == null ? undefined : Number(args.beforeUid);
  try {
    switch (command) {
      case "app_version": return "WispMail · 本地模拟 QA";
      case "list_accounts": return clone(accounts);
      case "list_cached_folders": return clone(folderCache.get(accountId) ?? []);
      case "refresh_folders": assertOnline(); folderCache.set(accountId, clone(folders)); return clone(folders);
      case "list_cached_messages": {
        const saved = cache.get(boxKey(accountId, folder));
        return page(saved?.messages ?? [], saved?.generation ?? null, before);
      }
      case "sync_folder": {
        assertOnline();
        const mailbox = requireBox(accountId);
        const saved = cache.get(boxKey(accountId, folder));
        const expected = args.expectedUidValidity ?? saved?.generation;
        const reset = before !== undefined && expected !== mailbox.generation;
        const summaries = source(accountId, folder).map((detail) => clone(detail.summary));
        cache.set(boxKey(accountId, folder), { generation: mailbox.generation, messages: summaries });
        if (saved?.generation !== mailbox.generation) {
          for (const key of bodies.keys()) if (key.startsWith(`${boxKey(accountId, folder)}|`)) bodies.delete(key);
        }
        return page(summaries, mailbox.generation, reset ? undefined : before);
      }
      case "get_message": {
        const key = bodyKey(accountId, folder, uid, validity);
        let detail = bodies.get(key);
        if (!detail) {
          assertOnline();
          detail = clone(remoteDetail(accountId, folder, uid, validity));
          bodies.set(key, detail);
        }
        const snapshot = clone(detail);
        await pause(delayBody ? 4000 : 150);
        return snapshot;
      }
      case "set_message_read": {
        assertOnline();
        const detail = remoteDetail(accountId, folder, uid, validity);
        detail.summary.unread = !args.read;
        const saved = cache.get(boxKey(accountId, folder));
        const summary = saved?.messages.find((message) => message.uid === uid && message.uidValidity === validity);
        if (summary) summary.unread = detail.summary.unread;
        const cachedBody = bodies.get(bodyKey(accountId, folder, uid, validity));
        if (cachedBody) cachedBody.summary.unread = detail.summary.unread;
        return null;
      }
      case "set_message_flags": {
        assertOnline();
        for (const id of args.uids as number[]) {
          const detail = remoteDetail(accountId, folder, id, validity);
          if (args.flag === "flagged") detail.summary.flagged = Boolean(args.value);
          else detail.summary.unread = !args.value;
          const summary = cache.get(boxKey(accountId, folder))?.messages.find((message) => message.uid === id && message.uidValidity === validity);
          if (summary) Object.assign(summary, { flagged: detail.summary.flagged, unread: detail.summary.unread });
          const cachedBody = bodies.get(bodyKey(accountId, folder, id, validity));
          if (cachedBody) cachedBody.summary = clone(detail.summary);
        }
        return null;
      }
      case "organize_messages": {
        assertOnline();
        const action = args.action as { type: string; target?: string };
        const uids = args.uids as number[];
        for (const id of uids) remoteDetail(accountId, folder, id, validity);
        // 夹具只有 INBOX 与 Sent，没有废纸篓：删除需要确认后永久删除。
        if (action.type === "trash") return { target: null, needsPermanentDelete: true };
        const list = source(accountId, folder);
        const moved = list.filter((detail) => uids.includes(detail.summary.uid));
        const kept = list.filter((detail) => !uids.includes(detail.summary.uid));
        const mailbox = requireBox(accountId);
        if (folder === "INBOX") mailbox.incoming = kept; else mailbox.sent = kept;
        const target = action.type === "move" ? action.target! : action.type === "archive" ? "Sent" : null;
        if (target === "Sent" && folder !== "Sent") mailbox.sent.unshift(...moved);
        if (target === "INBOX" && folder !== "INBOX") mailbox.incoming.unshift(...moved);
        const saved = cache.get(boxKey(accountId, folder));
        if (saved) saved.messages = saved.messages.filter((message) => !uids.includes(message.uid));
        return { target, needsPermanentDelete: false };
      }
      case "save_attachment": return null; // Simulate cancellation, never write a local file.
      case "search_messages": {
        // Substring search over the cached summaries and downloaded bodies, like the backend index.
        const terms = String(args.query ?? "").toLowerCase().split(/\s+/).filter(Boolean);
        const hits: { folder: string; summary: MessageSummary }[] = [];
        for (const name of ["INBOX", "Sent"]) {
          for (const summary of cache.get(boxKey(accountId, name))?.messages ?? []) {
            const body = bodies.get(bodyKey(accountId, name, summary.uid, summary.uidValidity))?.textBody ?? summary.snippet;
            const text = [summary.from, summary.to.join(", "), summary.subject, body].join(" ").toLowerCase();
            if (terms.every((term) => text.includes(term))) hits.push({ folder: name, summary: clone(summary) });
          }
        }
        return hits.sort((a, b) => b.summary.date.localeCompare(a.summary.date));
      }
      case "estimate_forward_size": return 48_000 + uid * 1000;
      case "forward_attachments": {
        const detail = bodies.get(bodyKey(accountId, folder, uid, validity)) ?? remoteDetail(accountId, folder, uid, validity);
        const wanted = new Set(args.indices as number[]);
        return {
          attachments: detail.attachments.filter((attachment) => wanted.has(attachment.index))
            .map((attachment) => ({ name: attachment.name, mimeType: attachment.mimeType, dataBase64: btoa("fixture") })),
          skipped: [],
        };
      }
      case "open_email_link": updateDiagnostics(`模拟外部链接（未打开）：${String(args.url ?? "")}`); return null;
      case "get_draft": return clone(drafts.get(accountId) ?? null);
      case "save_draft": drafts.set(accountId, clone(args.draft as ComposeMessage)); return null;
      case "clear_draft": drafts.delete(accountId); return null;
      case "send_message": {
        assertOnline();
        const message = clone(args.message as ComposeMessage);
        if (!message.to.length && !message.cc.length && !message.bcc.length) throw new Error("模拟发送也需要至少一个收件人。");
        const messageId = `fixture-send-${++sentCount}@example.test`;
        lastSent = { accountId, message, messageId };
        if (JSON.stringify(drafts.get(accountId)) === JSON.stringify(message)) drafts.delete(accountId);
        const mailbox = requireBox(accountId);
        const summary: MessageSummary = {
          uid: 200 + sentCount, uidValidity: mailbox.generation, from: accounts.find((account) => account.id === accountId)!.email,
          to: message.to, date: "2026-11-02T09:00:00Z", subject: message.subject, snippet: message.body.slice(0, 70),
          unread: false, flagged: false, size: message.body.length, hasAttachments: !!message.attachments.length,
        };
        mailbox.sent.unshift({ summary, cc: message.cc, replyTo: [], messageId, references: message.references, textBody: message.body, htmlBody: null, inlineResources: [], htmlBaseUrl: null,
          attachments: message.attachments.map((attachment, index) => ({ index, name: attachment.name, mimeType: attachment.mimeType, size: Math.floor(attachment.dataBase64.length * 3 / 4) })) });
        return { messageId, warning: "本地模拟发送完成；未连接真实邮箱，也未发送真实邮件。" };
      }
      case "discover_config": {
        const email = String(args.email ?? "");
        if (!email.includes("@")) return null;
        const builtin: Record<string, [string, string, string]> = { "qq.com": ["QQ 邮箱", "imap.qq.com", "smtp.qq.com"], "163.com": ["网易 163 邮箱", "imap.163.com", "smtp.163.com"] };
        if (email.toLowerCase().endsWith("@outlook.com")) return { provider: "Outlook", source: "builtin", incomingProtocol: "imap", incoming: { host: "outlook.office365.com", port: 993, encryption: "tls" }, outgoing: { host: "smtp-mail.outlook.com", port: 587, encryption: "starttls" }, auth: "oauth2", note: null };
        const known = builtin[email.split("@")[1].toLowerCase()];
        if (known) return { provider: known[0], source: "builtin", incomingProtocol: "imap", incoming: { host: known[1], port: 993, encryption: "tls" }, outgoing: { host: known[2], port: 465, encryption: "tls" }, auth: "password", note: "需先在网页版邮箱设置中开启 IMAP/SMTP 服务，并使用生成的授权码代替登录密码。" };
        return { provider: "本地模拟服务商", source: "guess", incomingProtocol: "imap", incoming: { host: "mail.example.test", port: 993, encryption: "tls" }, outgoing: { host: "mail.example.test", port: 587, encryption: "starttls" }, auth: "password", note: "这是模拟配置，不会连接服务器。" };
      }
      case "test_account_connection": assertOnline(); return { incoming: { success: true, message: "模拟 IMAP 验证成功" }, outgoing: { success: true, message: "模拟 SMTP 验证成功" } };
      case "save_account": {
        assertOnline();
        const input = args.input as AccountInput;
        // Explicitly select public fields: password fields are never retained or logged.
        const account: Account = { id: input.id || `fixture-${accounts.length + 1}`, email: input.email, displayName: input.displayName, incoming: clone(input.incoming), outgoing: clone(input.outgoing), incomingProtocol: input.incomingProtocol, incomingUsername: input.incomingUsername || input.email, outgoingUsername: input.outgoingUsername || input.email, useSamePassword: input.useSamePassword, auth: input.auth };
        const index = accounts.findIndex((item) => item.id === account.id);
        if (index >= 0) accounts[index] = account; else { accounts.push(account); seedAccount(account); }
        return clone(account);
      }
      case "oauth_providers": return [{ id: "microsoft", name: "Microsoft", configured: true }];
      case "cancel_oauth_sign_in": { oauthCancel?.(); return null; }
      case "sign_in_oauth": {
        assertOnline();
        // 模拟浏览器登录：稍后返回账户，期间可取消。
        await new Promise<void>((resolve, reject) => { const timer = setTimeout(resolve, 700); oauthCancel = () => { clearTimeout(timer); reject("已取消登录。"); }; });
        oauthCancel = undefined;
        const existing = accounts.find((item) => item.id === args.accountId);
        const email = existing?.email ?? `fixture-ms${accounts.length + 1}@outlook.com`;
        const account: Account = { id: existing?.id ?? `fixture-${accounts.length + 1}`, email, displayName: existing?.displayName ?? "模拟微软用户", incoming: { host: "outlook.office365.com", port: 993, encryption: "tls" }, outgoing: { host: "smtp-mail.outlook.com", port: 587, encryption: "starttls" }, incomingProtocol: "imap", incomingUsername: email, outgoingUsername: email, useSamePassword: true, auth: "oauth2" };
        if (existing) accounts[accounts.indexOf(existing)] = account; else { accounts.push(account); seedAccount(account); }
        return clone(account);
      }
      case "delete_account": {
        const id = String(args.id);
        const index = accounts.findIndex((account) => account.id === id);
        if (index >= 0) accounts.splice(index, 1);
        mailboxes.delete(id); drafts.delete(id); folderCache.delete(id);
        for (const key of [...cache.keys()]) if (key.startsWith(`${id}|`)) cache.delete(key);
        for (const key of [...bodies.keys()]) if (key.startsWith(`${id}|`)) bodies.delete(key);
        return null;
      }
      case "plugin:event|listen": {
        const list = listeners.get(String(args.event)) ?? [];
        list.push(Number(args.handler));
        listeners.set(String(args.event), list);
        return Number(args.handler);
      }
      case "plugin:event|unlisten": return null;
      default: throw new Error(`UI 夹具尚未实现命令：${command}`);
    }
  } finally { updateDiagnostics(); }
}

// Minimal event bridge: listen() registers a transformCallback id; emitMock() calls it like the backend would.
const callbacks = new Map<number, (message: unknown) => void>();
const listeners = new Map<string, number[]>();
let nextCallback = 1;
function transformCallback(callback: (message: unknown) => void) { const id = nextCallback++; callbacks.set(id, callback); return id; }
function emitMock(event: string, payload: unknown) { for (const id of listeners.get(event) ?? []) callbacks.get(id)?.({ event, id, payload }); }

// @tauri-apps/api/core.js checks globalThis.isTauri and delegates invoke directly here.
Object.assign(window, {
  isTauri: true,
  __TAURI_INTERNALS__: { invoke: mockInvoke, transformCallback },
  // unlisten() in @tauri-apps/api/event calls this before invoking plugin:event|unlisten.
  __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: (event: string, id: number) => { listeners.set(event, (listeners.get(event) ?? []).filter((item) => item !== id)); } },
});
document.getElementById("fixture-arrive")!.addEventListener("click", () => {
  // Same effect as the backend watcher: the server gets a message, the cache is synced, then mail-changed fires.
  const mailbox = requireBox(activeAccount);
  const uid = Math.max(100, ...mailbox.incoming.map((detail) => detail.summary.uid)) + 1;
  const template = clone(mailbox.incoming[mailbox.incoming.length - 1] ?? seedDetails(accounts.find((account) => account.id === activeAccount)!, mailbox.generation)[3]);
  template.summary = { ...template.summary, uid, uidValidity: mailbox.generation, unread: true, subject: `实时到达 · 第 ${uid} 号邮件`, snippet: "这封邮件由“模拟新邮件到达”生成。", date: new Date().toISOString() };
  template.messageId = `live-${uid}@example.test`;
  mailbox.incoming.unshift(template);
  const saved = cache.get(boxKey(activeAccount, "INBOX"));
  if (saved && saved.generation === mailbox.generation) saved.messages.unshift(clone(template.summary));
  emitMock("mail-changed", { accountId: activeAccount, folder: "INBOX", newCount: 1 });
  updateDiagnostics(`控制：模拟新邮件 UID ${uid} 到达并发出 mail-changed`);
});
document.getElementById("fixture-network")!.addEventListener("click", () => { online = !online; updateDiagnostics(online ? "控制：恢复在线" : "控制：模拟离线"); });
document.getElementById("fixture-delay")!.addEventListener("change", (event) => { delayBody = (event.currentTarget as HTMLInputElement).checked; updateDiagnostics("控制：切换正文延迟"); });
document.getElementById("fixture-generation")!.addEventListener("click", () => {
  const mailbox = requireBox(activeAccount);
  mailbox.generation++;
  mailbox.incoming = seedDetails(accounts.find((account) => account.id === activeAccount)!, mailbox.generation);
  for (const detail of mailbox.incoming) detail.summary.subject = `新一代 ${mailbox.generation} · ${detail.summary.subject}`;
  for (const detail of mailbox.sent) detail.summary.uidValidity = mailbox.generation;
  updateDiagnostics("控制：服务器 UIDVALIDITY 改变；缓存保留旧代，便于验证加载更多重置");
});
document.getElementById("fixture-empty")!.addEventListener("click", () => { requireBox(activeAccount).incoming = []; updateDiagnostics("控制：服务器收件箱已清空；请在应用内同步"); });
document.getElementById("fixture-restore")!.addEventListener("click", () => {
  const mailbox = requireBox(activeAccount);
  mailbox.incoming = seedDetails(accounts.find((account) => account.id === activeAccount)!, mailbox.generation);
  updateDiagnostics("控制：服务器模拟邮件已恢复；请在应用内同步");
});

const { default: App } = await import("../src/App");
function mount() { dispose?.(); root.replaceChildren(); dispose = render(() => <App />, root); updateDiagnostics("控制：真实 App 组件已挂载"); }
document.getElementById("fixture-reload")!.addEventListener("click", mount);
mount();
