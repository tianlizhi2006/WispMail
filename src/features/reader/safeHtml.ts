import DOMPurify from "dompurify";
import { sanitizeInlineStyle, sanitizeStyleSheet } from "./mailCss";
import {
  inlineResourceDataUrl,
  mailBaseUrl,
  resolveMailLinkUrl,
  resolveMailResourceUrl,
  type MailInlineResource,
  type ResourceContext,
} from "./mailResourceUrls";

export interface MailRenderOptions {
  inlineResources?: readonly MailInlineResource[];
  htmlBaseUrl?: string | null;
  allowRemoteImages?: boolean;
  bridgeToken: string;
  scriptNonce: string;
}

export interface RenderedMailDocument {
  html: string;
  usedAttachmentIndexes: number[];
  dispose: () => void;
}

interface InlineImageManifest {
  marker: string;
  mimeType: string;
  dataBase64: string;
}

function bridgeScript(token: string, resources: InlineImageManifest[]): string {
  return `(() => {
  'use strict';
  const token = ${JSON.stringify(token).replace(/</g, "\\u003c")};
  const manifest = ${JSON.stringify(resources).replace(/</g, "\\u003c")};
  const resourceUrls = new Map();
  for (const resource of manifest) {
    const binary = atob(resource.dataBase64);
    const bytes = new Uint8Array(binary.length);
    for (let index = 0; index < binary.length; index++) bytes[index] = binary.charCodeAt(index);
    resourceUrls.set(resource.marker, URL.createObjectURL(new Blob([bytes], { type: resource.mimeType })));
  }
  const markerPattern = new RegExp('data:image/gif;base64,[A-Za-z0-9+/=]+#wisp-[A-Za-z0-9_-]+-[0-9]+', 'g');
  const replaceResources = value => value.replace(markerPattern, marker => resourceUrls.get(marker) || marker);
  for (const element of document.querySelectorAll('*')) {
    for (const name of ['src', 'srcset', 'background', 'style']) {
      const value = element.getAttribute(name);
      if (value) { const replaced = replaceResources(value); if (replaced !== value) element.setAttribute(name, replaced); }
    }
    if (element.tagName === 'STYLE') element.textContent = replaceResources(element.textContent || '');
  }
  const send = (type, payload) => window.parent.postMessage({ channel: 'wispmail-email', token, type, ...payload }, '*');
  let scheduled = false;
  let lastHeight = 0;
  let lastWidth = window.innerWidth;
  let adjustments = 0;
  const measure = () => {
    scheduled = false;
    if (window.innerWidth !== lastWidth) { lastWidth = window.innerWidth; adjustments = 0; }
    const root = document.documentElement;
    const body = document.body;
    // 外层阅读区是唯一的纵向滚动容器；横向滚动条占用的高度也要计入，避免出现第二个纵向滚动条。
    const scrollbar = Math.max(0, window.innerHeight - root.clientHeight);
    const content = Math.max(root.scrollHeight, body ? body.scrollHeight + body.getBoundingClientRect().top : 0);
    const height = Math.min(100000, Math.ceil(content + scrollbar));
    if (height !== lastHeight && adjustments < 24) { lastHeight = height; adjustments++; send('resize', { height }); }
  };
  const schedule = () => { if (!scheduled) { scheduled = true; window.requestAnimationFrame(measure); } };
  const observer = new ResizeObserver(schedule);
  observer.observe(document.documentElement);
  if (document.body) observer.observe(document.body);
  const imagesChanged = () => { adjustments = 0; schedule(); };
  document.addEventListener('load', imagesChanged, true);
  document.addEventListener('error', imagesChanged, true);
  document.addEventListener('toggle', schedule, true);
  window.addEventListener('resize', schedule);
  const linkClick = event => {
    const target = event.target instanceof Element ? event.target.closest('a[href],area[href]') : null;
    if (!target) return;
    event.preventDefault();
    event.stopPropagation();
    if (!event.isTrusted || (event.type === 'auxclick' && event.button !== 1)) return;
    const url = target.getAttribute('href') || '';
    if (url.startsWith('#')) {
      try { document.getElementById(decodeURIComponent(url.slice(1)))?.scrollIntoView(); } catch {}
      return;
    }
    if (/^(https?:|mailto:)/i.test(url)) send('open-link', { url });
  };
  document.addEventListener('click', linkClick, true);
  document.addEventListener('auxclick', linkClick, true);
  window.addEventListener('pagehide', () => {
    observer.disconnect();
    for (const url of resourceUrls.values()) URL.revokeObjectURL(url);
    resourceUrls.clear();
    document.removeEventListener('load', imagesChanged, true);
    document.removeEventListener('error', imagesChanged, true);
    document.removeEventListener('toggle', schedule, true);
    window.removeEventListener('resize', schedule);
    document.removeEventListener('click', linkClick, true);
    document.removeEventListener('auxclick', linkClick, true);
  }, { once: true });
  schedule();
})();`;
}

