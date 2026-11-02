/** 与 Rust 后端通信的封装。所有 Tauri command 调用集中在此，便于管理与测试。 */
import { invoke } from "@tauri-apps/api/core";

/** 获取应用名称与版本（对应后端 `app_version` command）。 */
export function getAppVersion(): Promise<string> {
  return invoke<string>("app_version");
}
