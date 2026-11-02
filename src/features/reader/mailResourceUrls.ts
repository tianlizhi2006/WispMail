/** 无 DOM 依赖的邮件 URL 规则，供渲染器和消息桥共同使用。 */
export interface MailInlineResource {
  contentId: string | null;
  contentLocation: string | null;
  mimeType: string;
  dataBase64: string;
  attachmentIndex: number | null;
}

export interface ResourceContext {
  inlineResources?: readonly MailInlineResource[];
  baseUrl?: string | null;
  allowRemoteImages?: boolean;
  inlineResourceUrl?: (resource: MailInlineResource) => string | null;
}

export interface ResolvedMailResource {
  url: string;
  attachmentIndex: number | null;
  remote: boolean;
}

const IMAGE_TYPES = new Set([
  "image/png",
  "image/jpeg",
  "image/jpg",
  "image/gif",
  "image/webp",
  "image/avif",
  "image/bmp",
  "image/x-ms-bmp",
  "image/x-icon",
  "image/vnd.microsoft.icon",
  "image/svg+xml",
]);
const CONTROL = /[\x00-\x1f\x7f]/;

function httpUrl(value: string, base?: string | null): string | null {
  if (CONTROL.test(value)) return null;
  try {
    const url = base ? new URL(value, base) : new URL(value);
    return /^(https?:)$/.test(url.protocol) && !url.username && !url.password
      ? url.href
      : null;
  } catch {
    return null;
  }
}

export function mailBaseUrl(value?: string | null): string | null {
  return value ? httpUrl(value.trim()) : null;
}

function contentId(value: string): string {
  const trimmed = value.trim();
  return trimmed.startsWith("<") && trimmed.endsWith(">")
    ? trimmed.slice(1, -1)
    : trimmed;
}

function decodeCid(value: string): string {
  try {
    return decodeURIComponent(value);
  } catch {
    return value;
  }
}

function imageDataUrl(value: string): string | null {
  const match = value.match(/^data:([^;,]+)((?:;[^,]*)?),(.*)$/is);
  if (!match || !IMAGE_TYPES.has(match[1].toLowerCase())) return null;
  const parameters = match[2].toLowerCase();
  if (
    parameters &&
    !/^(?:;charset=[a-z0-9._-]+)?(?:;base64)?$/.test(parameters)
  )
    return null;
  if (parameters.endsWith(";base64")) {
    const payload = match[3].replace(/\s/g, "");
    if (
      !payload ||
      !/^[a-z0-9+/]*={0,2}$/i.test(payload) ||
      payload.length % 4 !== 0
    )
      return null;
    return `data:${match[1].toLowerCase()};base64,${payload}`;
  }
  return CONTROL.test(match[3])
    ? null
    : `data:${match[1].toLowerCase()}${parameters},${match[3]}`;
}

export function inlineResourceDataUrl(
  resource: MailInlineResource,
): string | null {
  return imageDataUrl(
    `data:${resource.mimeType};base64,${resource.dataBase64}`,
  );
}

function resourceData(
  resource: MailInlineResource,
  context: ResourceContext,
): ResolvedMailResource | null {
  const url = context.inlineResourceUrl
    ? context.inlineResourceUrl(resource)
    : inlineResourceDataUrl(resource);
  return url
    ? { url, attachmentIndex: resource.attachmentIndex, remote: false }
    : null;
}

function locationKey(value: string, base: string | null): string | null {
  if (
    CONTROL.test(value) ||
    (/^[a-z][a-z0-9+.-]*:/i.test(value) && !/^https?:/i.test(value))
  )
    return null;
  try {
    // 仅用于比较无 base 的 MIME Content-Location，绝不会返回为可加载 URL。
    const url = new URL(value, base ?? "https://mail-resource.invalid/");
    url.hash = "";
    return url.href;
  } catch {
    return null;
  }
}

export function resolveMailResourceUrl(
  raw: string,
  context: ResourceContext = {},
): ResolvedMailResource | null {
  const value = raw.trim();
  if (!value) return null;
  if (/^data:/i.test(value)) {
    const url = imageDataUrl(value);
    return url ? { url, attachmentIndex: null, remote: false } : null;
  }
  if (CONTROL.test(value)) return null;
  const resources = context.inlineResources ?? [];
  if (/^cid:/i.test(value)) {
    const id = contentId(decodeCid(value.slice(4)));
    const resource = resources.find(
      (item) => item.contentId !== null && contentId(item.contentId) === id,
    );
    return resource ? resourceData(resource, context) : null;
  }
  const base = mailBaseUrl(context.baseUrl);
  const key = locationKey(value, base);
  if (key) {
    const resource = resources.find(
      (item) =>
        item.contentLocation !== null &&
        locationKey(item.contentLocation, base) === key,
    );
    if (resource) return resourceData(resource, context);
  }
  if (context.allowRemoteImages === false) return null;
  const url = httpUrl(value, base);
  return url ? { url, attachmentIndex: null, remote: true } : null;
}

export function resolveMailLinkUrl(
  raw: string,
  base?: string | null,
): string | null {
  const value = raw.trim();
  if (!value || CONTROL.test(value)) return null;
  if (value.startsWith("#")) return value;
  if (/^mailto:/i.test(value)) {
    try {
      const url = new URL(value);
      return url.protocol === "mailto:" && (url.pathname || url.search) ? url.href : null;
    } catch {
      return null;
    }
  }
  return httpUrl(value, mailBaseUrl(base));
}

export type MailBridgeMessage =
  { type: "resize"; height: number } | { type: "open-link"; url: string };

/** 调用者还必须检查 event.source === iframe.contentWindow。 */
export function validateMailBridgeMessage(
  value: unknown,
  expectedToken: string,
): MailBridgeMessage | null {
  if (!value || typeof value !== "object") return null;
  const data = value as Record<string, unknown>;
  if (data.channel !== "wispmail-email" || data.token !== expectedToken)
    return null;
  if (
    data.type === "resize" &&
    typeof data.height === "number" &&
    Number.isFinite(data.height) &&
    data.height >= 0 &&
    data.height <= 100_000
  )
    return { type: "resize", height: Math.ceil(data.height) };
  if (data.type === "open-link" && typeof data.url === "string") {
    const url = resolveMailLinkUrl(data.url);
    return url && !url.startsWith("#") ? { type: "open-link", url } : null;
  }
  return null;
}
