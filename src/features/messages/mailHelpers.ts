import type {
  ComposeAttachment,
  ComposeMessage,
  MessageDetail,
  MessagePage,
  MessageSet,
  MessageSummary,
  SearchHit,
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
    forward: null,
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

/**
 * 转发：正文只放用户自己的文字；原邮件以 forward 引用，发送时由后端把原邮件头、
 * HTML 排版和内嵌图片附在下方。普通附件由调用方取出后传入，不设置回复线程头。
 */
export function forwardMessage(
  detail: MessageDetail,
  folder: string,
  attachments: ComposeAttachment[] = [],
): ComposeMessage {
  const subject = headerLine(detail.summary.subject, 600) || "（无主题）";
  const sender = headerLine(detail.summary.from, 600) || "未知发件人";
  return {
    ...emptyMessage(),
    subject: prefixedSubject(detail.summary.subject, "Fwd", FORWARD_SUBJECT),
    attachments,
    forward: {
      folder,
      uid: detail.summary.uid,
      uidValidity: detail.summary.uidValidity,
      label: `${subject} · ${sender}`,
    },
  };
}

/**
 * 转发时随邮件附上的附件索引：被原 HTML 以 CID 内嵌的图片会随正文一起发送，
 * 不再重复作为附件。
 */
export function forwardAttachmentIndexes(detail: MessageDetail): number[] {
  const embedded = new Set(
    detail.htmlBody
      ? detail.inlineResources
          .filter((resource) => resource.contentId)
          .map((resource) => resource.attachmentIndex)
      : [],
  );
  return detail.attachments
    .map((attachment) => attachment.index)
    .filter((index) => !embedded.has(index));
}

const REPLY_SUBJECT = /^(re|回复)\s*[:：]/i;
const FORWARD_SUBJECT = /^(fwd?|转发)\s*[:：]/i;

/** 撰写窗口标题：回复带线程头；转发通过主题前缀识别，重新打开草稿时也能保持。 */
export function composeKind(
  message: ComposeMessage,
): "reply" | "forward" | "new" {
  if (message.inReplyTo) return "reply";
  return message.forward || FORWARD_SUBJECT.test(message.subject)
    ? "forward"
    : "new";
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

/** 与后端一致：编码后的整封邮件不能超过 20 MiB。 */
export const MAX_MESSAGE_BYTES = 20 * 1024 * 1024;

const utf8 = new TextEncoder();

/** Base64 编码后的字节数（每 76 个字符换行 CRLF）。 */
export function base64EncodedSize(bytes: number): number {
  const characters = Math.ceil(bytes / 3) * 4;
  return characters + Math.ceil(characters / 76) * 2;
}

/**
 * 正文编码后的大小：纯 ASCII 原样发送；含中文等字符时发送程序在
 * quoted-printable（每个非 ASCII 字节变成 3 个字符）与 Base64 中取较短者。
 */
export function encodedTextSize(text: string): number {
  const bytes = utf8.encode(text.replace(/\r?\n/g, "\r\n"));
  let escaped = 0;
  for (const byte of bytes) if (byte >= 0x80 || byte === 0x3d) escaped++;
  if (escaped === 0) return bytes.length;
  const quoted = bytes.length + escaped * 2;
  return Math.min(
    quoted + Math.ceil(quoted / 75) * 3,
    base64EncodedSize(bytes.length),
  );
}

/** 编码后的邮件头字段：非 ASCII 文字使用 Base64 编码字（=?utf-8?b?…?=）。 */
function encodedHeaderSize(value: string): number {
  const bytes = utf8.encode(value).length;
  return /^[\x20-\x7e]*$/.test(value)
    ? bytes
    : base64EncodedSize(bytes) + Math.ceil(bytes / 45) * 14;
}

/**
 * 预估发送时的整封邮件大小（字节），用于撰写窗口实时显示。
 * forwardedBytes 为后端按实际编码算出的原邮件部分大小；转发时尚未得到则传 null。
 */
export function estimateMessageSize(
  draft: ComposeMessage,
  forwardedBytes: number | null = null,
): number {
  // From、Date、Message-ID、MIME-Version、Content-Type 等固定邮件头。
  let size = 150 + encodedHeaderSize(draft.subject);
  // 密送只在 SMTP 信封中，不写入邮件头。
  for (const address of [...draft.to, ...draft.cc])
    size += encodedHeaderSize(address) + 6;
  for (const id of [
    ...draft.references,
    ...(draft.inReplyTo ? [draft.inReplyTo] : []),
  ])
    size += id.length + 4;
  const text = encodedTextSize(draft.body);
  if (draft.forward)
    // 用户文字同时出现在纯文本和 HTML 两个版本中。
    size += (forwardedBytes ?? 0) + text * 2 + 13;
  else size += text + 90;
  for (const attachment of draft.attachments)
    size +=
      base64EncodedSize(base64Size(attachment.dataBase64)) +
      encodedHeaderSize(attachment.name) +
      170;
  if (draft.attachments.length) size += 160;
  return size;
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

/**
 * 用最新一页替换列表开头：该页 UID 范围内以它为准（新邮件加入、已删除的移除），
 * 范围之外用户已加载的更早邮件保留。该页已是全部邮件时直接替换。
 */
export function refreshFirstPage(
  current: MessageSummary[],
  page: MessagePage,
): MessageSummary[] {
  const generation = page.uidValidity;
  if (!page.hasMore || !page.messages.length) return page.messages;
  const oldest = Math.min(...page.messages.map((message) => message.uid));
  return [
    ...page.messages,
    ...current.filter(
      (message) => message.uidValidity === generation && message.uid < oldest,
    ),
  ].sort((a, b) => b.uid - a.uid);
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

/**
 * 从搜索结果中移除已移走/删除的邮件（只匹配同一文件夹、同一代 UID），
 * 并在打开的邮件被移除时选出相邻的结果，规则与 removeFromList 相同。
 */
export function removeHits(
  hits: SearchHit[],
  set: Pick<MessageSet, "folder" | "uidValidity" | "uids">,
  opened?: { folder: string; summary: MessageSummary },
): { hits: SearchHit[]; next?: SearchHit } {
  const removed = new Set(set.uids);
  const gone = (hit: { folder: string; summary: MessageSummary }) =>
    hit.folder === set.folder &&
    hit.summary.uidValidity === set.uidValidity &&
    removed.has(hit.summary.uid);
  const remaining = hits.filter((hit) => !gone(hit));
  if (!opened || !gone(opened)) return { hits: remaining };
  const index = hits.findIndex(
    (hit) =>
      hit.folder === opened.folder &&
      hit.summary.uid === opened.summary.uid &&
      hit.summary.uidValidity === opened.summary.uidValidity,
  );
  const after = hits.slice(index + 1).find((hit) => !gone(hit));
  const before = hits
    .slice(0, Math.max(0, index))
    .reverse()
    .find((hit) => !gone(hit));
  return { hits: remaining, next: after ?? before };
}

/** 列表中只显示发件人名称；没有名称时显示地址。 */
export function senderName(from: string): string {
  const match = /^\s*"?([^"<]*?)"?\s*<([^>]+)>\s*$/.exec(from);
  if (!match) return from.trim();
  return match[1].trim() || match[2].trim();
}

/** 可作为"移动到"目标的文件夹：可选择、未隐藏且不是当前文件夹。 */
export function moveTargets<
  F extends { name: string; selectable: boolean; hidden?: boolean },
>(folders: F[], current?: string): F[] {
  return folders.filter(
    (folder) => folder.selectable && !folder.hidden && folder.name !== current,
  );
}

/** 文件夹栏显示的文件夹：默认隐藏系统文件夹，但当前选中的始终显示。 */
export function visibleFolders<F extends { name: string; hidden?: boolean }>(
  folders: F[],
  showHidden: boolean,
  selected?: string,
): F[] {
  return folders.filter(
    (folder) => showHidden || !folder.hidden || folder.name === selected,
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
