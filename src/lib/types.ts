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
  incomingUsername: string;
  outgoingUsername: string;
  useSamePassword: boolean;
  auth: AuthMethod;
}

/** 仅在提交时传递凭据，不写入浏览器存储，也不由后端返回。 */
export interface AccountInput {
  id?: string | null;
  email: string;
  displayName: string;
  incomingProtocol: "imap";
  incoming: ServerConfig;
  outgoing: ServerConfig;
  incomingUsername: string;
  outgoingUsername: string;
  useSamePassword: boolean;
  auth: "password";
  password?: string | null;
  outgoingPassword?: string | null;
}

export interface ConnectionCheck {
  success: boolean;
  message: string;
}

export interface ConnectionReport {
  incoming: ConnectionCheck;
  outgoing: ConnectionCheck;
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
  uid: number;
  uidValidity: number;
  from: string;
  to: string[];
  subject: string;
  snippet: string;
  date: string;
  unread: boolean;
  flagged: boolean;
  hasAttachments: boolean;
  size: number;
}

export interface MailFolder {
  name: string;
  displayName: string;
  selectable: boolean;
  kind: string;
}

export interface MessagePage {
  messages: MessageSummary[];
  hasMore: boolean;
  uidValidity: number | null;
}

export interface MessageAttachment {
  index: number;
  name: string;
  mimeType: string;
  size: number;
}

export interface MessageDetail {
  summary: MessageSummary;
  cc: string[];
  replyTo: string[];
  messageId: string | null;
  references: string[];
  textBody: string;
  htmlBody: string | null;
  inlineResources: InlineResource[];
  htmlBaseUrl: string | null;
  attachments: MessageAttachment[];
}

/** 正文关联的 MIME 图片；attachmentIndex 仅标记可省略重复下载条目的内嵌图。 */
export interface InlineResource {
  contentId: string | null;
  contentLocation: string | null;
  mimeType: string;
  dataBase64: string;
  attachmentIndex: number | null;
}

export interface ComposeAttachment {
  name: string;
  mimeType: string;
  dataBase64: string;
}

export interface ComposeMessage {
  to: string[];
  cc: string[];
  bcc: string[];
  subject: string;
  body: string;
  inReplyTo?: string | null;
  references: string[];
  attachments: ComposeAttachment[];
}

export interface SendResult {
  messageId: string;
  warning: string | null;
}

export interface MessageRef {
  accountId: string;
  folder: string;
  uid: number;
  uidValidity: number;
}

/** 同一文件夹、同一代 UID 下的一组邮件，用于批量操作。 */
export interface MessageSet {
  accountId: string;
  folder: string;
  uidValidity: number;
  uids: number[];
}

export type MessageFlag = "seen" | "flagged";

export type OrganizeAction =
  | { type: "move"; target: string }
  | { type: "archive" }
  | { type: "trash" }
  | { type: "deletePermanently" };

export interface OrganizeResult {
  /** 邮件所到的文件夹；永久删除或未执行时为 null。 */
  target: string | null;
  /** 没有废纸篓或邮件已在废纸篓：服务器未改动，需用户确认永久删除。 */
  needsPermanentDelete: boolean;
}
