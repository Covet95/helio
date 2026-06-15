// Tauri commands and state management
mod adapters;
mod commands;
mod db;
mod models;
mod model_fetch;
mod session_history;

use commands::AppState;
use db::Database;
use std::sync::Mutex;

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
        .manage(AppState { db: Mutex::new(db) })
        .invoke_handler(tauri::generate_handler![
            commands::list_profiles,
            commands::get_profile,
            commands::add_profile,
            commands::update_profile,
            commands::delete_profile,
            commands::switch_profile,
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
