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

/// 初始化日志。
///
/// 核心库在 journal 崩溃恢复、保真合并回退等关键路径埋了 `tracing` 调用，
/// 但此前**无人初始化 subscriber**——所有日志被静默丢弃，出问题只能靠猜。
///
/// 落盘而非只写 stderr：桌面应用从 Finder / Explorer 启动时没有终端，
/// stderr 无处可看。日志文件与数据库同目录，沿用 `ensure_private_dir`
/// 收紧到仅当前用户可读——里面的路径与错误信息不该被其他用户看到。
///
/// 级别用 `RUST_LOG` 覆盖（默认 `info`）。
fn init_tracing(log_path: &std::path::Path) {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};

    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
    else {
        // 日志初始化失败不该阻止应用启动——退回只写 stderr。
        eprintln!("[Helio] 无法打开日志文件 {}", log_path.display());
        return;
    };

    // 日志里有配置路径与错误细节，收紧到仅当前用户可读（Windows 上走 ACL）。
    if let Err(error) = switch_api::utils::secure_fs::ensure_private_file(log_path) {
        eprintln!("[Helio] 无法收紧日志文件权限：{error:#}");
    }

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let subscriber = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(file).with_ansi(false))
        .with(fmt::layer().with_writer(std::io::stderr));

    // 已有全局 subscriber 时忽略错误（重复初始化）。
    let _ = tracing::subscriber::set_global_default(subscriber);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let db_path = dirs::home_dir()
        .expect("Failed to get home directory")
        .join(".switch-api")
        .join("db.sqlite");

    if let Some(parent) = db_path.parent() {
        ensure_private_dir(parent).expect("Failed to create database directory");
    }

    init_tracing(&db_path.with_file_name("helio.log"));

    let db = Database::open(&db_path).expect("Failed to open database");
    // 上次切换可能在写盘与写 DB 之间崩溃，按 journal 恢复半状态（失败不阻止启动）。
    if let Err(error) = switch_api::adapters::journal::recover_interrupted_switch(&db) {
        tracing::error!(error = %format!("{error:#}"), "恢复中断的切换失败");
    }

    // 冷启动同步：以磁盘共享配置为准写回数据库，省去手动扫描导入；
    // 文件缺失或读取失败时跳过，不阻塞启动；只动共享配置，不碰 API 凭据。
    let synced = switch_api::adapters::sync_startup_shared_configs(&db);
    if !synced.is_empty() {
        tracing::info!(synced = ?synced, "启动时同步了共享配置");
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .manage(AppState {
            db: Mutex::new(db),
            config_lock: Mutex::new(()),
        })
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
            commands::main_cmds::assign_legacy_profile,
            commands::main_cmds::delete_legacy_profile,
            commands::main_cmds::add_profile,
            commands::main_cmds::update_profile,
            commands::main_cmds::delete_profile,
            commands::main_cmds::switch_profile,
            commands::status::failover_profile_keys,
            commands::status::probe_active_profiles,
            commands::clipboard::copy_text,
            commands::main_cmds::list_config_backups,
            commands::main_cmds::restore_config_backup,
            commands::status::get_status,
            commands::import_export::export_database,
            commands::import_export::import_database,
            commands::import_export::export_portable_backup,
            commands::import_export::import_portable_backup,
            commands::import_export::export_skills,
            commands::import_export::import_skills,
            commands::main_cmds::get_local_config_info,
            commands::scan::scan_local_api,
            commands::cc_switch::scan_cc_switch,
            commands::cc_switch::import_cc_switch,
            commands::codex_raw::read_codex_config_raw,
            commands::codex_raw::save_codex_config_raw,
            commands::codex_raw::update_codex_fields,
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
