/** 与 Rust 后端通信的封装。所有 Tauri command 调用集中在此，便于管理与测试。 */
import { invoke, isTauri } from "@tauri-apps/api/core";
import type {
  Account,
  AccountInput,
  ConnectionReport,
  DiscoveredConfig,
  ComposeMessage,
  MailFolder,
  MessageDetail,
  MessagePage,
  MessageRef,
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

export function getMessage(ref: MessageRef): Promise<MessageDetail> {
  return command("get_message", { ...ref });
}

export function setMessageRead(ref: MessageRef, read: boolean): Promise<void> {
  return command("set_message_read", { ...ref, read });
}

export function saveAttachment(
  ref: MessageRef,
  index: number,
): Promise<string | null> {
  return command("save_attachment", { ...ref, index });
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
