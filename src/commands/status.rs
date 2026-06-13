use crate::db::Database;
use crate::models::{ApiProfile, TargetApp};
use serde::{Serialize, Deserialize};
use std::sync::Mutex;
use tauri::State;
use super::AppState;

#[derive(Debug, Serialize, Deserialize)]
pub struct StatusInfo {
    pub claude_code: Option<TargetStatus>,
    pub codex: Option<TargetStatus>,
    pub database: DatabaseInfo,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TargetStatus {
    pub profile: Option<ApiProfile>,
    pub connected: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DatabaseInfo {
    pub size: u64,
    pub profile_count: usize,
    pub path: String,
}

#[tauri::command]
pub async fn get_status(
    state: State<'_, AppState>
) -> Result<StatusInfo, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;

    // Claude Code status
    let claude_code_profile = db.get_active_profile_full(TargetApp::ClaudeCode)
        .map_err(|e| e.to_string())?;
    let claude_code = Some(TargetStatus {
        profile: claude_code_profile,
        connected: true, // TODO: 实际检测连接状态
    });

    // Codex status
    let codex_profile = db.get_active_profile_full(TargetApp::Codex)
        .map_err(|e| e.to_string())?;
    let codex = Some(TargetStatus {
        profile: codex_profile,
        connected: false, // Codex 暂未完全支持
    });

    // Database info
    let profiles = db.list_profiles().map_err(|e| e.to_string())?;
    let db_path = dirs::home_dir()
        .unwrap()
        .join(".switch-api")
        .join("db.sqlite");
    let size = std::fs::metadata(&db_path)
        .map(|m| m.len())
        .unwrap_or(0);

    Ok(StatusInfo {
        claude_code,
        codex,
        database: DatabaseInfo {
            size,
            profile_count: profiles.len(),
            path: db_path.to_string_lossy().to_string(),
        },
    })
}
