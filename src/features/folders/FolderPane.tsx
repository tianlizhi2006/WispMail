import { For, Show } from "solid-js";
import type { MailFolder } from "../../lib/types";
import { Icon } from "../../ui/Icon";

export function FolderPane(props: {
  folders: MailFolder[];
  selected: string;
  loading: boolean;
  error: string;
  onSelect: (folder: MailFolder) => void;
  onRefresh: () => void;
}) {
  return (
    <nav class="mail-folders" aria-label="邮箱文件夹">
      <div class="section-label account-list-title">
        <span>文件夹</span>
        <button
          type="button"
          class="icon-button"
          aria-label="刷新文件夹列表"
          disabled={props.loading}
          onClick={props.onRefresh}
        >
          <Icon name="refresh" />
        </button>
      </div>
      <Show when={props.error}>
        <p class="folder-error" role="status">
          {props.error}
        </p>
      </Show>
      <Show
        when={props.folders.length}
        fallback={
          <p class="sidebar-empty">
            {props.loading ? "正在读取文件夹…" : "暂无文件夹，请刷新重试。"}
          </p>
        }
      >
        <For each={props.folders}>
          {(folder) => (
            <button
              type="button"
              class="folder-item"
              classList={{
                "folder-item--selected": props.selected === folder.name,
              }}
              disabled={!folder.selectable}
              aria-current={props.selected === folder.name ? "true" : undefined}
              onClick={() => props.onSelect(folder)}
            >
              <Icon
                name={
                  folder.kind.toLowerCase().includes("inbox") ||
                  folder.name.toUpperCase() === "INBOX"
                    ? "inbox"
                    : "mail"
                }
              />
              <span title={folder.name}>
                {folder.displayName || folder.name}
              </span>
            </button>
          )}
        </For>
      </Show>
    </nav>
  );
}
