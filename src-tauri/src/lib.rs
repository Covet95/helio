// Tauri commands and state management
mod adapters;
mod commands;
mod db;
mod models;
mod model_fetch;
mod session_history;
mod tray;

use commands::AppState;
use db::Database;
use std::sync::Mutex;
use tauri::WindowEvent;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 初始化数据库
    let db_path = dirs::home_dir()
        .expect("Failed to get home directory")
        .join(".switch-api")
        .join("db.sqlite");

    // 确保目录存在
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).expect("Failed to create database directory");
    }

    // 打开数据库连接
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
            // 关窗 → 隐藏到状态栏，不退出
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_profiles,
            commands::get_profile,
            commands::add_profile,
            commands::update_profile,
            commands::delete_profile,
            commands::switch_profile,
            commands::copy_text,
            commands::get_shared_config,
            commands::save_shared_config,
            commands::get_status,
            commands::export_database,
            commands::import_database,
            commands::scan_local_mcp_servers,
            commands::scan_local_skills,
            commands::get_local_config_info,
            commands::scan_local_api,
            commands::import_shared_config,
            commands::scan_cc_switch,
            commands::import_cc_switch,
            model_fetch::fetch_models,
            model_fetch::test_model,
            session_history::list_sessions,
            session_history::read_session_preview,
            session_history::delete_session,
            session_history::delete_sessions,
            session_history::cleanup_sessions,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
