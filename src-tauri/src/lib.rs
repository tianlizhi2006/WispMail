//! WispMail 后端库入口。
//!
//! 代码按职责分层：
//! - [`account`]：邮箱账户模型、自动配置、凭据管理。
//! - [`sync`]：与邮件服务器通信（IMAP / POP3 / SMTP）。
//! - [`storage`]：本地缓存、全文搜索与设置持久化。
//!
//! 界面（前端 SolidJS）通过 Tauri command 调用这里暴露的能力。

pub mod account;
pub mod storage;
pub mod sync;

/// 返回应用名称与版本，供前端在"关于"中展示，并验证前后端连通。
#[tauri::command]
fn app_version() -> String {
    format!("WispMail {}", env!("CARGO_PKG_VERSION"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![app_version])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
