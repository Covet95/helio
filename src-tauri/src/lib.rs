// Tauri commands and state management
mod commands;
mod model_fetch;
mod session_history;
mod tray;

use commands::AppState;
use std::sync::Mutex;
use switch_api::db::Database;
use switch_api::utils::secure_fs::ensure_private_dir;
use tauri::WindowEvent;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let db_path = dirs::home_dir()
        .expect("Failed to get home directory")
        .join(".switch-api")
        .join("db.sqlite");

    if let Some(parent) = db_path.parent() {
        ensure_private_dir(parent).expect("Failed to create database directory");
    }

    let db = Database::open(&db_path).expect("Failed to open database");

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .manage(AppState { db: Mutex::new(db) })
        .setup(|app| {
            tray::build_tray(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // 关窗 → 隐藏到系统托盘 / 状态栏，不退出（macOS / Windows 通用）
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::main_cmds::list_profiles,
            commands::main_cmds::get_profile,
            commands::main_cmds::add_profile,
            commands::main_cmds::update_profile,
            commands::main_cmds::delete_profile,
            commands::main_cmds::switch_profile,
            commands::main_cmds::failover_profile_keys,
            commands::main_cmds::probe_active_profiles,
            commands::main_cmds::copy_text,
            commands::main_cmds::get_shared_config,
            commands::main_cmds::save_shared_config,
            commands::main_cmds::get_status,
            commands::main_cmds::export_database,
            commands::main_cmds::import_database,
            commands::main_cmds::scan_local_mcp_servers,
            commands::main_cmds::scan_local_skills,
            commands::main_cmds::get_local_config_info,
            commands::main_cmds::scan_local_api,
            commands::main_cmds::import_shared_config,
            commands::cc_switch::scan_cc_switch,
            commands::cc_switch::import_cc_switch,
            commands::main_cmds::read_codex_config_raw,
            commands::main_cmds::save_codex_config_raw,
            commands::main_cmds::update_codex_fields,
            model_fetch::fetch_models,
            model_fetch::test_model,
            session_history::list_sessions,
            session_history::read_session_preview,
            session_history::delete_session,
            session_history::delete_sessions,
            session_history::cleanup_sessions,
        ])
        // build + run 回调：处理 macOS Dock 点击（applicationShouldHandleReopen）。
        // 关窗 hide 后 has_visible_windows=false，必须主动 show，否则点 Dock 无反应。
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // macOS：关窗 hide 后点 Dock 触发 Reopen，需主动 show。
            // 其他平台无此事件；参数加下划线避免 unused 警告。
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen {
                has_visible_windows,
                ..
            } = event
            {
                if !has_visible_windows {
                    tray::show_main_window(app);
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = (app, event);
            }
        });
}
