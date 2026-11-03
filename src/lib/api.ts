/** 与 Rust 后端通信的封装。所有 Tauri command 调用集中在此，便于管理与测试。 */
import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  Account,
  AccountInput,
  ConnectionReport,
  DiscoveredConfig,
  OAuthProviderInfo,
  ComposeMessage,
  ForwardAttachments,
  ForwardSource,
  MailFolder,
  MessageDetail,
  MessagePage,
  MessageRef,
  MessageSet,
  MessageFlag,
  OrganizeAction,
  OrganizeResult,
  SearchHit,
  SendResult,
} from "./types";

export const desktopAvailable = isTauri();

function command<T>(name: string, args?: Record<string, unknown>): Promise<T> {
  if (!desktopAvailable) {
    return Promise.reject(
      new Error("请在 WispMail 桌面应用中使用账户连接与保存功能。"),
    );
  }
  return invoke<T>(name, args);
}

/** 后端在收件箱有变化（新邮件、删除等）并完成同步后发出。 */
export interface MailChanged {
  accountId: string;
  folder: string;
  newCount: number;
}

/** 订阅收件箱实时变化；返回取消订阅函数。非桌面环境下不做任何事。 */
export function onMailChanged(
  handler: (event: MailChanged) => void,
): () => void {
  if (!desktopAvailable) return () => {};
  let stopped = false;
  let unlisten: (() => void) | undefined;
  listen<MailChanged>("mail-changed", (event) => handler(event.payload))
    .then((stop) => {
      if (stopped) stop();
      else unlisten = stop;
    })
    .catch(() => {
      /* 事件通道不可用时仍可手动刷新。 */
    });
  return () => {
    stopped = true;
    unlisten?.();
  };
}

export function errorMessage(error: unknown): string {
  return error instanceof Error
    ? error.message
    : typeof error === "string"
      ? error
      : "操作未完成，请重试。";
}

/** 获取应用名称与版本（对应后端 `app_version` command）。 */
export function getAppVersion(): Promise<string> {
  return command<string>("app_version");
}

/** 根据邮箱地址推断服务器配置；地址不合法时返回 null。 */
export function discoverConfig(
  email: string,
): Promise<DiscoveredConfig | null> {
  return command<DiscoveredConfig | null>("discover_config", { email });
}

export function listAccounts(): Promise<Account[]> {
  return command<Account[]>("list_accounts");
}

export function testAccountConnection(
  input: AccountInput,
): Promise<ConnectionReport> {
  return command<ConnectionReport>("test_account_connection", { input });
}

/** 保存前，后端会重新验证两台服务器，避免保存未经验证的配置。 */
export function saveAccount(input: AccountInput): Promise<Account> {
  return command<Account>("save_account", { input });
}

export function deleteAccount(id: string): Promise<void> {
  return command<void>("delete_account", { id });
}

export function oauthProviders(): Promise<OAuthProviderInfo[]> {
  return command<OAuthProviderInfo[]>("oauth_providers");
}

/** 在系统浏览器中登录；完成后后端验证收发信并保存账户。传入 accountId 时为重新授权。 */
export function signInOAuth(
  provider: string,
  accountId?: string,
): Promise<Account> {
  return command<Account>("sign_in_oauth", {
    provider,
    accountId: accountId ?? null,
  });
}

export function cancelOAuthSignIn(): Promise<void> {
  return command<void>("cancel_oauth_sign_in");
}

export function listCachedFolders(accountId: string): Promise<MailFolder[]> {
  return command("list_cached_folders", { accountId });
}

export function refreshFolders(accountId: string): Promise<MailFolder[]> {
  return command("refresh_folders", { accountId });
}

export function listCachedMessages(
  accountId: string,
  folder: string,
  beforeUid?: number,
): Promise<MessagePage> {
  return command("list_cached_messages", { accountId, folder, beforeUid });
}

export function syncFolder(
  accountId: string,
  folder: string,
  beforeUid?: number,
  expectedUidValidity?: number | null,
): Promise<MessagePage> {
  return command("sync_folder", {
    accountId,
    folder,
    beforeUid,
    expectedUidValidity,
  });
}

/** 在本地缓存中全文搜索当前账户的所有文件夹（最多 200 条，按日期从新到旧）。 */
export function searchMessages(
  accountId: string,
  query: string,
): Promise<SearchHit[]> {
  return command("search_messages", { accountId, query });
}

export function getMessage(ref: MessageRef): Promise<MessageDetail> {
  return command("get_message", { ...ref });
}

export function setMessageRead(ref: MessageRef, read: boolean): Promise<void> {
  return command("set_message_read", { ...ref, read });
}

/** 批量设置已读或星标；服务器确认每封邮件后才返回成功。 */
export function setMessageFlags(
  set: MessageSet,
  flag: MessageFlag,
  value: boolean,
): Promise<void> {
  return command("set_message_flags", { ...set, flag, value });
}

/** 移动、归档、移到废纸篓或永久删除；返回邮件所到的文件夹（永久删除为 null）。 */
export function organizeMessages(
  set: MessageSet,
  action: OrganizeAction,
): Promise<OrganizeResult> {
  return command("organize_messages", { ...set, action });
}

export function saveAttachment(
  ref: MessageRef,
  index: number,
): Promise<string | null> {
  return command("save_attachment", { ...ref, index });
}

/** 读取要随转发一起发送的原邮件附件（按给定的附件索引）。 */
export function forwardAttachments(
  ref: MessageRef,
  indices: number[],
): Promise<ForwardAttachments> {
  return command("forward_attachments", { ...ref, indices });
}

/** 转发原邮件部分编码后的字节数，撰写窗口据此实时估算邮件大小。 */
export function estimateForwardSize(
  accountId: string,
  source: ForwardSource,
): Promise<number> {
  return command("estimate_forward_size", {
    accountId,
    folder: source.folder,
    uid: source.uid,
    uidValidity: source.uidValidity,
  });
}

/** 仅由正文中明确的链接点击触发，后端只允许 HTTP(S) 和 mailto。 */
export function openEmailLink(url: string): Promise<void> {
  return command("open_email_link", { url });
}

export function getDraft(accountId: string): Promise<ComposeMessage | null> {
  return command("get_draft", { accountId });
}

export function saveDraft(
  accountId: string,
  draft: ComposeMessage,
): Promise<void> {
  return command("save_draft", { accountId, draft });
}

export function clearDraft(accountId: string): Promise<void> {
  return command("clear_draft", { accountId });
}

export function sendMessage(
  accountId: string,
  message: ComposeMessage,
): Promise<SendResult> {
  return command("send_message", { accountId, message });
}
