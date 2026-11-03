import {
  createEffect,
  createMemo,
  createSignal,
  For,
  onCleanup,
  onMount,
  Show,
} from "solid-js";
import type {
  InlineResource,
  MailFolder,
  MessageDetail,
} from "../../lib/types";
import { errorMessage, openEmailLink } from "../../lib/api";
import { Icon } from "../../ui/Icon";
import { formatMailDate, formatSize } from "../messages/mailHelpers";
import { renderMailDocument } from "./safeHtml";
import { validateMailBridgeMessage } from "./mailResourceUrls";

const NO_INLINE_RESOURCES: InlineResource[] = [];
function randomSecret(): string {
  return Array.from(crypto.getRandomValues(new Uint8Array(24)), (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("");
}

export function MessageView(props: {
  message: MessageDetail;
  readBusy: boolean;
  attachmentBusy: boolean;
  organizeBusy: boolean;
  moveTargets: MailFolder[];
  forwardBusy: boolean;
  onReply: (all: boolean) => void;
  onForward: () => void;
  onToggleRead: () => void;
  onToggleFlag: () => void;
  onArchive: () => void;
  onDelete: () => void;
  onMove: (target: string) => void;
  onSaveAttachment: (index: number) => void;
}) {
  const [plainText, setPlainText] = createSignal(false);
  const [remoteImages, setRemoteImages] = createSignal(true);
  const [frameHeight, setFrameHeight] = createSignal(240);
  const [linkError, setLinkError] = createSignal("");
  const htmlBody = createMemo(() => props.message.htmlBody);
  const inlineResources = createMemo(
    () => props.message.inlineResources ?? NO_INLINE_RESOURCES,
  );
  const htmlBaseUrl = createMemo(() => props.message.htmlBaseUrl ?? null);
  let frame: HTMLIFrameElement | undefined;
  let alive = true;
  let openingLink = false;
  const rendered = createMemo(() => {
    const html = htmlBody();
    const resources = inlineResources();
    const base = htmlBaseUrl();
    const allowRemoteImages = remoteImages();
    if (!html) return null;
    try {
      const token = randomSecret();
      const result = renderMailDocument(html, {
        inlineResources: resources,
        htmlBaseUrl: base,
        allowRemoteImages,
        bridgeToken: token,
        scriptNonce: randomSecret(),
      });
      const url = URL.createObjectURL(
        new Blob([result.html], { type: "text/html;charset=utf-8" }),
      );
      onCleanup(() => {
        URL.revokeObjectURL(url);
        result.dispose();
      });
      return {
        url,
        token,
        usedAttachmentIndexes: result.usedAttachmentIndexes,
        error: "",
      };
    } catch (error) {
      return {
        url: "",
        token: "",
        usedAttachmentIndexes: [] as number[],
        error: `邮件排版暂时无法显示：${errorMessage(error)}`,
      };
    }
  });
  const visibleAttachments = createMemo(() => {
    const used = plainText() ? [] : (rendered()?.usedAttachmentIndexes ?? []);
    return props.message.attachments.filter(
      (attachment) => !used.includes(attachment.index),
    );
  });
  createEffect(() => {
    rendered();
    setFrameHeight(240);
  });
  const onBridgeMessage = (event: MessageEvent) => {
    const document = rendered();
    if (
      !alive ||
      plainText() ||
      !frame?.isConnected ||
      event.source !== frame.contentWindow ||
      event.origin !== "null" ||
      !document?.token
    )
      return;
    const message = validateMailBridgeMessage(event.data, document.token);
    if (!message) return;
    if (message.type === "resize")
      setFrameHeight(Math.max(160, message.height));
    else if (!openingLink) {
      openingLink = true;
      setLinkError("");
      void openEmailLink(message.url)
        .catch((error) => {
          if (alive) setLinkError(`无法打开链接：${errorMessage(error)}`);
        })
        .finally(() => {
          openingLink = false;
        });
    }
  };
  onMount(() => window.addEventListener("message", onBridgeMessage));
  onCleanup(() => {
    alive = false;
    window.removeEventListener("message", onBridgeMessage);
  });
  return (
    <article class="message-reader">
      <div class="reader-toolbar">
        <div class="button-group">
          <button
            type="button"
            class="button button--secondary"
            title="回复"
            onClick={() => props.onReply(false)}
          >
            <Icon name="reply" />
            <span class="button-label">回复</span>
          </button>
          <button
            type="button"
            class="button button--secondary"
            title="全部回复"
            onClick={() => props.onReply(true)}
          >
            <Icon name="replyAll" />
            <span class="button-label">全部回复</span>
          </button>
          <button
            type="button"
            class="button button--secondary"
            title="转发"
            disabled={props.forwardBusy}
            onClick={props.onForward}
          >
            <Icon name="forward" />
            <span class="button-label">
              {props.forwardBusy ? "准备中…" : "转发"}
            </span>
          </button>
        </div>
        <div class="reader-actions">
          <button
            type="button"
            class="icon-button"
            classList={{
              "icon-button--starred": props.message.summary.flagged,
            }}
            aria-pressed={props.message.summary.flagged}
            aria-label={props.message.summary.flagged ? "取消星标" : "添加星标"}
            title={props.message.summary.flagged ? "取消星标" : "添加星标"}
            disabled={props.readBusy || props.organizeBusy}
            onClick={props.onToggleFlag}
          >
            <Icon name="star" />
          </button>
          <button
            type="button"
            class="icon-button"
            aria-label="归档"
            title="归档"
            disabled={props.organizeBusy}
            onClick={props.onArchive}
          >
            <Icon name="archive" />
          </button>
          <button
            type="button"
            class="icon-button"
            aria-label="删除"
            title="删除（移到废纸篓）"
            disabled={props.organizeBusy}
            onClick={props.onDelete}
          >
            <Icon name="trash" />
          </button>
          <Show when={props.moveTargets.length}>
            <label class="move-select" title="移动到文件夹">
              <Icon name="folder" />
              <span class="visually-hidden">移动到文件夹</span>
              <select
                disabled={props.organizeBusy}
                value=""
                onChange={(event) => {
                  const target = event.currentTarget.value;
                  event.currentTarget.value = "";
                  if (target) props.onMove(target);
                }}
              >
                <option value="">移动到…</option>
                <For each={props.moveTargets}>
                  {(folder) => (
                    <option value={folder.name}>
                      {folder.displayName || folder.name}
                    </option>
                  )}
                </For>
              </select>
            </label>
          </Show>
          <button
            type="button"
            class="button button--text"
            disabled={props.readBusy || props.organizeBusy}
            onClick={props.onToggleRead}
          >
            {props.readBusy
              ? "更新中…"
              : props.message.summary.unread
                ? "标为已读"
                : "标为未读"}
          </button>
        </div>
      </div>
      <header class="reader-header">
        <h2>{props.message.summary.subject || "（无主题）"}</h2>
        <p class="reader-from">{props.message.summary.from || "未知发件人"}</p>
        <dl>
          <div>
            <dt>收件人</dt>
            <dd>{props.message.summary.to.join("，") || "未提供"}</dd>
          </div>
          <Show when={props.message.cc.length}>
            <div>
              <dt>抄送</dt>
              <dd>{props.message.cc.join("，")}</dd>
            </div>
          </Show>
          <div>
            <dt>时间</dt>
            <dd>{formatMailDate(props.message.summary.date)}</dd>
          </div>
        </dl>
      </header>
      <Show when={props.message.htmlBody}>
        <div class="reader-safety">
          <Icon name="shield" />
          <span>
            {remoteImages()
              ? "按原邮件显示图文"
              : "已隐藏网络图片，内嵌图片仍显示"}
          </span>
          <button
            type="button"
            onClick={() => setRemoteImages((value) => !value)}
          >
            {remoteImages() ? "隐藏网络图片" : "显示网络图片"}
          </button>
          <button type="button" onClick={() => setPlainText((value) => !value)}>
            {plainText() ? "显示排版" : "纯文本"}
          </button>
        </div>
      </Show>
      <Show when={linkError() || rendered()?.error}>
        <p class="reader-render-error" role="status">
          {linkError() || rendered()?.error}
        </p>
      </Show>
      <div class="reader-content">
        <Show
          when={!!rendered()?.url && !plainText()}
          fallback={
            <pre class="plain-message">
              {props.message.textBody ||
                (props.message.htmlBody
                  ? "此邮件没有纯文本正文，可切换到排版视图。"
                  : "此邮件没有可显示的正文。")}
            </pre>
          }
        >
          <iframe
            ref={frame}
            class="html-message"
            title="邮件图文正文"
            sandbox="allow-scripts"
            referrerpolicy="no-referrer"
            src={rendered()?.url}
            style={{ height: `${frameHeight()}px` }}
          />
        </Show>
      </div>
      <Show when={visibleAttachments().length}>
        <section class="reader-attachments" aria-label="邮件附件">
          <h3>
            <Icon name="paperclip" />
            附件 · {visibleAttachments().length}
          </h3>
          <For each={visibleAttachments()}>
            {(attachment) => (
              <button
                type="button"
                class="download-attachment"
                disabled={props.attachmentBusy}
                onClick={() => props.onSaveAttachment(attachment.index)}
              >
                <Icon name="download" />
                <span>
                  <strong>{attachment.name || "未命名附件"}</strong>
                  <small>{formatSize(attachment.size)}</small>
                </span>
                <span>保存</span>
              </button>
            )}
          </For>
        </section>
      </Show>
    </article>
  );
}
