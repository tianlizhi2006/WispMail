import {
  createEffect,
  createMemo,
  For,
  onCleanup,
  onMount,
  Show,
} from "solid-js";
import type {
  MailFolder,
  MessageFlag,
  MessageSummary,
  OrganizeAction,
} from "../../lib/types";
import { formatMailDate, messageKey, senderAddress } from "./mailHelpers";
import { Icon } from "../../ui/Icon";

export function MessageList(props: {
  title: string;
  messages: MessageSummary[];
  selected?: MessageSummary;
  loading: boolean;
  loadingMore: boolean;
  hasMore: boolean;
  error: string;
  /** 已勾选邮件的键（见 messageKey）。 */
  checked: ReadonlySet<string>;
  batchBusy: boolean;
  moveTargets: MailFolder[];
  onSelect: (message: MessageSummary) => void;
  /** range 为 true 时选中上次勾选的邮件到本封之间的全部邮件。 */
  onToggleCheck: (message: MessageSummary, range: boolean) => void;
  onCheckAll: (checked: boolean) => void;
  onBatchFlag: (flag: MessageFlag, value: boolean) => void;
  onBatchOrganize: (action: OrganizeAction) => void;
  onRefresh: () => void;
  onLoadMore: () => void;
  /** 搜索框内容；非空时列表显示搜索结果。 */
  searchQuery: string;
  onSearchQuery: (query: string) => void;
  searching: boolean;
  /** 搜索结果所在文件夹的显示名称，显示在每封邮件上。 */
  folderOf?: (message: MessageSummary) => string;
}) {
  const checkedList = createMemo(() =>
    props.searching
      ? []
      : props.messages.filter((message) =>
          props.checked.has(messageKey(message)),
        ),
  );
  const selecting = () => checkedList().length > 0;
  const allChecked = () =>
    props.messages.length > 0 && checkedList().length === props.messages.length;
  const anyUnread = () => checkedList().some((message) => message.unread);
  const anyUnflagged = () => checkedList().some((message) => !message.flagged);
  const isChecked = (message: MessageSummary) =>
    props.checked.has(messageKey(message));
  const isSelected = (message: MessageSummary) =>
    props.selected?.uid === message.uid &&
    props.selected?.uidValidity === message.uidValidity;
  let checkAll: HTMLInputElement | undefined;
  // 部分选中时"全选"框显示半选状态（indeterminate 只能通过属性设置）。
  createEffect(() => {
    const partial = selecting() && !allChecked();
    if (checkAll) checkAll.indeterminate = partial;
  });
  let searchInput: HTMLInputElement | undefined;
  // Ctrl+F 跳到搜索框（撰写窗口等对话框打开时不拦截）。
  const onKeyDown = (event: KeyboardEvent) => {
    if (
      (event.ctrlKey || event.metaKey) &&
      event.key.toLowerCase() === "f" &&
      !document.querySelector("dialog[open]")
    ) {
      event.preventDefault();
      searchInput?.focus();
      searchInput?.select();
    }
  };
  onMount(() => window.addEventListener("keydown", onKeyDown));
  onCleanup(() => window.removeEventListener("keydown", onKeyDown));

  return (
    <section
      class="message-list-pane"
      classList={{ "message-list--selecting": selecting() }}
      aria-label="邮件列表"
    >
      <header class="message-list-header">
        <div>
          <h1>{props.searching ? "搜索结果" : props.title || "邮件"}</h1>
          <span>
            {props.searching
              ? props.loading
                ? ""
                : `找到 ${props.messages.length} 封${props.messages.length >= 200 ? "（仅显示最新 200 封）" : ""}`
              : props.messages.length
                ? `已加载 ${props.messages.length} 封`
                : ""}
          </span>
        </div>
        <Show when={!props.searching}>
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
        </Show>
      </header>
      <div class="list-search" role="search">
        <Icon name="search" />
        <input
          ref={searchInput}
          type="search"
          aria-label="搜索邮件"
          placeholder="搜索发件人、收件人、主题或正文（Ctrl+F）"
          maxlength="200"
          value={props.searchQuery}
          onInput={(event) => props.onSearchQuery(event.currentTarget.value)}
          onKeyDown={(event) => {
            if (event.key === "Escape" && props.searchQuery) {
              event.preventDefault();
              props.onSearchQuery("");
            }
          }}
        />
        <Show when={props.searchQuery}>
          <button
            type="button"
            class="icon-button list-search-clear"
            aria-label="清除搜索"
            title="清除搜索（Esc）"
            onClick={() => {
              props.onSearchQuery("");
              searchInput?.focus();
            }}
          >
            <Icon name="close" />
          </button>
        </Show>
      </div>
      <Show when={selecting()}>
        <div class="batch-bar" role="toolbar" aria-label="批量操作">
          <div class="batch-bar-top">
            <label class="batch-check-all">
              <input
                type="checkbox"
                checked={allChecked()}
                ref={checkAll}
                aria-label={allChecked() ? "取消全选" : "全选已加载的邮件"}
                disabled={props.batchBusy}
                onClick={(event) => {
                  props.onCheckAll(!allChecked());
                  event.currentTarget.checked = allChecked();
                }}
              />
              <span>
                已选 <strong>{checkedList().length}</strong> 封
              </span>
            </label>
            <Show when={props.batchBusy}>
              <span class="spinner" role="status" aria-label="正在处理" />
            </Show>
            <button
              type="button"
              class="button button--text batch-cancel"
              disabled={props.batchBusy}
              onClick={() => props.onCheckAll(false)}
            >
              取消选择
            </button>
          </div>
          <div class="batch-actions">
            <button
              type="button"
              class="batch-action"
              disabled={props.batchBusy}
              onClick={() => props.onBatchFlag("seen", anyUnread())}
            >
              {anyUnread() ? "标为已读" : "标为未读"}
            </button>
            <button
              type="button"
              class="batch-action"
              classList={{ "batch-action--starred": !anyUnflagged() }}
              disabled={props.batchBusy}
              title={anyUnflagged() ? "添加星标" : "取消星标"}
              onClick={() => props.onBatchFlag("flagged", anyUnflagged())}
            >
              <Icon name="star" />
              {anyUnflagged() ? "星标" : "取消星标"}
            </button>
            <button
              type="button"
              class="batch-action"
              disabled={props.batchBusy}
              title="归档"
              onClick={() => props.onBatchOrganize({ type: "archive" })}
            >
              <Icon name="archive" />
              归档
            </button>
            <button
              type="button"
              class="batch-action batch-action--danger"
              disabled={props.batchBusy}
              title="删除（移到废纸篓）"
              onClick={() => props.onBatchOrganize({ type: "trash" })}
            >
              <Icon name="trash" />
              删除
            </button>
            <Show when={props.moveTargets.length}>
              <label class="move-select batch-move" title="移动到文件夹">
                <Icon name="folder" />
                <span class="visually-hidden">把选中的邮件移动到文件夹</span>
                <select
                  disabled={props.batchBusy}
                  value=""
                  onChange={(event) => {
                    const target = event.currentTarget.value;
                    event.currentTarget.value = "";
                    if (target) props.onBatchOrganize({ type: "move", target });
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
          </div>
        </div>
      </Show>
      <Show when={props.loading}>
        <div class="mail-progress" role="status">
          <span class="spinner" />
          {props.searching ? "正在搜索…" : "正在同步邮件…"}
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
            <Show
              when={!props.searching}
              fallback={
                <div class="mail-empty">
                  <Icon name="search" />
                  <strong>
                    {props.loading ? "正在搜索" : "没有找到匹配的邮件"}
                  </strong>
                  <p>
                    搜索范围是已同步到本机的邮件；正文只包含已下载过的邮件。
                  </p>
                </div>
              }
            >
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
            </Show>
          }
        >
          <For each={props.messages}>
            {(message) => (
              <div
                class="message-row"
                classList={{
                  "message-row--selected": isSelected(message),
                  "message-row--checked": isChecked(message),
                  "message-row--unread": message.unread,
                }}
              >
                {/* 左侧一列：平时显示未读圆点，悬停或多选时显示复选框。 */}
                <span class="row-lead">
                  <span
                    class="unread-dot"
                    classList={{ "unread-dot--visible": message.unread }}
                    aria-hidden="true"
                  />
                  <Show when={!props.searching}>
                    <input
                      type="checkbox"
                      class="row-check"
                      checked={isChecked(message)}
                      disabled={props.batchBusy}
                      aria-label={`选择邮件：${message.subject || "（无主题）"}`}
                      onClick={(event) => {
                        props.onToggleCheck(message, event.shiftKey);
                        // 范围选择可能不改变本项状态，手动与数据保持一致。
                        event.currentTarget.checked = isChecked(message);
                      }}
                    />
                  </Show>
                </span>
                <button
                  type="button"
                  class="message-row-open"
                  aria-pressed={isSelected(message)}
                  title={
                    props.searching
                      ? undefined
                      : "按住 Ctrl 点击可多选，按住 Shift 点击可连续选择"
                  }
                  onClick={(event) => {
                    if (props.searching) {
                      props.onSelect(message);
                      return;
                    }
                    if (
                      props.batchBusy &&
                      (event.ctrlKey || event.metaKey || event.shiftKey)
                    )
                      return;
                    if (event.ctrlKey || event.metaKey)
                      props.onToggleCheck(message, false);
                    else if (event.shiftKey) props.onToggleCheck(message, true);
                    else props.onSelect(message);
                  }}
                >
                  <div class="message-row-top">
                    <span class="message-sender" title={message.from}>
                      {senderAddress(message.from) || "未知发件人"}
                    </span>
                    <Show when={props.searching && props.folderOf}>
                      <span class="row-folder">
                        {props.folderOf?.(message)}
                      </span>
                    </Show>
                    <time>{formatMailDate(message.date)}</time>
                  </div>
                  <div class="message-row-subject">
                    <strong>{message.subject || "（无主题）"}</strong>
                    <Show when={message.flagged}>
                      <Icon name="star" class="row-star" />
                    </Show>
                    <Show when={message.hasAttachments}>
                      <Icon name="paperclip" />
                    </Show>
                  </div>
                  <p>{message.snippet || "点击阅读邮件正文"}</p>
                </button>
              </div>
            )}
          </For>
        </Show>
        <Show when={props.hasMore && !props.searching}>
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
