use crate::db::Database;
use crate::models::{ApiProfile, TargetApp};
use std::sync::Mutex;
use tauri::State;

pub struct AppState {
    pub db: Mutex<Database>,
}

#[tauri::command]
pub async fn list_profiles(
    state: State<'_, AppState>
) -> Result<Vec<ApiProfile>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.list_profiles().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_profile(
    name: String,
    state: State<'_, AppState>
) -> Result<ApiProfile, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.get_profile_by_name(&name).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn add_profile(
    profile: ApiProfile,
    state: State<'_, AppState>
) -> Result<i64, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.add_profile(&profile).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_profile(
    profile: ApiProfile,
    state: State<'_, AppState>
) -> Result<(), String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.update_profile(&profile).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_profile(
    name: String,
    state: State<'_, AppState>
) -> Result<bool, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.delete_profile(&name).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn switch_profile(
    target_app: String,
    profile_name: String,
    state: State<'_, AppState>
) -> Result<(), String> {
    use crate::adapters::get_adapter;

    let target = TargetApp::from_str(&target_app)
        .ok_or_else(|| format!("Unknown target app: {}", target_app))?;

    let db = state.db.lock().map_err(|e| e.to_string())?;

    // 获取 API Profile
    let api_profile = db.get_profile_by_name(&profile_name)
        .map_err(|e| e.to_string())?;

    // 获取共享配置
    let shared_config = db.get_shared_config(target)
        .map_err(|e| e.to_string())?
        .map(|c| c.config)
        .unwrap_or_else(|| serde_json::json!({}));

    // 获取适配器
    let adapter = get_adapter(target);

    // 备份当前配置
    if adapter.config_path().exists() {
        adapter.backup_config().map_err(|e| e.to_string())?;
    }

    // 合并配置
    let merged = adapter.merge_config(&api_profile, &shared_config);

    // 写入配置
    adapter.write_config(&merged).map_err(|e| e.to_string())?;

    // 应用工具特定的 API 凭据（如 Codex 的 auth.json、Gemini 的 .env）
    adapter.apply_api_credentials(&api_profile).map_err(|e| e.to_string())?;

    // 更新活动记录
    db.set_active_profile(target, api_profile.id.unwrap())
        .map_err(|e| e.to_string())?;

    Ok(())
}

// 新增：导出数据库
#[tauri::command]
pub async fn export_database(
    output_path: String,
    state: State<'_, AppState>
) -> Result<(), String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;

    // 获取数据库文件路径
    let db_path = dirs::home_dir()
        .ok_or("Failed to get home directory")?
        .join(".switch-api")
        .join("db.sqlite");

    // 复制数据库文件
    std::fs::copy(&db_path, &output_path)
        .map_err(|e| format!("Failed to export database: {}", e))?;

    Ok(())
}

// 新增：导入数据库
#[tauri::command]
pub async fn import_database(
    input_path: String,
    state: State<'_, AppState>
) -> Result<(), String> {
    let db_path = dirs::home_dir()
        .ok_or("Failed to get home directory")?
        .join(".switch-api")
        .join("db.sqlite");

    // 备份当前数据库
    let backup_path = db_path.with_extension("sqlite.backup");
    if db_path.exists() {
        std::fs::copy(&db_path, &backup_path)
            .map_err(|e| format!("Failed to backup database: {}", e))?;
    }

    // 复制新数据库
    std::fs::copy(&input_path, &db_path)
        .map_err(|e| format!("Failed to import database: {}", e))?;

    // 重新加载数据库
    drop(state.db.lock().map_err(|e| e.to_string())?);

    Ok(())
}
