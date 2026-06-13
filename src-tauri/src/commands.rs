// Tauri commands
use crate::db::Database;
use crate::models::{ApiProfile, TargetApp};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tauri::State;

pub struct AppState {
    pub db: Mutex<Database>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StatusInfo {
    pub claude_code: Option<TargetStatus>,
    pub codex: Option<TargetStatus>,
    pub gemini: Option<TargetStatus>,
    pub opencode: Option<TargetStatus>,
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

#[derive(Debug, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub command: String,
    pub args: Vec<String>,
    pub env: Option<std::collections::HashMap<String, String>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LocalConfigInfo {
    pub mcp_servers: std::collections::HashMap<String, McpServerConfig>,
    pub skills: Vec<String>,
    pub hooks: serde_json::Value,
    pub permissions: serde_json::Value,
}

// 新增：扫描本地 MCP 配置
#[tauri::command]
pub async fn scan_local_mcp_servers(
    target_app: String,
) -> Result<std::collections::HashMap<String, McpServerConfig>, String> {
    let target = TargetApp::from_str(&target_app)
        .ok_or_else(|| format!("Unknown target app: {}", target_app))?;

    use crate::adapters::get_adapter;
    let adapter = get_adapter(target);
    let config_path = adapter.config_path();

    if !config_path.exists() {
        return Ok(std::collections::HashMap::new());
    }

    // 读取配置文件
    let content = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("Failed to read config: {}", e))?;

    let config: serde_json::Value =
        serde_json::from_str(&content).map_err(|e| format!("Failed to parse config: {}", e))?;

    // 提取 MCP 服务器配置
    let mcp_servers = config
        .get("mcpServers")
        .or_else(|| config.get("mcp_servers"))
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    Ok(mcp_servers)
}

// 新增：扫描本地 Skills
#[tauri::command]
pub async fn scan_local_skills(target_app: String) -> Result<Vec<String>, String> {
    let target = TargetApp::from_str(&target_app)
        .ok_or_else(|| format!("Unknown target app: {}", target_app))?;

    // Claude Code skills 路径: ~/.claude/skills/
    // Codex skills 路径: ~/.codex/skills/
    let skills_dir = if target == TargetApp::ClaudeCode {
        dirs::home_dir()
            .ok_or("Failed to get home directory")?
            .join(".claude")
            .join("skills")
    } else {
        dirs::home_dir()
            .ok_or("Failed to get home directory")?
            .join(".codex")
            .join("skills")
    };

    if !skills_dir.exists() {
        return Ok(Vec::new());
    }

    let mut skills = Vec::new();
    for entry in std::fs::read_dir(&skills_dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.path().is_dir() {
            if let Some(name) = entry.file_name().to_str() {
                skills.push(name.to_string());
            }
        }
    }

    Ok(skills)
}

// 新增：获取完整的本地配置信息
#[tauri::command]
pub async fn get_local_config_info(target_app: String) -> Result<LocalConfigInfo, String> {
    let target = TargetApp::from_str(&target_app)
        .ok_or_else(|| format!("Unknown target app: {}", target_app))?;

    use crate::adapters::get_adapter;
    let adapter = get_adapter(target);
    let config_path = adapter.config_path();

    let mut info = LocalConfigInfo {
        mcp_servers: std::collections::HashMap::new(),
        skills: Vec::new(),
        hooks: serde_json::json!({}),
        permissions: serde_json::json!({}),
    };

    if !config_path.exists() {
        return Ok(info);
    }

    // 读取配置文件
    let content = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("Failed to read config: {}", e))?;

    let config: serde_json::Value =
        serde_json::from_str(&content).map_err(|e| format!("Failed to parse config: {}", e))?;

    // MCP Servers
    info.mcp_servers = config
        .get("mcpServers")
        .or_else(|| config.get("mcp_servers"))
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    // Skills
    info.skills = read_local_skills(target)?;

    // Hooks
    if let Some(hooks) = config.get("hooks") {
        info.hooks = hooks.clone();
    }

    // Permissions
    if let Some(permissions) = config.get("permissions") {
        info.permissions = permissions.clone();
    }

    Ok(info)
}

