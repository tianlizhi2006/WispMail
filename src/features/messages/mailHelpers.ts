import type {
  ComposeAttachment,
  ComposeMessage,
  MessageDetail,
  MessagePage,
  MessageSummary,
} from "../../lib/types.ts";

export const MAX_ATTACHMENT_BYTES = 10 * 1024 * 1024;

export function emptyMessage(): ComposeMessage {
  return {
    to: [],
    cc: [],
    bcc: [],
    subject: "",
    body: "",
    inReplyTo: null,
    references: [],
    attachments: [],
  };
}

/** 按引号和尖括号之外的分隔符拆分，保留草稿里未写完的内容。 */
export function splitAddresses(value: string): string[] {
  const result: string[] = [];
  let current = "";
  let quoted = false;
  let escaped = false;
  let angle = 0;
  for (const character of value) {
    if (character === '"' && !escaped) quoted = !quoted;
    if (!quoted && character === "<") angle++;
    if (!quoted && character === ">") angle = Math.max(0, angle - 1);
    if (!quoted && angle === 0 && /[,;\n\r；，]/.test(character)) {
      if (current.trim()) result.push(current.trim());
      current = "";
    } else current += character;
    escaped = character === "\\" && !escaped;
  }
  if (current.trim()) result.push(current.trim());
  return result;
}

export function parseAddresses(values: string[]): {
  addresses: string[];
  invalid: string[];
} {
  const addresses: string[] = [];
  const invalid: string[] = [];
  for (const token of values.flatMap(splitAddresses)) {
    const match = token.match(/^[^<>]*<([^<>]+)>$/);
    const address = (match?.[1] ?? token).trim();
    const parts = address.split("@");
    const local = parts[0];
    const domain = parts[1];
    const valid =
      parts.length === 2 &&
      address.length <= 254 &&
      local.length <= 64 &&
      /^[a-z0-9.!#$%&'*+/=?^_`{|}~-]+$/i.test(local) &&
      !local.startsWith(".") &&
      !local.endsWith(".") &&
      !local.includes("..") &&
      !!domain &&
      domain.includes(".") &&
      domain
        .split(".")
        .every((label) =>
          /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/i.test(label),
        );
    if (!valid) invalid.push(token);
    else if (
      !addresses.some(
        (existing) => existing.toLowerCase() === address.toLowerCase(),
      )
    )
      addresses.push(address);
  }
  return { addresses, invalid };
}

export function prepareSend(draft: ComposeMessage): ComposeMessage {
  const to = parseAddresses(draft.to);
  const cc = parseAddresses(draft.cc);
  const bcc = parseAddresses(draft.bcc);
  if (to.invalid.length || cc.invalid.length || bcc.invalid.length)
    throw new Error("收件人地址尚未填写完整，请检查收件人、抄送和密送。");
  if (to.addresses.length + cc.addresses.length + bcc.addresses.length === 0)
    throw new Error("请至少填写一个收件人。");
  if (
    draft.attachments.reduce(
      (total, file) => total + base64Size(file.dataBase64),
      0,
    ) > MAX_ATTACHMENT_BYTES
  )
    throw new Error("附件总大小不能超过 10 MB。");
  return { ...draft, to: to.addresses, cc: cc.addresses, bcc: bcc.addresses };
}

export function replyMessage(
  detail: MessageDetail,
  ownEmail: string,
  replyAll: boolean,
): ComposeMessage {
  const self = ownEmail.toLowerCase();
  const targets = parseAddresses(
    detail.replyTo.length ? detail.replyTo : [detail.summary.from],
  ).addresses;
  const primary = targets.filter((address) => address.toLowerCase() !== self);
  const originalTo = parseAddresses(detail.summary.to).addresses.filter(
    (address) => address.toLowerCase() !== self,
  );
  const to = [...(primary.length ? primary : originalTo)];
  if (replyAll) to.push(...originalTo);
  const uniqueTo = parseAddresses(to).addresses;
  const cc = replyAll
    ? parseAddresses(detail.cc).addresses.filter(
        (address) =>
          address.toLowerCase() !== self &&
          !uniqueTo.some(
            (target) => target.toLowerCase() === address.toLowerCase(),
          ),
      )
    : [];
  const subject = prefixedSubject(detail.summary.subject, "Re", REPLY_SUBJECT);
  const quote = originalText(detail);
  // mail-parser 返回裸 ID；统一去掉可选尖括号，避免正常回复丢失线程头。
  const normalizeId = (id: string | null): string | null => {
    if (!id || /[\x00-\x20\x7f]/.test(id)) return null;
    const bare = id.startsWith("<") && id.endsWith(">") ? id.slice(1, -1) : id;
    return bare.length <= 996 && /^[^<>\s@]+@[^<>\s@]+$/.test(bare)
      ? bare
      : null;
  };
  const messageId = normalizeId(detail.messageId);
  const references = [
    ...new Set([
      ...detail.references
        .map(normalizeId)
        .filter((id): id is string => id !== null),
      ...(messageId ? [messageId] : []),
    ]),
  ].slice(-100);
  const quoteSender = headerLine(detail.summary.from, 600);
  const quoteDate = quotedDate(detail.summary.date);
  return {
    ...emptyMessage(),
    to: uniqueTo,
    cc,
    subject,
    body: `\n\n在 ${quoteDate || "此前"}，${quoteSender} 写道：\n${quote
      .split(/\r?\n/)
      .map((line) => `> ${line}`)
      .join("\n")}`,
    inReplyTo: messageId,
    references,
  };
}

/** 转发：带原邮件头和正文，附件由调用方从原邮件取出后传入。不设置回复线程头。 */
export function forwardMessage(
  detail: MessageDetail,
  attachments: ComposeAttachment[] = [],
): ComposeMessage {
  const header = [
    `发件人：${headerLine(detail.summary.from, 600) || "未知"}`,
    `日期：${quotedDate(detail.summary.date) || "未知"}`,
    `主题：${headerLine(detail.summary.subject, 900) || "（无主题）"}`,
    `收件人：${headerLine(detail.summary.to.join(", "), 4000) || "未提供"}`,
    ...(detail.cc.length
      ? [`抄送：${headerLine(detail.cc.join(", "), 4000)}`]
      : []),
  ].join("\n");
  return {
    ...emptyMessage(),
    subject: prefixedSubject(detail.summary.subject, "Fwd", FORWARD_SUBJECT),
    body: `\n\n---------- 转发的邮件 ----------\n${header}\n\n${originalText(detail)}`,
    attachments,
  };
}

const REPLY_SUBJECT = /^(re|回复)\s*[:：]/i;
const FORWARD_SUBJECT = /^(fwd?|转发)\s*[:：]/i;

/** 撰写窗口标题：回复带线程头；转发通过主题前缀识别，重新打开草稿时也能保持。 */
export function composeKind(
  message: ComposeMessage,
): "reply" | "forward" | "new" {
  if (message.inReplyTo) return "reply";
  return FORWARD_SUBJECT.test(message.subject) ? "forward" : "new";
}

/** 引用 / 转发头里的完整本地时间；无法解析时保留原文。 */
function quotedDate(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.valueOf())
    ? headerLine(value, 100)
    : date.toLocaleString("zh-CN", {
        year: "numeric",
        month: "2-digit",
        day: "2-digit",
        hour: "2-digit",
        minute: "2-digit",
      });
}

