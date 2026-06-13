use crate::db::Database;
use crate::models::TargetApp;
use std::sync::Mutex;
use tauri::State;
use super::AppState;

#[tauri::command]
pub async fn get_shared_config(
    target_app: String,
    state: State<'_, AppState>
) -> Result<Option<serde_json::Value>, String> {
    let target = TargetApp::from_str(&target_app)
        .ok_or_else(|| format!("Unknown target app: {}", target_app))?;

    let db = state.db.lock().map_err(|e| e.to_string())?;

    db.get_shared_config(target)
        .map(|opt| opt.map(|sc| sc.config))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn save_shared_config(
    target_app: String,
    config: serde_json::Value,
    state: State<'_, AppState>
) -> Result<(), String> {
    let target = TargetApp::from_str(&target_app)
        .ok_or_else(|| format!("Unknown target app: {}", target_app))?;

    let db = state.db.lock().map_err(|e| e.to_string())?;

    db.save_shared_config(target, config).map_err(|e| e.to_string())
}