#[tauri::command]
pub async fn list_profiles(state: State<'_, AppState>) -> Result<Vec<ApiProfile>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.list_profiles().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_profile(name: String, state: State<'_, AppState>) -> Result<ApiProfile, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.get_profile_by_name(&name).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn add_profile(profile: ApiProfile, state: State<'_, AppState>) -> Result<i64, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.add_profile(&profile).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_profile(profile: ApiProfile, state: State<'_, AppState>) -> Result<(), String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.update_profile(&profile).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_profile(name: String, state: State<'_, AppState>) -> Result<bool, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.delete_profile(&name).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn switch_profile(
    target_app: String,
    profile_name: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    use crate::adapters::get_adapter;

    let target = TargetApp::from_str(&target_app)
        .ok_or_else(|| format!("Unknown target app: {}", target_app))?;

    // 获取适配器
    let adapter = get_adapter(target);

    let db = state.db.lock().map_err(|e| e.to_string())?;

    // 获取 API Profile
    let api_profile = db
        .get_profile_by_name(&profile_name)
        .map_err(|e| e.to_string())?;

    // 切换前先提取当前文件中的共享配置，避免覆盖 permissions/hooks/MCP 等字段。
    let shared_config = if adapter.config_path().exists() {
        let current_config = adapter.read_config().map_err(|e| e.to_string())?;
        let extracted = adapter.extract_shared_config(&current_config);
        db.save_shared_config(target, extracted.clone())
            .map_err(|e| e.to_string())?;
        extracted
    } else {
        db.get_shared_config(target)
            .map_err(|e| e.to_string())?
            .map(|c| c.config)
            .unwrap_or_else(|| serde_json::json!({}))
    };

    // 备份当前配置
    if adapter.config_path().exists() {
        adapter.backup_config().map_err(|e| e.to_string())?;
    }

    // 合并配置
    let merged = adapter.merge_config(&api_profile, &shared_config);

    // 写入配置
    adapter.write_config(&merged).map_err(|e| e.to_string())?;

    // 应用工具特定的 API 凭据（如 Gemini 的 .env）
    adapter
        .apply_api_credentials(&api_profile)
        .map_err(|e| e.to_string())?;

    // 更新活动记录
    db.set_active_profile(target, api_profile.id.unwrap())
        .map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
pub async fn get_shared_config(
    target_app: String,
    state: State<'_, AppState>,
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
    state: State<'_, AppState>,
) -> Result<(), String> {
    let target = TargetApp::from_str(&target_app)
        .ok_or_else(|| format!("Unknown target app: {}", target_app))?;

    let db = state.db.lock().map_err(|e| e.to_string())?;

    db.save_shared_config(target, config)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn export_database(
    output_path: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let _db = state.db.lock().map_err(|e| e.to_string())?;
    let db_path = default_db_path()?;

    std::fs::copy(&db_path, &output_path)
        .map_err(|e| format!("Failed to export database: {}", e))?;

    Ok(())
}

#[tauri::command]
pub async fn import_database(input_path: String, state: State<'_, AppState>) -> Result<(), String> {
    let db_path = default_db_path()?;

    if !std::path::Path::new(&input_path).exists() {
        return Err(format!("Input file does not exist: {}", input_path));
    }

    let mut db = state.db.lock().map_err(|e| e.to_string())?;

    if db_path.exists() {
        let backup_path = db_path.with_extension("backup");
        std::fs::copy(&db_path, &backup_path)
            .map_err(|e| format!("Failed to backup database: {}", e))?;
    }

    std::fs::copy(&input_path, &db_path)
        .map_err(|e| format!("Failed to import database: {}", e))?;

    *db = Database::open(&db_path).map_err(|e| format!("Failed to reload database: {}", e))?;

    Ok(())
}

#[tauri::command]
pub async fn get_status(state: State<'_, AppState>) -> Result<StatusInfo, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;

    // Claude Code status
    let claude_code_profile = db
        .get_active_profile_full(TargetApp::ClaudeCode)
        .map_err(|e| e.to_string())?;
    let claude_code = Some(TargetStatus {
        profile: claude_code_profile,
        connected: true,
    });

    // Codex status
    let codex_profile = db
        .get_active_profile_full(TargetApp::Codex)
        .map_err(|e| e.to_string())?;
    let codex = Some(TargetStatus {
        profile: codex_profile,
        connected: false,
    });

    // Gemini status
    let gemini_profile = db
        .get_active_profile_full(TargetApp::Gemini)
        .map_err(|e| e.to_string())?;
    let gemini = Some(TargetStatus {
        profile: gemini_profile,
        connected: false,
    });

    // OpenCode status
    let opencode_profile = db
        .get_active_profile_full(TargetApp::OpenCode)
        .map_err(|e| e.to_string())?;
    let opencode = Some(TargetStatus {
        profile: opencode_profile,
        connected: false,
    });

    // Database info
    let profiles = db.list_profiles().map_err(|e| e.to_string())?;
    let db_path = dirs::home_dir()
        .unwrap()
        .join(".switch-api")
        .join("db.sqlite");
    let size = std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);

    Ok(StatusInfo {
        claude_code,
        codex,
        gemini,
        opencode,
        database: DatabaseInfo {
            size,
            profile_count: profiles.len(),
            path: db_path.to_string_lossy().to_string(),
        },
    })
}

fn read_local_skills(target: TargetApp) -> Result<Vec<String>, String> {
    let home = dirs::home_dir().ok_or("Failed to get home directory")?;
    let skills_dir = match target {
        TargetApp::ClaudeCode => home.join(".claude").join("skills"),
        TargetApp::Codex => home.join(".codex").join("skills"),
        TargetApp::Gemini => home.join(".gemini").join("skills"),
        TargetApp::OpenCode => home.join(".config").join("opencode").join("skills"),
    };

    if !skills_dir.exists() {
        return Ok(Vec::new());
    }

    let mut skills = Vec::new();
    for entry in std::fs::read_dir(&skills_dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.path().is_dir() {
            if let Some(name) = entry.file_name().to_str() {
                skills.push(name.to_string());
            }
        }
    }

    Ok(skills)
}

fn default_db_path() -> Result<std::path::PathBuf, String> {
    dirs::home_dir()
        .ok_or("Failed to get home directory".to_string())
        .map(|home| home.join(".switch-api").join("db.sqlite"))
}