function headerLine(value: string, maxBytes: number): string {
  return limitUtf8(value.replace(/[\r\n\x00-\x1f\x7f]/g, " "), maxBytes).text;
}

function prefixedSubject(subject: string, prefix: string, existing: RegExp) {
  const clean = headerLine(subject, 998);
  return limitUtf8(existing.test(clean) ? clean : `${prefix}: ${clean}`, 900)
    .text;
}

/** 引用或转发用的原文纯文本，限制大小以符合正文 1 MB 上限。 */
function originalText(detail: MessageDetail): string {
  const original = (
    detail.textBody || "[原邮件为 HTML 内容，请在原邮件中查看。]"
  ).replace(/[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]/g, "");
  const limited = limitUtf8(original, 200_000);
  return (
    limited.text +
    (limited.truncated
      ? "\n[原文较长，已截断引用；完整内容请查看原邮件。]"
      : "")
  );
}

function limitUtf8(
  value: string,
  maxBytes: number,
): { text: string; truncated: boolean } {
  const encoder = new TextEncoder();
  let bytes = 0;
  let result = "";
  for (const character of value) {
    bytes += encoder.encode(character).length;
    if (bytes > maxBytes) return { text: result, truncated: true };
    result += character;
  }
  return { text: result, truncated: false };
}

export function base64Size(value: string): number {
  return Math.max(
    0,
    Math.floor((value.length * 3) / 4) -
      (value.endsWith("==") ? 2 : value.endsWith("=") ? 1 : 0),
  );
}

export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.ceil(bytes / 1024)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export function formatMailDate(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.valueOf())
    ? value || "日期未知"
    : date.toLocaleString("zh-CN", {
        month: "2-digit",
        day: "2-digit",
        hour: "2-digit",
        minute: "2-digit",
      });
}

export function mergeMessagePages(
  current: MessageSummary[],
  next: MessageSummary[],
  uidValidity?: number | null,
): MessageSummary[] {
  const generation =
    uidValidity ?? next[0]?.uidValidity ?? current[0]?.uidValidity;
  const merged = new Map(
    current
      .filter((message) => message.uidValidity === generation)
      .map((message) => [`${message.uidValidity}:${message.uid}`, message]),
  );
  for (const message of next.filter(
    (message) => message.uidValidity === generation,
  ))
    merged.set(`${message.uidValidity}:${message.uid}`, message);
  return [...merged.values()].sort((a, b) => b.uid - a.uid);
}