function rewriteSrcset(
  value: string,
  resolve: (value: string) => string | null,
): string {
  const candidates: string[] = [];
  let index = 0;
  while (index < value.length) {
    while (/[\s,]/.test(value[index] ?? "")) index++;
    if (index >= value.length) break;
    const start = index;
    while (index < value.length && !/\s/.test(value[index])) index++;
    let url = value.slice(start, index);
    if (url.endsWith(",")) {
      url = url.replace(/,+$/, "");
      const resolved = resolve(url);
      if (resolved) candidates.push(resolved);
      continue;
    }
    const descriptorStart = index;
    while (index < value.length && value[index] !== ",") index++;
    const descriptor = value.slice(descriptorStart, index).trim();
    const resolved = resolve(url);
    if (resolved && (!descriptor || /^\d+(?:\.\d+)?[wx]$/.test(descriptor)))
      candidates.push(`${resolved}${descriptor ? ` ${descriptor}` : ""}`);
    index++;
  }
  return candidates.join(", ");
}

/**
 * 保留邮件的完整 HTML 排版，在不具有同源权限的 iframe 中展示。
 * 图片在可信桥内部创建为当前隔离文档的 Blob，每份 MIME 数据只出现一次。
 * 调用者换信/卸载时必须 dispose，并释放用于承载本文档的外层 Blob URL。
 */
