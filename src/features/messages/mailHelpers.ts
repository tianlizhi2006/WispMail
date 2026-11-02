import type {
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
  const cleanSubject = detail.summary.subject.replace(
    /[\r\n\x00-\x1f\x7f]/g,
    " ",
  );
  const subject = limitUtf8(
    /^(re|回复)\s*:/i.test(cleanSubject) ? cleanSubject : `Re: ${cleanSubject}`,
    900,
  ).text;
  const original = (
    detail.textBody || "[原邮件为 HTML 内容，请在原邮件中查看。]"
  ).replace(/[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]/g, "");
  const quoted = limitUtf8(original, 200_000);
  const quote =
    quoted.text +
    (quoted.truncated
      ? "\n[原文较长，已截断引用；完整内容请查看原邮件。]"
      : "");
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
  const quoteSender = limitUtf8(
    detail.summary.from.replace(/[\r\n\x00-\x1f\x7f]/g, " "),
    600,
  ).text;
  const quoteDate = limitUtf8(
    detail.summary.date.replace(/[\r\n\x00-\x1f\x7f]/g, " "),
    100,
  ).text;
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
