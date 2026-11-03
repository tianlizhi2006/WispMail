import type { Account, AccountInput } from "../../lib/types.ts";
import { authCodeProviderFor, normalizeAuthCode } from "./authCodeProviders.ts";

/** authcode：QQ / 网易等授权码邮箱；automatic：其他常见邮箱；manual：自建 / 域名邮箱。 */
export type ConfigurationMode = "authcode" | "automatic" | "manual";

export function shouldDiscoverConfig(
  mode: ConfigurationMode,
  email: string,
  lastEmail: string,
  force = false,
) {
  return (
    mode !== "manual" &&
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

/**
 * 授权码邮箱的提交内容：登录名就是邮箱地址，收发信共用同一个授权码，
 * 并去掉复制授权码时夹带的空白。
 */
export function authCodeSubmission(draft: AccountInput): AccountInput {
  return {
    ...accountInputForSubmission({
      ...draft,
      incomingUsername: "",
      outgoingUsername: "",
      useSamePassword: true,
    }),
    password: draft.password
      ? normalizeAuthCode(draft.password)
      : draft.password,
  };
}

/** 编辑已有账户时，按授权码邮箱方式保存的账户继续使用简化表单。 */
export function initialModeFor(
  account?: Account,
): ConfigurationMode | undefined {
  if (!account) return undefined;
  return authCodeProviderFor(account.email) &&
    account.useSamePassword &&
    !account.incomingUsername &&
    !account.outgoingUsername
    ? "authcode"
    : "manual";
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
