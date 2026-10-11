// Tauri 2 客户端外壳。
//
// 分工（DEVELOPMENT-PLAN.md §五）：
//   · 业务逻辑全在 src/（Vue 3 + TypeScript）
//   · 平台原生能力在 Rust 侧，但**只做薄桥接**
// 这个文件因此只有四件事：托盘、单实例、关窗不退出、mpv 桥接的命令注册。
//
// mpv 的**参数构造与协议编解码不在本 crate**：那部分在 `whatmedia-mpv`，有单元测试。
// 这里只管起进程、接流、转发事件 —— 没有测试覆盖的代码要尽量少。

mod mpv;

use mpv::{mpv_send, mpv_start, mpv_status, mpv_stop, MpvState};
use tauri::menu::{MenuBuilder, MenuItem};
use tauri::tray::TrayIconBuilder;
// `get_webview_window` / `state` 都来自 Manager，没导入会报"方法不存在"，
// 而错误信息不会提示是 trait 没进作用域。
use tauri::Manager;

#[tauri::command]
fn ping() -> &'static str {
    "pong"
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            // 第二次启动时把已有窗口拉到前台，而不是再开一个 ——
            // 两个实例会抢同一个 mpv 会话，表现为"控制条点了没反应"。
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .manage(MpvState::default())
        .invoke_handler(tauri::generate_handler![
            ping,
            mpv_status,
            mpv_start,
            mpv_send,
            mpv_stop
        ])
        .setup(|app| {
            build_tray(app)?;
            Ok(())
        })
        // 关窗不退出：客户端关了窗口还要在托盘里活着。
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("启动 Tauri 应用失败");
}

fn build_tray(app: &mut tauri::App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "打开 WhatMedia", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = MenuBuilder::new(app).item(&show).item(&quit).build()?;

    TrayIconBuilder::with_id("main")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            "quit" => {
                // 退出前把 mpv 收掉。托盘退出不走窗口关闭事件，
                // 漏了这一步会留下一个孤儿 mpv 窗口。
                let state = app.state::<MpvState>();
                let _ = mpv_stop(state);
                app.exit(0);
            }
            _ => {}
        })
        .build(app)?;
    Ok(())
}
