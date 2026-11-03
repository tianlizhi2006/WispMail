import { createMemo, createSignal, For, Show } from "solid-js";
import type { MailFolder } from "../../lib/types";
import { Icon, type IconName } from "../../ui/Icon";
import { visibleFolders } from "../messages/mailHelpers";

const KIND_ICONS: Record<string, IconName> = {
  inbox: "inbox",
  drafts: "compose",
  sent: "forward",
  archive: "archive",
  junk: "shield",
  trash: "trash",
};

function folderIcon(folder: MailFolder): IconName {
  if (folder.name.toUpperCase() === "INBOX") return "inbox";
  return KIND_ICONS[folder.kind] ?? "folder";
}

export function FolderPane(props: {
  folders: MailFolder[];
  selected: string;
  loading: boolean;
  error: string;
  onSelect: (folder: MailFolder) => void;
  onRefresh: () => void;
}) {
  const [showHidden, setShowHidden] = createSignal(false);
  const hiddenCount = createMemo(
    () => props.folders.filter((folder) => folder.hidden).length,
  );
  // 当前选中的隐藏文件夹始终显示，避免选中项从列表中消失。
  const shown = createMemo(() =>
    visibleFolders(props.folders, showHidden(), props.selected),
  );

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
        <For each={shown()}>
          {(folder) => (
            <button
              type="button"
              class="folder-item"
              classList={{
                "folder-item--selected": props.selected === folder.name,
                "folder-item--group": !folder.selectable,
                "folder-item--hidden": !!folder.hidden,
              }}
              style={{ "padding-left": `${10 + (folder.depth ?? 0) * 16}px` }}
              disabled={!folder.selectable}
              aria-current={props.selected === folder.name ? "true" : undefined}
              onClick={() => props.onSelect(folder)}
            >
              <Icon name={folderIcon(folder)} />
              <span title={folder.name}>
                {folder.displayName || folder.name}
              </span>
            </button>
          )}
        </For>
        <Show when={hiddenCount()}>
          <button
            type="button"
            class="folder-toggle"
            aria-pressed={showHidden()}
            onClick={() => setShowHidden((value) => !value)}
          >
            {showHidden()
              ? "收起系统文件夹"
              : `显示 ${hiddenCount()} 个隐藏的文件夹`}
          </button>
        </Show>
      </Show>
    </nav>
  );
}
