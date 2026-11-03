//! 托盘常驻：关闭窗口时隐藏到系统托盘，收件箱监视和新邮件通知继续工作；
//! 托盘菜单可重新打开窗口、切换开机自启或退出。

use tauri::{
    AppHandle, Manager, Window, WindowEvent,
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};
use tauri_plugin_autostart::ManagerExt;

/// 开机自启时附带的参数：启动后直接留在托盘，不弹出窗口。
pub const MINIMIZED_ARG: &str = "--minimized";

pub fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

pub fn setup(app: &tauri::App) -> tauri::Result<()> {
    let handle = app.handle();
    let open = MenuItem::with_id(handle, "open", "打开 WispMail", true, None::<&str>)?;
    let autostart = CheckMenuItem::with_id(
        handle,
        "autostart",
        "开机自动启动",
        true,
        handle.autolaunch().is_enabled().unwrap_or(false),
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(handle, "quit", "退出 WispMail", true, None::<&str>)?;
    let menu = Menu::with_items(
        handle,
        &[
            &open,
            &PredefinedMenuItem::separator(handle)?,
            &autostart,
            &PredefinedMenuItem::separator(handle)?,
            &quit,
        ],
    )?;
    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("WispMail")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "open" => show_main(app),
            "autostart" => {
                let launcher = app.autolaunch();
                let enable = !launcher.is_enabled().unwrap_or(false);
                let _ = if enable {
                    launcher.enable()
                } else {
                    launcher.disable()
                };
                // 以系统中的实际状态为准，失败时勾选状态不会与系统不一致。
                let _ = autostart.set_checked(launcher.is_enabled().unwrap_or(false));
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    if !std::env::args().any(|arg| arg == MINIMIZED_ARG) {
        show_main(handle);
    }
    Ok(())
}

/// 点击窗口关闭按钮时静默隐藏到托盘而不是退出。
pub fn on_window_event(window: &Window, event: &WindowEvent) {
    if let WindowEvent::CloseRequested { api, .. } = event
        && window.label() == "main"
    {
        api.prevent_close();
        let _ = window.hide();
    }
}
