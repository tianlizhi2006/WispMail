import { For, Show } from "solid-js";
import type { MessageSummary } from "../../lib/types";
import { formatMailDate } from "./mailHelpers";
import { Icon } from "../../ui/Icon";

export function MessageList(props: {
  title: string;
  messages: MessageSummary[];
  selected?: MessageSummary;
  loading: boolean;
  loadingMore: boolean;
  hasMore: boolean;
  error: string;
  onSelect: (message: MessageSummary) => void;
  onRefresh: () => void;
  onLoadMore: () => void;
}) {
  return (
    <section class="message-list-pane" aria-label="邮件列表">
      <header class="message-list-header">
        <div>
          <h1>{props.title || "邮件"}</h1>
          <span>
            {props.messages.length ? `已加载 ${props.messages.length} 封` : ""}
          </span>
        </div>
        <button
          type="button"
          class="icon-button"
          aria-label="刷新当前文件夹"
          title="刷新当前文件夹"
          disabled={props.loading || props.loadingMore || !props.title}
          onClick={props.onRefresh}
        >
          <Icon name="refresh" />
        </button>
      </header>
      <Show when={props.loading}>
        <div class="mail-progress" role="status">
          <span class="spinner" />
          正在同步邮件…
        </div>
      </Show>
      <Show when={props.error}>
        <div class="mail-inline-error" role="status">
          {props.error}
        </div>
      </Show>
      <div class="message-list-scroll">
        <Show
          when={props.messages.length > 0}
          fallback={
            <div class="mail-empty">
              <Icon name="inbox" />
              <strong>
                {props.loading
                  ? "正在读取邮箱"
                  : props.error
                    ? "暂无可用的本地邮件"
                    : "这里还没有邮件"}
              </strong>
              <p>
                {props.loading
                  ? "首次同步需要一点时间。"
                  : props.error
                    ? "恢复连接后点击刷新重试。"
                    : "选择其他文件夹，或刷新查看新邮件。"}
              </p>
            </div>
          }
        >
          <For each={props.messages}>
            {(message) => (
              <button
                type="button"
                class="message-row"
                classList={{
                  "message-row--selected":
                    props.selected?.uid === message.uid &&
                    props.selected?.uidValidity === message.uidValidity,
                  "message-row--unread": message.unread,
                }}
                onClick={() => props.onSelect(message)}
                aria-pressed={
                  props.selected?.uid === message.uid &&
                  props.selected?.uidValidity === message.uidValidity
                }
              >
                <div class="message-row-top">
                  <span class="message-sender">
                    {message.from || "未知发件人"}
                  </span>
                  <time>{formatMailDate(message.date)}</time>
                </div>
                <div class="message-row-subject">
                  <span
                    class="unread-dot"
                    classList={{ "unread-dot--visible": message.unread }}
                  />
                  <strong>{message.subject || "（无主题）"}</strong>
                  <Show when={message.hasAttachments}>
                    <Icon name="paperclip" />
                  </Show>
                </div>
                <p>{message.snippet || "点击阅读邮件正文"}</p>
              </button>
            )}
          </For>
        </Show>
        <Show when={props.hasMore}>
          <button
            type="button"
            class="load-more"
            disabled={props.loadingMore || props.loading}
            onClick={props.onLoadMore}
          >
            {props.loadingMore ? "正在加载…" : "加载更早的邮件"}
          </button>
        </Show>
      </div>
    </section>
  );
}
