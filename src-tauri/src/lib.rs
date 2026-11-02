//! WispMail 后端库入口。
//!
//! 代码按职责分层：
//! - [`account`]：邮箱账户模型、自动配置、凭据管理。
//! - [`sync`]：与邮件服务器通信（IMAP / POP3 / SMTP）。
//! - [`storage`]：本地缓存、全文搜索与设置持久化。
//!
//! 界面（前端 SolidJS）通过 Tauri command 调用这里暴露的能力。

pub mod account;
mod commands;
pub mod mail;
mod mail_commands;
pub mod storage;
pub mod sync;

use tauri::Manager;

/// 返回应用名称与版本，供前端在"关于"中展示，并验证前后端连通。
#[tauri::command]
fn app_version() -> String {
    format!("WispMail {}", env!("CARGO_PKG_VERSION"))
}

/// 根据邮箱地址推断服务器配置；地址不合法时返回 null。
#[tauri::command]
fn discover_config(email: &str) -> Option<account::DiscoveredConfig> {
    account::autodiscover(email)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // 初始化错误也交给界面展示，避免应用直接退出而没有可读提示。
            let service = (|| {
                let directory = app
                    .path()
                    .app_data_dir()
                    .map_err(|_| "无法定位应用数据目录。".to_string())?;
                std::fs::create_dir_all(&directory)
                    .map_err(|_| "无法创建应用数据目录，请检查目录写入权限。".to_string())?;
                account::AccountService::open(directory.join("wispmail.sqlite3"))
            })();
            app.manage(commands::AccountsState::new(service));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_version,
            discover_config,
            commands::list_accounts,
            commands::test_account_connection,
            commands::save_account,
            commands::delete_account,
            mail_commands::list_cached_folders,
            mail_commands::refresh_folders,
            mail_commands::list_cached_messages,
            mail_commands::sync_folder,
            mail_commands::get_message,
            mail_commands::set_message_read,
            mail_commands::save_attachment,
            mail_commands::open_email_link,
            mail_commands::get_draft,
            mail_commands::save_draft,
            mail_commands::clear_draft,
            mail_commands::send_message,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
