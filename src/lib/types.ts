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