export function renderMailDocument(
  html: string,
  options: MailRenderOptions,
): RenderedMailDocument {
  if (!/^[a-z0-9+/_-]{16,128}={0,2}$/i.test(options.scriptNonce))
    throw new Error("无效的邮件渲染 nonce。");
  const used = new Set<number>();
  const resourceMarkers = new Map<
    MailInlineResource,
    InlineImageManifest | null
  >();
  const dispose = () => resourceMarkers.clear();
  try {
    const root = DOMPurify.sanitize(html, {
      WHOLE_DOCUMENT: true,
      RETURN_DOM: true,
      USE_PROFILES: { html: true },
      ADD_TAGS: ["style", "base"],
      ADD_ATTR: [
        "background",
        "bgcolor",
        "text",
        "link",
        "vlink",
        "alink",
        "face",
        "color",
        "size",
      ],
      FORBID_TAGS: [
        "script",
        "iframe",
        "object",
        "embed",
        "form",
        "input",
        "button",
        "textarea",
        "select",
        "link",
        "meta",
        "video",
        "audio",
        "track",
        "svg",
        "math",
        "canvas",
        "noscript",
        "template",
      ],
      FORBID_ATTR: [
        "srcdoc",
        "action",
        "formaction",
        "ping",
        "autofocus",
        "nonce",
        "integrity",
        "is",
        "target",
        "download",
        "xlink:href",
      ],
      ALLOW_DATA_ATTR: false,
      ALLOW_ARIA_ATTR: true,
    }) as HTMLElement;
    const doc = root.ownerDocument;
    const head =
      root.querySelector("head") ??
      root.insertBefore(doc.createElement("head"), root.firstChild);
    const body =
      root.querySelector("body") ?? root.appendChild(doc.createElement("body"));
    const rawBase = root.querySelector("base[href]")?.getAttribute("href");
    const suppliedBase = mailBaseUrl(options.htmlBaseUrl);
    const baseUrl = rawBase
      ? resolveMailLinkUrl(rawBase, suppliedBase)
      : suppliedBase;
    root.querySelectorAll("base").forEach((element) => element.remove());
    const context: ResourceContext = {
      baseUrl: mailBaseUrl(baseUrl),
      inlineResources: options.inlineResources,
      allowRemoteImages: options.allowRemoteImages !== false,
      inlineResourceUrl(resource) {
        if (resourceMarkers.has(resource))
          return resourceMarkers.get(resource)?.marker ?? null;
        const data = inlineResourceDataUrl(resource);
        if (!data) {
          resourceMarkers.set(resource, null);
          return null;
        }
        const marker = `data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7#wisp-${options.scriptNonce.replace(/[^a-z0-9_-]/gi, "")}-${resourceMarkers.size}`;
        resourceMarkers.set(resource, {
          marker,
          mimeType: resource.mimeType.toLowerCase(),
          dataBase64: data.slice(data.indexOf(",") + 1),
        });
        return marker;
      },
    };
    const imageUrl = (value: string): string | null => {
      const resolved = resolveMailResourceUrl(value, context);
      if (
        resolved?.attachmentIndex !== null &&
        resolved?.attachmentIndex !== undefined
      )
        used.add(resolved.attachmentIndex);
      return resolved?.url ?? null;
    };
    const elements = [root, ...Array.from(root.querySelectorAll("*"))];
    for (const element of elements) {
      const tag = element.tagName.toLowerCase();
      for (const attribute of Array.from(element.attributes)) {
        const name = attribute.name.toLowerCase();
        if (name.startsWith("on") || name === "xmlns" || name === "xml:base")
          element.removeAttribute(attribute.name);
      }
      if (element.hasAttribute("style")) {
        const style = sanitizeInlineStyle(
          element.getAttribute("style")!,
          context,
          used,
        );
        if (style) element.setAttribute("style", style);
        else element.removeAttribute("style");
      }
      if (tag === "style") {
        element.textContent = sanitizeStyleSheet(
          element.textContent ?? "",
          context,
          used,
        );
        element.removeAttribute("nonce");
      }
      if (element.hasAttribute("src")) {
        const url =
          tag === "img" ? imageUrl(element.getAttribute("src")!) : null;
        if (url) element.setAttribute("src", url);
        else element.removeAttribute("src");
      }
      if (element.hasAttribute("srcset")) {
        const value =
          tag === "img" ||
          (tag === "source" &&
            element.parentElement?.tagName.toLowerCase() === "picture")
            ? rewriteSrcset(element.getAttribute("srcset")!, imageUrl)
            : "";
        if (value) element.setAttribute("srcset", value);
        else element.removeAttribute("srcset");
      }
      if (element.hasAttribute("background")) {
        const url = imageUrl(element.getAttribute("background")!);
        if (url) element.setAttribute("background", url);
        else element.removeAttribute("background");
      }
      if (element.hasAttribute("href")) {
        const url =
          tag === "a" || tag === "area"
            ? resolveMailLinkUrl(element.getAttribute("href")!, context.baseUrl)
            : null;
        if (url) element.setAttribute("href", url);
        else element.removeAttribute("href");
      }
      if (tag === "img") {
        element.setAttribute("referrerpolicy", "no-referrer");
        element.setAttribute("loading", "eager");
      }
    }
    // iframe 高度随内容自适应，由外层阅读区统一滚动：禁止邮件文档自身纵向滚动，
    // 宽表格仍可横向滚动。行内 !important 优先级高于邮件样式表中的任何规则。
    root.style.setProperty("overflow-y", "hidden", "important");
    root.style.setProperty("overflow-x", "auto", "important");
    body.style.setProperty("overflow", "visible", "important");
    const csp = doc.createElement("meta");
    csp.httpEquiv = "Content-Security-Policy";
    csp.content = `default-src 'none'; img-src data: blob:${options.allowRemoteImages !== false ? " http: https:" : ""}; style-src 'unsafe-inline'; script-src 'nonce-${options.scriptNonce}'; script-src-attr 'none'; connect-src 'none'; font-src 'none'; media-src 'none'; frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'`;
    const charset = doc.createElement("meta");
    charset.setAttribute("charset", "utf-8");
    const referrer = doc.createElement("meta");
    referrer.name = "referrer";
    referrer.content = "no-referrer";
    const colorScheme = doc.createElement("meta");
    colorScheme.name = "color-scheme";
    colorScheme.content = "light";
    head.prepend(charset, csp, referrer, colorScheme);
    const script = doc.createElement("script");
    script.setAttribute("nonce", options.scriptNonce);
    script.setAttribute("style", "display:none!important");
    script.textContent = bridgeScript(
      options.bridgeToken,
      [...resourceMarkers.values()].filter(
        (value): value is InlineImageManifest => value !== null,
      ),
    );
    body.append(script);
    return {
      html: `<!doctype html>${root.outerHTML}`,
      usedAttachmentIndexes: [...used].sort((a, b) => a - b),
      dispose,
    };
  } catch (error) {
    dispose();
    throw error;
  }
}
