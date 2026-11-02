import type { Account, AccountInput } from "../../lib/types.ts";

export type ConfigurationMode = "automatic" | "manual";

export function shouldDiscoverConfig(
  mode: ConfigurationMode,
  email: string,
  lastEmail: string,
  force = false,
) {
  return (
    mode === "automatic" &&
    email.trim().length > 0 &&
    (force || email.trim() !== lastEmail)
  );
}

/** 老账户可能没有独立用户名字段；空值始终表示使用邮箱地址。 */
export function createAccountDraft(account?: Account): AccountInput {
  return {
    id: account?.id,
    email: account?.email ?? "",
    displayName: account?.displayName ?? "",
    incomingProtocol: "imap",
    incoming: account
      ? { ...account.incoming }
      : { host: "", port: 993, encryption: "tls" },
    outgoing: account
      ? { ...account.outgoing }
      : { host: "", port: 465, encryption: "tls" },
    incomingUsername: account?.incomingUsername ?? "",
    outgoingUsername: account?.outgoingUsername ?? "",
    useSamePassword: account?.useSamePassword ?? true,
    auth: "password",
    password: "",
    outgoingPassword: "",
  };
}

export function requiresOutgoingPassword(
  draft: AccountInput,
  account?: Account,
): boolean {
  return (
    !draft.useSamePassword && (!account || account.useSamePassword !== false)
  );
}

export function setPasswordSharing(
  draft: AccountInput,
  shared: boolean,
): AccountInput {
  return { ...draft, useSamePassword: shared, outgoingPassword: "" };
}

/** 清理地址周边空白；密码保持原样，隐藏的 SMTP 密码永远不提交。 */
export function accountInputForSubmission(draft: AccountInput): AccountInput {
  return {
    ...draft,
    email: draft.email.trim(),
    displayName: draft.displayName.trim(),
    incomingUsername: draft.incomingUsername.trim(),
    outgoingUsername: draft.outgoingUsername.trim(),
    outgoingPassword: draft.useSamePassword ? null : draft.outgoingPassword,
  };
}

/** 每次手动配置或切换模式使旧结果失效，关闭弹窗后也不会写回。 */
export function createDiscoveryGuard() {
  let revision = 0;
  let request = 0;
  return {
    invalidate() {
      revision++;
    },
    begin() {
      return { request: ++request, revision };
    },
    isLatest(token: { request: number }) {
      return token.request === request;
    },
    canApply(token: { request: number; revision: number }) {
      return token.request === request && token.revision === revision;
    },
  };
}