export function selectionRemainsInPage(
  selected: MessageSummary,
  page: MessagePage,
): boolean {
  return (
    selected.uidValidity === page.uidValidity &&
    page.messages.some(
      (message) =>
        message.uid === selected.uid &&
        message.uidValidity === selected.uidValidity,
    )
  );
}

/**
 * 从列表中移除已移走/删除的邮件，并选出接下来要打开的邮件：
 * 优先同位置的下一封（更早的邮件），列表末尾时取上一封，列表清空时为 undefined。
 */
export function removeFromList(
  messages: MessageSummary[],
  uidValidity: number,
  uids: number[],
  selected?: MessageSummary,
): { messages: MessageSummary[]; next?: MessageSummary } {
  const removed = new Set(uids);
  const gone = (message: MessageSummary) =>
    message.uidValidity === uidValidity && removed.has(message.uid);
  const remaining = messages.filter((message) => !gone(message));
  if (!selected || !gone(selected)) return { messages: remaining };
  const index = messages.findIndex(
    (message) =>
      message.uid === selected.uid &&
      message.uidValidity === selected.uidValidity,
  );
  const after = messages.slice(index + 1).find((message) => !gone(message));
  const before = messages
    .slice(0, Math.max(0, index))
    .reverse()
    .find((message) => !gone(message));
  return { messages: remaining, next: after ?? before };
}

/** 多选状态的键：UID 只在同一 UIDVALIDITY 内唯一。 */
export function messageKey(message: { uid: number; uidValidity: number }) {
  return `${message.uidValidity}:${message.uid}`;
}

/**
 * 更新多选集合。普通切换只翻转目标；范围选择把锚点到目标之间的邮件全部选中
 * （锚点不在列表中时退化为普通切换）。
 */
export function toggleChecked(
  messages: MessageSummary[],
  checked: ReadonlySet<string>,
  target: MessageSummary,
  anchor?: string,
): Set<string> {
  const next = new Set(checked);
  const key = messageKey(target);
  const from = anchor
    ? messages.findIndex((message) => messageKey(message) === anchor)
    : -1;
  const to = messages.findIndex((message) => messageKey(message) === key);
  if (from >= 0 && to >= 0) {
    for (let index = Math.min(from, to); index <= Math.max(from, to); index++)
      next.add(messageKey(messages[index]));
  } else if (next.has(key)) next.delete(key);
  else next.add(key);
  return next;
}

/** 当前列表中已勾选的邮件（列表变化后自动排除已不存在的项）。 */
export function checkedMessages(
  messages: MessageSummary[],
  checked: ReadonlySet<string>,
): MessageSummary[] {
  return checked.size
    ? messages.filter((message) => checked.has(messageKey(message)))
    : [];
}

/** 可作为"移动到"目标的文件夹：可选择且不是当前文件夹。 */
export function moveTargets<F extends { name: string; selectable: boolean }>(
  folders: F[],
  current?: string,
): F[] {
  return folders.filter(
    (folder) => folder.selectable && folder.name !== current,
  );
}

/** 容量固定的最近使用缓存；Map 保持插入顺序，最早插入的即最久未用。 */
export function createLruCache<K, V>(capacity: number) {
  const entries = new Map<K, V>();
  return {
    get(key: K): V | undefined {
      const value = entries.get(key);
      if (value !== undefined) {
        entries.delete(key);
        entries.set(key, value);
      }
      return value;
    },
    set(key: K, value: V) {
      entries.delete(key);
      entries.set(key, value);
      while (entries.size > capacity)
        entries.delete(entries.keys().next().value as K);
    },
    clear: () => entries.clear(),
    delete: (key: K) => entries.delete(key),
  };
}

export function createRequestGuard() {
  let generation = 0;
  return {
    next: () => ++generation,
    valid: (token: number) => token === generation,
    invalidate: () => {
      generation++;
    },
  };
}

/** 串行写入避免较旧草稿晚到覆盖新内容。发送前等待最后一次写入，再停止队列。 */
export function createDraftWriter<T>(save: (value: T) => Promise<void>) {
  let tail: Promise<void> = Promise.resolve();
  let stopped = false;
  return {
    write(value: T): Promise<void> {
      if (stopped) return Promise.reject(new Error("草稿编辑已结束。"));
      const operation = tail
        .catch(() => {})
        .then(async () => {
          if (!stopped) await save(value);
        });
      tail = operation;
      return operation;
    },
    settled: () => tail,
    stop: () => {
      stopped = true;
    },
  };
}
