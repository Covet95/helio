mod adapters;
mod cli;
mod db;
mod models;
mod utils;

#[cfg(feature = "gui")]
mod commands;

use anyhow::Result;

#[cfg(not(feature = "gui"))]
use clap::Parser;

#[cfg(not(feature = "gui"))]
fn main() -> Result<()> {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // 解析命令行参数
    let cli = cli::Cli::parse();

    // 执行命令
    if let Err(e) = cli::execute(cli) {
        utils::error(&format!("{}", e));
        std::process::exit(1);
    }

    Ok(())
}

#[cfg(feature = "gui")]
fn main() {
    use commands::AppState;
    use db::Database;
    use std::sync::Mutex;

    // 打开数据库
    let db_path = dirs::home_dir()
        .expect("Failed to get home directory")
        .join(".switch-api")
        .join("db.sqlite");

    // 确保目录存在
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).expect("Failed to create database directory");
    }

    let db = Database::open(&db_path).expect("Failed to open database");

    tauri::Builder::default()
        .manage(AppState {
            db: Mutex::new(db),
        })
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
