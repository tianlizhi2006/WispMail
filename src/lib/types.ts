/** 前后端共享的核心数据类型。应与后端 `account` 层的模型保持一致。 */

export type Encryption = "none" | "starttls" | "tls";
export type AuthMethod = "password" | "oauth2";
export type IncomingProtocol = "imap" | "pop3";

export interface ServerConfig {
  host: string;
  port: number;
  encryption: Encryption;
}

export interface Account {
  id: string;
  email: string;
  displayName: string;
  incomingProtocol: IncomingProtocol;
  incoming: ServerConfig;
  outgoing: ServerConfig;
  auth: AuthMethod;
}

/** 根据邮箱地址自动识别得到的配置建议（对应后端 `DiscoveredConfig`）。 */
export interface DiscoveredConfig {
  /** 服务商名称；按命名规则猜测时为 null。 */
  provider: string | null;
  /** builtin：内置表，可信；guess：推测，需要用户核对。 */
  source: "builtin" | "guess";
  incomingProtocol: IncomingProtocol;
  incoming: ServerConfig;
  outgoing: ServerConfig;
  auth: AuthMethod;
  note: string | null;
}

/** 邮件列表项（摘要信息，正文按需加载）。 */
export interface MessageSummary {
  id: string;
  from: string;
  subject: string;
  snippet: string;
  date: string;
  unread: boolean;
  flagged: boolean;
  hasAttachments: boolean;
}
