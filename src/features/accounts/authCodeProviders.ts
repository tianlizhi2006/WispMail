/**
 * 使用授权码登录的邮箱：需要先在网页版开启 IMAP/SMTP，再用生成的授权码代替登录密码。
 * 服务器配置由后端的服务商表提供（见 src-tauri/src/account/providers.rs），这里只负责引导。
 */
export interface AuthCodeProvider {
  id: string;
  name: string;
  /** 第一个域名用于选中服务商时补全地址。 */
  domains: string[];
  /** 网页版入口，用系统浏览器打开。 */
  webUrl: string;
  steps: string[];
  codeHint: string;
}

export const AUTH_CODE_PROVIDERS: AuthCodeProvider[] = [
  {
    id: "qq",
    name: "QQ 邮箱",
    domains: ["qq.com", "foxmail.com"],
    webUrl: "https://mail.qq.com/",
    steps: [
      "登录网页版 QQ 邮箱，进入「设置 → 账号与安全 → 安全设置」（旧版为「设置 → 账户」）。",
      "找到「POP3/IMAP/SMTP/Exchange/CardDAV 服务」，开启「IMAP/SMTP 服务」。",
      "按提示完成验证后会显示授权码，复制后粘贴到下方。",
    ],
    codeHint: "16 位字母授权码，不是 QQ 密码",
  },
  {
    id: "163",
    name: "网易 163 邮箱",
    domains: ["163.com"],
    webUrl: "https://mail.163.com/",
    steps: [
      "登录网页版 163 邮箱，进入「设置 → POP3/SMTP/IMAP」。",
      "开启「IMAP/SMTP 服务」，按提示完成验证。",
      "复制生成的授权码（只显示一次）粘贴到下方；以后也可在该页面新增授权码。",
    ],
    codeHint: "网易提供的授权码，不是邮箱登录密码",
  },
  {
    id: "126",
    name: "网易 126 邮箱",
    domains: ["126.com"],
    webUrl: "https://mail.126.com/",
    steps: [
      "登录网页版 126 邮箱，进入「设置 → POP3/SMTP/IMAP」。",
      "开启「IMAP/SMTP 服务」，按提示完成验证。",
      "复制生成的授权码（只显示一次）粘贴到下方。",
    ],
    codeHint: "网易提供的授权码，不是邮箱登录密码",
  },
  {
    id: "yeah",
    name: "网易 yeah.net 邮箱",
    domains: ["yeah.net"],
    webUrl: "https://mail.yeah.net/",
    steps: [
      "登录网页版 yeah.net 邮箱，进入「设置 → POP3/SMTP/IMAP」。",
      "开启「IMAP/SMTP 服务」，按提示完成验证。",
      "复制生成的授权码（只显示一次）粘贴到下方。",
    ],
    codeHint: "网易提供的授权码，不是邮箱登录密码",
  },
];

/** 按邮箱地址的域名找到授权码邮箱；不是这类邮箱时返回 undefined。 */
export function authCodeProviderFor(
  email: string,
): AuthCodeProvider | undefined {
  const domain = email.trim().toLowerCase().split("@")[1];
  if (!domain) return undefined;
  return AUTH_CODE_PROVIDERS.find((provider) =>
    provider.domains.includes(domain),
  );
}

/** 选中服务商时补全或替换地址的域名，保留已输入的用户名部分。 */
export function emailForProvider(
  email: string,
  provider: AuthCodeProvider,
): string {
  const local = email.trim().split("@")[0];
  return local ? `${local}@${provider.domains[0]}` : "";
}

/** 授权码由服务商生成，不含空白；去掉复制时夹带的空格和换行。 */
export function normalizeAuthCode(code: string): string {
  return code.replace(/\s+/g, "");
}
