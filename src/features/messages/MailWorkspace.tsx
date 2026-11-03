import {
  createMemo,
  createSignal,
  For,
  onCleanup,
  onMount,
  Show,
} from "solid-js";
import {
  clearDraft,
  errorMessage,
  forwardAttachments,
  getDraft,
  getMessage,
  listCachedFolders,
  listCachedMessages,
  onMailChanged,
  organizeMessages,
  refreshFolders,
  saveAttachment,
  searchMessages,
  setMessageFlags,
  setMessageRead,
  syncFolder,
} from "../../lib/api";
import type {
  Account,
  ComposeAttachment,
  ComposeMessage,
  MailFolder,
  MessageDetail,
  MessageFlag,
  MessageRef,
  MessageSet,
  MessageSummary,
  OrganizeAction,
  SearchHit,
  SendResult,
} from "../../lib/types";
import { Icon } from "../../ui/Icon";
import { FolderPane } from "../folders/FolderPane";
import { MessageView } from "../reader/MessageView";
import { Composer } from "./Composer";
import { MessageList } from "./MessageList";
import {
  checkedMessages,
  createLruCache,
  createRequestGuard,
  emptyMessage,
  forwardAttachmentIndexes,
  forwardMessage,
  mergeMessagePages,
  messageKey,
  moveTargets,
  refreshFirstPage,
  removeFromList,
  removeHits,
  replyMessage,
  selectionRemainsInPage,
  toggleChecked,
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
  const [organizeBusy, setOrganizeBusy] = createSignal(false);
  const [pendingDelete, setPendingDelete] = createSignal<MessageSet>();
  const [checked, setChecked] = createSignal<ReadonlySet<string>>(new Set());
  const [forwardBusy, setForwardBusy] = createSignal(false);
  /** 打开的邮件所在的文件夹；打开搜索结果时可能不是左侧选中的文件夹。 */
  const [readerFolder, setReaderFolder] = createSignal<string>();
  const [searchQuery, setSearchQuery] = createSignal("");
  /** 非 null 时列表处于搜索模式。 */
  const [searchHits, setSearchHits] = createSignal<SearchHit[] | null>(null);
  const [searchLoading, setSearchLoading] = createSignal(false);
  const [searchError, setSearchError] = createSignal("");
  const searchGuard = createRequestGuard();
  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  const hitFolders = createMemo(
    () => new Map(searchHits()?.map((hit) => [hit.summary, hit.folder]) ?? []),
  );
  const searching = () => searchHits() !== null || searchLoading();
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
  /** Shift 连续选择的起点：最近一次勾选的邮件。 */
  let checkAnchor: string | undefined;
  let alive = true;
  let conflictDialog!: HTMLDialogElement;
  let deleteDialog!: HTMLDialogElement;

  onMount(() => {
    void loadFolders(true);
    // 后台监视已把新邮件同步进缓存：当前正看着这个文件夹时，直接从缓存刷新列表。
    const stop = onMailChanged((event) => {
      const current = folder();
      if (
        alive &&
        event.accountId === accountId &&
        current &&
        current.name.toUpperCase() === event.folder.toUpperCase()
      )
        void refreshFromCache(current);
    });
    onCleanup(stop);
  });
  onCleanup(() => {
    alive = false;
    if (searchTimer !== undefined) clearTimeout(searchTimer);
    searchGuard.invalidate();
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

  /** 输入停顿 250 毫秒后搜索；清空搜索框回到文件夹列表。 */
  function changeSearch(value: string) {
    setSearchQuery(value);
    if (searchTimer !== undefined) clearTimeout(searchTimer);
    searchGuard.invalidate();
    setSearchError("");
    if (!value.trim()) {
      setSearchHits(null);
      setSearchLoading(false);
      return;
    }
    setSearchLoading(true);
    searchTimer = setTimeout(() => void runSearch(value), 250);
  }

  async function runSearch(value: string) {
    const token = searchGuard.next();
    try {
      const hits = await searchMessages(accountId, value);
      if (alive && searchGuard.valid(token)) setSearchHits(hits);
    } catch (error) {
      if (alive && searchGuard.valid(token)) {
        setSearchHits([]);
        setSearchError(`搜索失败：${errorMessage(error)}`);
      }
    } finally {
      if (alive && searchGuard.valid(token)) setSearchLoading(false);
    }
  }

  function selectFolder(next: MailFolder) {
    if (searching()) changeSearch("");
    if (!next.selectable || next.name === folder()?.name) return;
    readerGuard.invalidate();
    attachmentGuard.invalidate();
    setSelected(undefined);
    setDetail(undefined);
    setReaderFolder(undefined);
    setReaderError("");
    setReaderLoading(false);
    setReadBusy(false);
    setAttachmentBusy(false);
    setNotice("");
    setChecked(new Set<string>());
    checkAnchor = undefined;
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

  /** 合并缓存中的最新一页，保留已加载的更早邮件和当前选择。 */
  async function refreshFromCache(target: MailFolder) {
    // 正在同步时，同步结果本身就包含最新邮件。
    if (listLoading() || moreLoading()) return;
    try {
      const page = await listCachedMessages(accountId, target.name);
      if (!alive || folder()?.name !== target.name || listLoading()) return;
      if (page.uidValidity !== uidValidity()) {
        // 收件箱被重建：旧 UID 已失效，按完整重新同步处理。
        void loadMessages(target, false);
        return;
      }
      setMessages((current) => refreshFirstPage(current, page));
      if (!page.hasMore) setHasMore(false);
    } catch {
      /* 读取缓存失败时保留当前列表，用户仍可手动刷新。 */
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
      folder: readerFolder() ?? folder()!.name,
      uid: message.uid,
      uidValidity: message.uidValidity,
    };
  }
  /**
   * 把服务器已确认的摘要变化应用到列表、搜索结果、阅读区和内存缓存。
   * 只修改同一文件夹中的邮件：不同文件夹的 UID 可能相同。
   */
  function applySummary(
    folderName: string,
    uids: number[],
    uidValidity: number,
    patch: Partial<Pick<MessageSummary, "unread" | "flagged">>,
  ) {
    const targets = new Set(uids);
    const transform = (message: MessageSummary) =>
      targets.has(message.uid) && message.uidValidity === uidValidity
        ? { ...message, ...patch }
        : message;
    if (folder()?.name === folderName)
      setMessages((current) => current.map(transform));
    setSearchHits((current) =>
      current
        ? current.map((hit) =>
            hit.folder === folderName
              ? { ...hit, summary: transform(hit.summary) }
              : hit,
          )
        : current,
    );
    if (readerFolder() === folderName) {
      setSelected((current) => (current ? transform(current) : current));
      setDetail((current) =>
        current ? { ...current, summary: transform(current.summary) } : current,
      );
    }
    for (const uid of uids) {
      const key = detailKey({
        accountId,
        folder: folderName,
        uid,
        uidValidity,
      });
      const cached = detailCache.get(key);
      if (cached)
        detailCache.set(key, { ...cached, summary: transform(cached.summary) });
    }
  }
  const applyRead = (ref: MessageRef, read: boolean) =>
    applySummary(ref.folder, [ref.uid], ref.uidValidity, { unread: !read });

  async function toggleFlag() {
    const loaded = detail();
    const target = readerFolder();
    if (!loaded || !target || readBusy() || organizeBusy()) return;
    const { uid, uidValidity, flagged } = loaded.summary;
    const token = readerGuard.next();
    setReadBusy(true);
    try {
      await setMessageFlags(
        { accountId, folder: target, uidValidity, uids: [uid] },
        "flagged",
        !flagged,
      );
      if (alive && readerGuard.valid(token))
        applySummary(target, [uid], uidValidity, { flagged: !flagged });
    } catch (error) {
      if (alive && readerGuard.valid(token))
        setNotice(`更新星标失败：${errorMessage(error)}`);
    } finally {
      if (alive && readerGuard.valid(token)) setReadBusy(false);
    }
  }

  /** 邮件已离开所在文件夹：从列表和搜索结果移除，并自动打开相邻的邮件。 */
  function afterRemoved(set: MessageSet) {
    const { uids, uidValidity } = set;
    for (const uid of uids)
      detailCache.delete(
        detailKey({ accountId, folder: set.folder, uid, uidValidity }),
      );
    const opened = selected();
    const openedFolder = readerFolder();
    const openedRemoved =
      !!opened &&
      openedFolder === set.folder &&
      opened.uidValidity === uidValidity &&
      uids.includes(opened.uid);
    let next: { message: MessageSummary; folder: string } | undefined;
    const hits = searchHits();
    if (hits) {
      const result = removeHits(
        hits,
        set,
        openedRemoved ? { folder: openedFolder!, summary: opened! } : undefined,
      );
      setSearchHits(result.hits);
      if (result.next)
        next = { message: result.next.summary, folder: result.next.folder };
    }
    // 请求期间用户可能已切换文件夹：列表属于另一个文件夹时不能按 UID 删除。
    if (folder()?.name === set.folder) {
      setChecked((current) => {
        const remaining = new Set(current);
        for (const uid of uids)
          remaining.delete(messageKey({ uid, uidValidity }));
        return remaining;
      });
      const result = removeFromList(
        messages(),
        uidValidity,
        uids,
        openedRemoved && !hits ? opened : undefined,
      );
      setMessages(result.messages);
      if (!hits && result.next)
        next = { message: result.next, folder: set.folder };
    }
    if (!openedRemoved) return;
    if (next) void openMessage(next.message, next.folder);
    else {
      readerGuard.invalidate();
      attachmentGuard.invalidate();
      setSelected(undefined);
      setDetail(undefined);
      setReaderFolder(undefined);
      setReaderLoading(false);
    }
  }

  function folderLabel(name: string | null) {
    if (!name) return "";
    const known = folders().find((item) => item.name === name);
    return known?.displayName || name;
  }

  /** 对一组邮件执行整理动作；单封（阅读区）和多选（列表）共用。 */
  async function runOrganize(set: MessageSet, action: OrganizeAction) {
    if (organizeBusy()) return;
    setOrganizeBusy(true);
    try {
      const result = await organizeMessages(set, action);
      if (!alive) return;
      if (result.needsPermanentDelete) {
        setPendingDelete(set);
        queueMicrotask(() => {
          if (alive) deleteDialog.showModal();
        });
        return;
      }
      afterRemoved(set);
      const what = set.uids.length > 1 ? `${set.uids.length} 封邮件` : "邮件";
      const where = folderLabel(result.target);
      setNotice(
        action.type === "deletePermanently"
          ? `${what}已永久删除。`
          : action.type === "trash"
            ? `${what}已移到「${where}」。`
            : action.type === "archive"
              ? `${what}已归档到「${where}」。`
              : `${what}已移动到「${where}」。`,
      );
      // 归档时服务器可能新建了 Archive 文件夹，刷新左侧列表。
      if (
        result.target &&
        !folders().some((item) => item.name === result.target)
      )
        void loadFolders();
    } catch (error) {
      if (alive) setNotice(`操作未完成：${errorMessage(error)}`);
    } finally {
      if (alive) setOrganizeBusy(false);
    }
  }

  function organize(action: OrganizeAction) {
    const loaded = detail();
    const target = readerFolder();
    if (!loaded || !target) return;
    const { uid, uidValidity } = loaded.summary;
    void runOrganize(
      { accountId, folder: target, uidValidity, uids: [uid] },
      action,
    );
  }

  /** 当前多选的邮件组成的集合；列表中的邮件始终属于同一 UIDVALIDITY。 */
  function checkedSet(): MessageSet | undefined {
    const target = folder();
    const chosen = checkedMessages(messages(), checked());
    if (!target || !chosen.length) return undefined;
    const uidValidity = chosen[0].uidValidity;
    return {
      accountId,
      folder: target.name,
      uidValidity,
      uids: chosen
        .filter((message) => message.uidValidity === uidValidity)
        .map((message) => message.uid),
    };
  }

  function toggleCheck(message: MessageSummary, range: boolean) {
    setChecked((current) =>
      toggleChecked(
        messages(),
        current,
        message,
        range ? checkAnchor : undefined,
      ),
    );
    checkAnchor = messageKey(message);
  }

  function checkAll(value: boolean) {
    setChecked(new Set<string>(value ? messages().map(messageKey) : []));
    checkAnchor = undefined;
  }

  async function batchFlag(flag: MessageFlag, value: boolean) {
    const set = checkedSet();
    if (!set || organizeBusy()) return;
    setOrganizeBusy(true);
    try {
      await setMessageFlags(set, flag, value);
      if (alive)
        applySummary(
          set.folder,
          set.uids,
          set.uidValidity,
          flag === "seen" ? { unread: !value } : { flagged: value },
        );
    } catch (error) {
      if (alive) setNotice(`批量更新未完成：${errorMessage(error)}`);
    } finally {
      if (alive) setOrganizeBusy(false);
    }
  }

  function batchOrganize(action: OrganizeAction) {
    const set = checkedSet();
    if (set) void runOrganize(set, action);
  }

  async function confirmPermanentDelete() {
    const set = pendingDelete();
    if (!set || organizeBusy()) return;
    setOrganizeBusy(true);
    try {
      await organizeMessages(set, { type: "deletePermanently" });
      if (!alive) return;
      deleteDialog.close();
      setPendingDelete(undefined);
      afterRemoved(set);
      setNotice(
        set.uids.length > 1
          ? `${set.uids.length} 封邮件已永久删除。`
          : "邮件已永久删除。",
      );
    } catch (error) {
      if (alive) setNotice(`删除未完成：${errorMessage(error)}`);
    } finally {
      if (alive) setOrganizeBusy(false);
    }
  }

  async function openMessage(message: MessageSummary, inFolder?: string) {
    const folderName = inFolder ?? folder()?.name;
    if (!folderName) return;
    const token = readerGuard.next();
    attachmentGuard.invalidate();
    setReaderFolder(folderName);
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
          if (alive && readerGuard.valid(token)) applyRead(ref, true);
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
      if (alive && readerGuard.valid(token)) applyRead(ref, read);
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

  async function forward() {
    const loaded = detail();
    const target = readerFolder();
    if (!loaded || !target || forwardBusy() || draftLoading() || composer())
      return;
    setForwardBusy(true);
    try {
      let attachments: ComposeAttachment[] = [];
      const indexes = forwardAttachmentIndexes(loaded);
      if (indexes.length) {
        const result = await forwardAttachments(
          refFor(loaded.summary),
          indexes,
        );
        if (!alive) return;
        attachments = result.attachments;
        if (result.skipped.length)
          setNotice(
            `以下附件超出发送限制（合计 10 MB、最多 20 个）或已损坏，未随转发附上：${result.skipped.join("、")}`,
          );
      }
      await compose(forwardMessage(loaded, target, attachments));
    } catch (error) {
      if (alive) setNotice(`无法转发：${errorMessage(error)}`);
    } finally {
      if (alive) setForwardBusy(false);
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
        messages={
          searchHits()?.map((hit) => hit.summary) ??
          (searching() ? [] : messages())
        }
        selected={selected()}
        loading={searching() ? searchLoading() : listLoading()}
        loadingMore={moreLoading()}
        hasMore={hasMore()}
        error={searching() ? searchError() : listError()}
        searchQuery={searchQuery()}
        onSearchQuery={changeSearch}
        searching={searching()}
        folderOf={(message) => folderLabel(hitFolders().get(message) ?? null)}
        checked={checked()}
        batchBusy={organizeBusy()}
        moveTargets={moveTargets(folders(), folder()?.name)}
        onSelect={(message) =>
          void openMessage(
            message,
            searching() ? hitFolders().get(message) : undefined,
          )
        }
        onToggleCheck={toggleCheck}
        onCheckAll={checkAll}
        onBatchFlag={(flag, value) => void batchFlag(flag, value)}
        onBatchOrganize={batchOrganize}
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
                    if (target) void openMessage(target, readerFolder());
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
                  organizeBusy={organizeBusy()}
                  forwardBusy={forwardBusy() || draftLoading()}
                  moveTargets={moveTargets(folders(), readerFolder())}
                  onReply={(all) =>
                    void compose(
                      replyMessage(loaded(), props.account.email, all),
                    )
                  }
                  onForward={() => void forward()}
                  onToggleRead={() => void toggleRead()}
                  onToggleFlag={() => void toggleFlag()}
                  onArchive={() => organize({ type: "archive" })}
                  onDelete={() => organize({ type: "trash" })}
                  onMove={(target) => organize({ type: "move", target })}
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
      <Show when={pendingDelete()}>
        <dialog
          ref={deleteDialog}
          class="account-dialog delete-dialog"
          aria-labelledby="permanent-delete-title"
          onCancel={(event) => {
            event.preventDefault();
            if (!organizeBusy()) {
              deleteDialog.close();
              setPendingDelete(undefined);
            }
          }}
        >
          <div class="dialog-body">
            <span class="delete-symbol">
              <Icon name="trash" />
            </span>
            <h2 id="permanent-delete-title">
              {(pendingDelete()?.uids.length ?? 1) > 1
                ? `永久删除这 ${pendingDelete()!.uids.length} 封邮件？`
                : "永久删除这封邮件？"}
            </h2>
            <p class="muted">
              {folders().find((item) => item.name === pendingDelete()?.folder)
                ?.kind === "trash"
                ? "邮件已在废纸篓中，删除后将从服务器上永久移除，无法恢复。"
                : "服务器上没有废纸篓文件夹，删除后将从服务器上永久移除，无法恢复。"}
            </p>
          </div>
          <footer class="dialog-footer">
            <div class="button-group">
              <button
                type="button"
                class="button button--text"
                disabled={organizeBusy()}
                onClick={() => {
                  deleteDialog.close();
                  setPendingDelete(undefined);
                }}
              >
                取消
              </button>
              <button
                type="button"
                class="button button--danger"
                disabled={organizeBusy()}
                onClick={() => void confirmPermanentDelete()}
              >
                {organizeBusy() ? "正在删除…" : "永久删除"}
              </button>
            </div>
          </footer>
        </dialog>
      </Show>
    </div>
  );
}
