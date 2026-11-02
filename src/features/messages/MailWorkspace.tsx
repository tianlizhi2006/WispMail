import { createSignal, For, onCleanup, onMount, Show } from "solid-js";
import {
  clearDraft,
  errorMessage,
  getDraft,
  getMessage,
  listCachedFolders,
  listCachedMessages,
  refreshFolders,
  saveAttachment,
  setMessageRead,
  syncFolder,
} from "../../lib/api";
import type {
  Account,
  ComposeMessage,
  MailFolder,
  MessageDetail,
  MessageRef,
  MessageSummary,
  SendResult,
} from "../../lib/types";
import { Icon } from "../../ui/Icon";
import { FolderPane } from "../folders/FolderPane";
import { MessageView } from "../reader/MessageView";
import { Composer } from "./Composer";
import { MessageList } from "./MessageList";
import {
  createLruCache,
  createRequestGuard,
  emptyMessage,
  mergeMessagePages,
  replyMessage,
  selectionRemainsInPage,
} from "./mailHelpers";
import "./mail.css";

/** 内存中保留的最近打开邮件数；含内嵌图片的邮件较大，数量保持克制。 */
const DETAIL_CACHE_SIZE = 20;

export function MailWorkspace(props: {
  account: Account;
  accounts: Account[];
  onAccountChange: (id: string) => void;
  onSettings: () => void;
}) {
  const accountId = props.account.id;
  const [folders, setFolders] = createSignal<MailFolder[]>([]);
  const [folder, setFolder] = createSignal<MailFolder>();
  const [folderLoading, setFolderLoading] = createSignal(false);
  const [folderError, setFolderError] = createSignal("");
  const [messages, setMessages] = createSignal<MessageSummary[]>([]);
  const [uidValidity, setUidValidity] = createSignal<number | null>(null);
  const [listLoading, setListLoading] = createSignal(false);
  const [moreLoading, setMoreLoading] = createSignal(false);
  const [hasMore, setHasMore] = createSignal(false);
  const [listError, setListError] = createSignal("");
  const [selected, setSelected] = createSignal<MessageSummary>();
  const [detail, setDetail] = createSignal<MessageDetail>();
  const [readerLoading, setReaderLoading] = createSignal(false);
  const [readerError, setReaderError] = createSignal("");
  const [readBusy, setReadBusy] = createSignal(false);
  const [attachmentBusy, setAttachmentBusy] = createSignal(false);
  const [notice, setNotice] = createSignal("");
  const [draftLoading, setDraftLoading] = createSignal(false);
  const [composer, setComposer] = createSignal<{
    initial: ComposeMessage;
    saveInitially: boolean;
  }>();
  const [draftConflict, setDraftConflict] = createSignal<{
    existing: ComposeMessage;
    requested: ComposeMessage;
  }>();
  const [conflictError, setConflictError] = createSignal("");
  const listGuard = createRequestGuard();
  const readerGuard = createRequestGuard();
  const attachmentGuard = createRequestGuard();
  const detailCache = createLruCache<string, MessageDetail>(DETAIL_CACHE_SIZE);
  const detailKey = (ref: MessageRef) =>
    `${ref.folder}\u0000${ref.uidValidity}\u0000${ref.uid}`;
  let alive = true;
  let conflictDialog!: HTMLDialogElement;

  onMount(() => {
    void loadFolders(true);
  });
  onCleanup(() => {
    alive = false;
    listGuard.invalidate();
    readerGuard.invalidate();
    attachmentGuard.invalidate();
  });

  function selectAvailableFolder(available: MailFolder[]) {
    if (
      folder() &&
      available.some((item) => item.name === folder()!.name && item.selectable)
    )
      return;
    const next =
      available.find(
        (item) =>
          item.selectable &&
          (item.kind.toLowerCase().includes("inbox") ||
            item.name.toUpperCase() === "INBOX"),
      ) ?? available.find((item) => item.selectable);
    if (next) selectFolder(next);
    else {
      listGuard.invalidate();
      readerGuard.invalidate();
      setFolder(undefined);
      setMessages([]);
      setSelected(undefined);
      setDetail(undefined);
      setListLoading(false);
      setReaderLoading(false);
    }
  }

  async function loadFolders(cached = false) {
    if (folderLoading()) return;
    setFolderLoading(true);
    setFolderError("");
    if (cached) {
      try {
        const saved = await listCachedFolders(accountId);
        if (alive && saved.length) {
          setFolders(saved);
          selectAvailableFolder(saved);
        }
      } catch {
        /* 网络刷新仍然可用。 */
      }
    }
    try {
      const refreshed = await refreshFolders(accountId);
      if (alive) {
        setFolders(refreshed);
        selectAvailableFolder(refreshed);
      }
    } catch (error) {
      if (alive)
        setFolderError(
          `文件夹刷新失败：${errorMessage(error)}${folders().length ? "，当前显示本地缓存。" : ""}`,
        );
    } finally {
      if (alive) setFolderLoading(false);
    }
  }

  function selectFolder(next: MailFolder) {
    if (!next.selectable || next.name === folder()?.name) return;
    readerGuard.invalidate();
    attachmentGuard.invalidate();
    setSelected(undefined);
    setDetail(undefined);
    setReaderError("");
    setReaderLoading(false);
    setReadBusy(false);
    setAttachmentBusy(false);
    setNotice("");
    setFolder(next);
    setMessages([]);
    setUidValidity(null);
    setHasMore(false);
    void loadMessages(next, true);
  }

  async function loadMessages(target: MailFolder, cached: boolean) {
    const token = listGuard.next();
    setListLoading(true);
    setMoreLoading(false);
    setListError("");
    if (cached) {
      try {
        const page = await listCachedMessages(accountId, target.name);
        if (alive && listGuard.valid(token)) {
          setMessages(page.messages);
          setUidValidity(page.uidValidity);
          setHasMore(page.hasMore);
        }
      } catch {
        /* 读取缓存失败仍尝试服务器。 */
      }
    }
    try {
      const page = await syncFolder(accountId, target.name);
      if (!alive || !listGuard.valid(token)) return;
      const chosen = selected();
      if (chosen && !selectionRemainsInPage(chosen, page)) {
        readerGuard.invalidate();
        attachmentGuard.invalidate();
        setSelected(undefined);
        setDetail(undefined);
        setReaderLoading(false);
        setReaderError("");
        setReadBusy(false);
        setAttachmentBusy(false);
      }
      setMessages(page.messages);
      setUidValidity(page.uidValidity);
      setHasMore(page.hasMore);
    } catch (error) {
      if (alive && listGuard.valid(token))
        setListError(
          `同步失败：${errorMessage(error)}${messages().length ? "。正在显示本地缓存，可继续阅读。" : ""}`,
        );
    } finally {
      if (alive && listGuard.valid(token)) setListLoading(false);
    }
  }

  async function loadMore() {
    const target = folder();
    if (!target || listLoading() || moreLoading() || !hasMore()) return;
    const beforeUid = messages()[messages().length - 1]?.uid;
    if (!beforeUid) return;
    const token = listGuard.next();
    setMoreLoading(true);
    setListError("");
    try {
      let page;
      try {
        page = await syncFolder(
          accountId,
          target.name,
          beforeUid,
          uidValidity(),
        );
      } catch (error) {
        page = await listCachedMessages(accountId, target.name, beforeUid);
        if (alive && listGuard.valid(token))
          setListError(
            `无法连接服务器：${errorMessage(error)}。仅加载本地已缓存的邮件。`,
          );
      }
      if (alive && listGuard.valid(token)) {
        if (page.uidValidity !== null && page.uidValidity !== uidValidity()) {
          readerGuard.invalidate();
          attachmentGuard.invalidate();
          setSelected(undefined);
          setDetail(undefined);
          setReaderLoading(false);
          setReaderError("");
          setReadBusy(false);
          setAttachmentBusy(false);
          setMessages(page.messages);
          setUidValidity(page.uidValidity);
          setHasMore(page.hasMore);
          return;
        }
        setMessages((current) =>
          mergeMessagePages(current, page.messages, page.uidValidity),
        );
        setHasMore(page.hasMore);
      }
    } catch (error) {
      if (alive && listGuard.valid(token)) setListError(errorMessage(error));
    } finally {
      if (alive && listGuard.valid(token)) setMoreLoading(false);
    }
  }

  function refFor(message: MessageSummary): MessageRef {
    return {
      accountId,
      folder: folder()!.name,
      uid: message.uid,
      uidValidity: message.uidValidity,
    };
  }
  function applyRead(uid: number, uidValidity: number, read: boolean) {
    const transform = (message: MessageSummary) =>
      message.uid === uid && message.uidValidity === uidValidity
        ? { ...message, unread: !read }
        : message;
    setMessages((current) => current.map(transform));
    setSelected((current) => (current ? transform(current) : current));
    setDetail((current) =>
      current ? { ...current, summary: transform(current.summary) } : current,
    );
    const target = folder();
    if (target) {
      const key = detailKey({ accountId, folder: target.name, uid, uidValidity });
      const cached = detailCache.get(key);
      if (cached)
        detailCache.set(key, { ...cached, summary: transform(cached.summary) });
    }
  }

  async function openMessage(message: MessageSummary) {
    if (!folder()) return;
    const token = readerGuard.next();
    attachmentGuard.invalidate();
    const ref = refFor(message);
    const key = detailKey(ref);
    setSelected(message);
    setReaderError("");
    setReadBusy(false);
    setAttachmentBusy(false);
    // 最近打开过的邮件直接从内存显示，不经过 IPC，也不出现加载占位。
    const remembered = detailCache.get(key);
    if (remembered) {
      setDetail({
        ...remembered,
        summary: { ...remembered.summary, unread: message.unread },
      });
      setReaderLoading(false);
    } else {
      setDetail(undefined);
      setReaderLoading(true);
    }
    try {
      const loaded = remembered ?? (await getMessage(ref));
      if (!alive || !readerGuard.valid(token)) return;
      detailCache.set(key, loaded);
      if (!remembered) setDetail(loaded);
      setReaderLoading(false);
      if (message.unread) {
        setReadBusy(true);
        try {
          await setMessageRead(ref, true);
          if (alive && readerGuard.valid(token))
            applyRead(ref.uid, ref.uidValidity, true);
        } catch {
          if (alive && readerGuard.valid(token))
            setNotice("邮件已打开；已读状态暂未同步，可恢复连接后重试。");
        } finally {
          if (alive && readerGuard.valid(token)) setReadBusy(false);
        }
      }
    } catch (error) {
      if (alive && readerGuard.valid(token))
        setReaderError(errorMessage(error));
    } finally {
      if (alive && readerGuard.valid(token)) setReaderLoading(false);
    }
  }

  async function toggleRead() {
    const loaded = detail();
    if (!loaded || readBusy()) return;
    const ref = refFor(loaded.summary);
    const read = loaded.summary.unread;
    const token = readerGuard.next();
    setReadBusy(true);
    try {
      await setMessageRead(ref, read);
      if (alive && readerGuard.valid(token))
        applyRead(ref.uid, ref.uidValidity, read);
    } catch (error) {
      if (alive && readerGuard.valid(token))
        setNotice(`更新已读状态失败：${errorMessage(error)}`);
    } finally {
      if (alive && readerGuard.valid(token)) setReadBusy(false);
    }
  }

  async function download(index: number) {
    const loaded = detail();
    if (!loaded || attachmentBusy()) return;
    const ref = refFor(loaded.summary);
    const token = attachmentGuard.next();
    setAttachmentBusy(true);
    try {
      const path = await saveAttachment(ref, index);
      if (alive && attachmentGuard.valid(token) && path)
        setNotice(`附件已保存：${path}`);
    } catch (error) {
      if (alive && attachmentGuard.valid(token))
        setNotice(`附件保存失败：${errorMessage(error)}`);
    } finally {
      if (alive && attachmentGuard.valid(token)) setAttachmentBusy(false);
    }
  }

  async function compose(requested?: ComposeMessage) {
    if (draftLoading() || composer()) return;
    setDraftLoading(true);
    try {
      const existing = await getDraft(accountId);
      if (!alive) return;
      if (existing && requested) {
        setConflictError("");
        setDraftConflict({ existing, requested });
        queueMicrotask(() => {
          if (alive) conflictDialog.showModal();
        });
      } else
        setComposer({
          initial: existing ?? requested ?? emptyMessage(),
          saveInitially: !existing && !!requested,
        });
    } catch (error) {
      if (alive)
        setNotice(
          `无法读取草稿：${errorMessage(error)}。请重试，避免覆盖已有草稿。`,
        );
    } finally {
      if (alive) setDraftLoading(false);
    }
  }

  async function replaceDraft() {
    const conflict = draftConflict();
    if (!conflict || draftLoading()) return;
    setDraftLoading(true);
    try {
      await clearDraft(accountId);
      if (alive) {
        conflictDialog.close();
        setDraftConflict(undefined);
        setComposer({ initial: conflict.requested, saveInitially: true });
      }
    } catch (error) {
      if (alive) setConflictError(`未能舍弃草稿：${errorMessage(error)}`);
    } finally {
      if (alive) setDraftLoading(false);
    }
  }

  function sent(result: SendResult) {
    setComposer(undefined);
    setNotice(
      result.warning ? `邮件已发送。${result.warning}` : "邮件已发送。",
    );
    const target = folder();
    if (target && /sent/i.test(target.kind)) void loadMessages(target, false);
  }

  return (
    <div class="mail-app">
      <aside class="sidebar mail-sidebar">
        <div class="brand">
          <span class="brand-symbol">
            <Icon name="mail" />
          </span>
          <div>
            <strong>WispMail</strong>
            <span>让邮件，轻一点</span>
          </div>
        </div>
        <label class="mail-account-select">
          <span class="visually-hidden">当前邮箱账户</span>
          <select
            value={props.account.id}
            onChange={(event) =>
              props.onAccountChange(event.currentTarget.value)
            }
          >
            <For each={props.accounts}>
              {(account) => (
                <option value={account.id}>
                  {account.displayName ? `${account.displayName} · ` : ""}
                  {account.email}
                </option>
              )}
            </For>
          </select>
        </label>
        <button
          type="button"
          class="button button--primary compose-launch"
          disabled={draftLoading()}
          onClick={() => void compose()}
        >
          <Icon name="compose" />
          {draftLoading() ? "正在读取草稿…" : "写邮件 / 继续草稿"}
        </button>
        <FolderPane
          folders={folders()}
          selected={folder()?.name ?? ""}
          loading={folderLoading()}
          error={folderError()}
          onSelect={selectFolder}
          onRefresh={() => void loadFolders()}
        />
        <button
          type="button"
          class="button button--secondary mail-settings"
          onClick={props.onSettings}
        >
          <Icon name="settings" />
          邮箱账户与设置
        </button>
        <div class="mail-local-note">
          <Icon name="shield" />
          <span>
            邮件缓存在本机
            <br />
            离线可读已下载正文
          </span>
        </div>
      </aside>
      <MessageList
        title={folder()?.displayName || folder()?.name || ""}
        messages={messages()}
        selected={selected()}
        loading={listLoading()}
        loadingMore={moreLoading()}
        hasMore={hasMore()}
        error={listError()}
        onSelect={(message) => void openMessage(message)}
        onRefresh={() => {
          const target = folder();
          if (target) void loadMessages(target, false);
        }}
        onLoadMore={() => void loadMore()}
      />
      <section class="reader-pane" aria-label="邮件阅读">
        <Show when={notice()}>
          <div class="reader-notice" role="status">
            <span>{notice()}</span>
            <button
              type="button"
              class="icon-button"
              aria-label="关闭邮件提示"
              onClick={() => setNotice("")}
            >
              <Icon name="close" />
            </button>
          </div>
        </Show>
        <Show
          when={!readerLoading()}
          fallback={
            <div class="mail-empty" role="status">
              <span class="spinner" />
              <strong>正在读取邮件…</strong>
            </div>
          }
        >
          <Show
            when={!readerError()}
            fallback={
              <div class="mail-empty">
                <Icon name="mail" />
                <strong>邮件暂时无法打开</strong>
                <p>{readerError()}</p>
                <button
                  type="button"
                  class="button button--secondary"
                  onClick={() => {
                    const target = selected();
                    if (target) void openMessage(target);
                  }}
                >
                  重试
                </button>
              </div>
            }
          >
            <Show
              when={detail()}
              fallback={
                <div class="mail-empty reader-empty">
                  <span class="reader-empty-icon">
                    <Icon name="mail" />
                  </span>
                  <strong>留一点空间，给重要的邮件。</strong>
                  <p>从左侧选择一封邮件开始阅读。</p>
                </div>
              }
            >
              {(loaded) => (
                <MessageView
                  message={loaded()}
                  readBusy={readBusy()}
                  attachmentBusy={attachmentBusy()}
                  onReply={(all) =>
                    void compose(replyMessage(loaded(), props.account.email, all))
                  }
                  onToggleRead={() => void toggleRead()}
                  onSaveAttachment={(index) => void download(index)}
                />
              )}
            </Show>
          </Show>
        </Show>
      </section>
      <Show when={composer()} keyed>
        {(state) => (
          <Composer
            account={props.account}
            initial={state.initial}
            saveInitially={state.saveInitially}
            onClose={() => setComposer(undefined)}
            onSent={sent}
          />
        )}
      </Show>
      <Show when={draftConflict()} keyed>
        {(conflict) => (
          <dialog
            ref={conflictDialog}
            class="account-dialog delete-dialog"
            aria-labelledby="draft-conflict-title"
            onCancel={(event) => {
              event.preventDefault();
              if (!draftLoading()) {
                conflictDialog.close();
                setDraftConflict(undefined);
              }
            }}
          >
            <div class="dialog-body">
              <h2 id="draft-conflict-title">这个邮箱还有未发送的草稿</h2>
              <p class="muted">继续编辑原草稿，或舍弃它后开始本次回复。</p>
              <p>{conflict.existing.subject || "（无主题草稿）"}</p>
              <Show when={conflictError()}>
                <p class="error-message" role="alert">
                  {conflictError()}
                </p>
              </Show>
            </div>
            <footer class="dialog-footer">
              <div class="button-group">
                <button
                  type="button"
                  class="button button--text"
                  disabled={draftLoading()}
                  onClick={() => {
                    conflictDialog.close();
                    setDraftConflict(undefined);
                  }}
                >
                  取消
                </button>
                <button
                  type="button"
                  class="button button--secondary"
                  disabled={draftLoading()}
                  onClick={() => {
                    conflictDialog.close();
                    setDraftConflict(undefined);
                    setComposer({
                      initial: conflict.existing,
                      saveInitially: false,
                    });
                  }}
                >
                  继续原草稿
                </button>
                <button
                  type="button"
                  class="button button--danger"
                  disabled={draftLoading()}
                  onClick={() => void replaceDraft()}
                >
                  舍弃并回复
                </button>
              </div>
            </footer>
          </dialog>
        )}
      </Show>
    </div>
  );
}
