// Tauri commands
use crate::db::Database;
use crate::models::{ApiProfile, TargetApp};
use serde::{Deserialize, Serialize};
#[cfg(target_os = "macos")]
use std::io::Write;
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};
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

#[derive(Debug, Serialize)]
pub struct McpServerConfig {
    pub command: String,
    pub args: Vec<String>,
    pub url: Option<String>,
    pub env: Option<std::collections::HashMap<String, String>>,
}

// 反序列化中间体：command 可为字符串(Claude)或数组(OpenCode)，
// env(Claude) 或 environment(OpenCode) 都接受。其它字段忽略。
#[derive(Deserialize)]
struct RawMcpServer {
    #[serde(default)]
    command: Option<serde_json::Value>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    env: Option<std::collections::HashMap<String, String>>,
    #[serde(default)]
    environment: Option<std::collections::HashMap<String, String>>,
}

impl<'de> Deserialize<'de> for McpServerConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = RawMcpServer::deserialize(deserializer)?;

        // command 归一化：数组 → 第一个元素当 command，其余并入 args 前面；
        // 字符串 → 直接当 command；缺失 → 空。
        let (command, mut args) = match raw.command {
            Some(serde_json::Value::String(s)) => (s, Vec::new()),
            Some(serde_json::Value::Array(arr)) => {
                let mut parts: Vec<String> = arr
                    .into_iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect();
                if parts.is_empty() {
                    (String::new(), Vec::new())
                } else {
                    let cmd = parts.remove(0);
                    (cmd, parts)
                }
            }
            _ => (String::new(), Vec::new()),
        };
        // raw.args 接在数组拆出的 args 之后（Claude 情形 parts 为空，等于只有 raw.args）
        args.extend(raw.args);

        // env 优先 env(Claude)，否则 environment(OpenCode)
        let env = raw.env.or(raw.environment);

        Ok(McpServerConfig {
            command,
            args,
            url: raw.url,
            env,
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LocalConfigInfo {
    pub mcp_servers: std::collections::HashMap<String, McpServerConfig>,
    pub skills: Vec<String>,
    pub hooks: serde_json::Value,
    pub permissions: serde_json::Value,
}

#[tauri::command]
pub async fn copy_text(text: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        return copy_text_with_pbcopy(&text);
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = text;
        Err("Clipboard copy is only implemented for macOS".to_string())
    }
}

#[cfg(target_os = "macos")]
fn copy_text_with_pbcopy(text: &str) -> Result<(), String> {
    let mut child = Command::new("/usr/bin/pbcopy")
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to start pbcopy: {}", e))?;

    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| "Failed to open pbcopy stdin".to_string())?;
        stdin
            .write_all(text.as_bytes())
            .map_err(|e| format!("Failed to write clipboard text: {}", e))?;
    }

    let status = child
        .wait()
        .map_err(|e| format!("Failed to wait for pbcopy: {}", e))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("pbcopy exited with status {}", status))
    }
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
    // MCP 来源因工具而异（Claude 在 ~/.claude.json），交给适配器决定
    let mcp_servers = adapter
        .read_mcp_servers_raw()
        .map_err(|e| format!("Failed to read MCP servers: {}", e))?
        .and_then(|v| serde_json::from_value(v).ok())
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

    let mut info = LocalConfigInfo {
        mcp_servers: std::collections::HashMap::new(),
        skills: Vec::new(),
        hooks: serde_json::json!({}),
        permissions: serde_json::json!({}),
    };

    // 直接用适配器读取配置（read_config 自带 local→global 回退、TOML/JSON 分派）。
    // 不要先检查 config_path().exists()——Claude 的 local 文件可能不存在但 global 存在。
    let config = match adapter.read_config() {
        Ok(c) => c,
        Err(_) => return Ok(info), // 读取失败返回空，不报错
    };

    // MCP Servers —— 来源因工具而异（Claude 在 ~/.claude.json），交给适配器
    info.mcp_servers = adapter
        .read_mcp_servers_raw()
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    // Skills
    info.skills = read_local_skills(target)?;

    // Hooks（仅当存在且非空）
    if let Some(hooks) = config.get("hooks") {
        let empty = hooks.as_object().map(|o| o.is_empty()).unwrap_or(false);
        if !empty {
            info.hooks = hooks.clone();
        }
    }

    // Permissions（仅当存在且非空）
    if let Some(permissions) = config.get("permissions") {
        let empty = permissions.as_object().map(|o| o.is_empty()).unwrap_or(false);
        if !empty {
            info.permissions = permissions.clone();
        }
    }

    Ok(info)
}

#[tauri::command]
pub async fn list_profiles(state: State<'_, AppState>) -> Result<Vec<ApiProfile>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.list_profiles().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_profile(name: String, target_app: String, state: State<'_, AppState>) -> Result<ApiProfile, String> {
    let target = TargetApp::from_str(&target_app)
        .ok_or_else(|| format!("Unknown target app: {}", target_app))?;
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.get_profile_by_name_and_target(&name, target).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn add_profile(profile: ApiProfile, state: State<'_, AppState>) -> Result<i64, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.add_profile(&profile).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_profile(profile: ApiProfile, state: State<'_, AppState>) -> Result<(), String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.update_profile(&profile).map_err(|e| e.to_string())?;
    if let Some(id) = profile.id {
        for target in db
            .get_active_targets_for_profile(id)
            .map_err(|e| e.to_string())?
        {
            if let Some(api_profile) = db
                .get_active_profile_full(target)
                .map_err(|e| e.to_string())?
            {
                apply_profile_config(&db, target, &api_profile)?;
            }
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn delete_profile(name: String, target_app: String, state: State<'_, AppState>) -> Result<bool, String> {
    let target = TargetApp::from_str(&target_app)
        .ok_or_else(|| format!("Unknown target app: {}", target_app))?;
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.delete_profile(&name, target).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn switch_profile(
    target_app: String,
    profile_name: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let target = TargetApp::from_str(&target_app)
        .ok_or_else(|| format!("Unknown target app: {}", target_app))?;

    let db = state.db.lock().map_err(|e| e.to_string())?;

    // 获取 API Profile
    let api_profile = db
        .get_profile_by_name_and_target(&profile_name, target)
        .map_err(|e| e.to_string())?;

    apply_profile_config(&db, target, &api_profile)?;

    // 更新活动记录
    db.set_active_profile(target, api_profile.id.unwrap())
        .map_err(|e| e.to_string())?;

    Ok(())
}

pub(crate) fn apply_profile_config(
    db: &Database,
    target: TargetApp,
    api_profile: &ApiProfile,
) -> Result<(), String> {
    use crate::adapters::get_adapter;

    let adapter = get_adapter(target);
    let current_config = adapter.read_config().unwrap_or_else(|_| serde_json::json!({}));
    let shared_config = adapter.extract_shared_config(&current_config);
    let _ = db.save_shared_config(target, shared_config.clone());

    if adapter.config_path().exists() {
        adapter.backup_config().map_err(|e| e.to_string())?;
    }

    let merged = adapter.merge_config(api_profile, &shared_config);
    adapter.write_config(&merged).map_err(|e| e.to_string())?;
    adapter
        .apply_api_credentials(api_profile)
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

    // 先查数据库
    let from_db = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        db.get_shared_config(target)
            .map(|opt| opt.map(|sc| sc.config))
            .map_err(|e| e.to_string())?
    };

    // 数据库里有非空配置则返回；否则回退到读取实时配置文件的共享部分
    if let Some(cfg) = &from_db {
        let is_empty = cfg.as_object().map(|o| o.is_empty()).unwrap_or(false);
        if !is_empty {
            return Ok(from_db);
        }
    }

    // 回退：从实时配置文件提取共享配置（让用户能看到当前工具的真实配置）
    use crate::adapters::get_adapter;
    let adapter = get_adapter(target);
    if adapter.config_path().exists() {
        match adapter.read_config() {
            Ok(live) => {
                let shared = adapter.extract_shared_config(&live);
                return Ok(Some(shared));
            }
            Err(_) => return Ok(from_db),
        }
    }

    Ok(from_db)
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

/// 从本地配置文件扫描出的 API 凭据（用于导入为 Profile）
#[derive(Debug, Serialize, Deserialize)]
pub struct ScannedApi {
    pub found: bool,
    pub api_url: String,
    pub api_key: String,
    pub provider: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub context_1m: Option<bool>,
    /// 来源配置文件路径，便于用户确认
    pub source: String,
}

/// 读取某工具当前配置文件，提取其中的 API URL / Key（不写库，仅返回供预览）
#[tauri::command]
pub async fn scan_local_api(target_app: String) -> Result<ScannedApi, String> {
    use crate::adapters::get_adapter;
    let target = TargetApp::from_str(&target_app)
        .ok_or_else(|| format!("Unknown target app: {}", target_app))?;
    let adapter = get_adapter(target);
    let source = adapter.config_path().to_string_lossy().to_string();
    let cfg = adapter.read_config().map_err(|e| e.to_string())?;

    let (mut url, mut key, provider) = (String::new(), String::new(), default_provider(target));

    match target {
        TargetApp::ClaudeCode => {
            // Claude: API 在 env.ANTHROPIC_*。settings.local.json 优先，
            // 但若 local 没有 API 字段，回退到 settings.json
            if let Some(env) = cfg.get("env") {
                url = str_field(env, "ANTHROPIC_BASE_URL");
                key = str_field(env, "ANTHROPIC_AUTH_TOKEN");
            }
            if url.is_empty() || key.is_empty() {
                if let Some(home) = dirs::home_dir() {
                    let global = home.join(".claude").join("settings.json");
                    if let Ok(c) = std::fs::read_to_string(&global) {
                        if let Ok(j) = serde_json::from_str::<serde_json::Value>(&c) {
                            if let Some(env) = j.get("env") {
                                if url.is_empty() { url = str_field(env, "ANTHROPIC_BASE_URL"); }
                                if key.is_empty() { key = str_field(env, "ANTHROPIC_AUTH_TOKEN"); }
                            }
                        }
                    }
                }
            }
        }
        TargetApp::Codex => {
            // Codex: model_provider 字段指明当前 provider，base_url 在
            // [model_providers.<id>] 块；api key 不在 config.toml（走 auth.json /
            // OPENAI_API_KEY 环境变量），这里尽力读取。
            let pid = str_field(&cfg, "model_provider");
            if let Some(providers) = cfg.get("model_providers").and_then(|v| v.as_object()) {
                // 优先用 model_provider 指定的块，否则取第一个
                let block = providers
                    .get(&pid)
                    .or_else(|| providers.values().next());
                if let Some(b) = block {
                    url = str_field(b, "base_url");
                    // 某些配置把 key 写在 provider 块里
                    if key.is_empty() {
                        key = str_field(b, "api_key");
                    }
                    // env_key 指向环境变量名
                    let env_key = str_field(b, "env_key");
                    if key.is_empty() && !env_key.is_empty() {
                        key = std::env::var(&env_key).unwrap_or_default();
                    }
                }
            }
            // 顶层兜底
            if url.is_empty() {
                url = str_field(&cfg, "base_url");
            }
            // 从 auth.json 或常见环境变量读 key
            if key.is_empty() {
                if let Some(home) = dirs::home_dir() {
                    let auth = home.join(".codex").join("auth.json");
                    if let Ok(c) = std::fs::read_to_string(&auth) {
                        if let Ok(j) = serde_json::from_str::<serde_json::Value>(&c) {
                            key = str_field(&j, "OPENAI_API_KEY");
                            if key.is_empty() {
                                key = str_field(&j, "api_key");
                            }
                        }
                    }
                }
            }
            if key.is_empty() {
                key = std::env::var("OPENAI_API_KEY").unwrap_or_default();
            }
        }
        TargetApp::Gemini => {
            // Gemini: API 在 .env，adapter.read_config 返回 settings.json，
            // 这里直接读 ~/.gemini/.env
            if let Some(home) = dirs::home_dir() {
                let env_path = home.join(".gemini").join(".env");
                if let Ok(content) = std::fs::read_to_string(&env_path) {
                    for line in content.lines() {
                        if let Some((k, v)) = line.split_once('=') {
                            let v = v.trim().trim_matches('"').to_string();
                            match k.trim() {
                                "GEMINI_API_KEY" | "GOOGLE_API_KEY" => key = v,
                                "GOOGLE_GEMINI_BASE_URL" => url = v,
                                _ => {}
                            }
                        }
                    }
                }
            }
            if url.is_empty() {
                url = "https://generativelanguage.googleapis.com".to_string();
            }
        }
        TargetApp::OpenCode => {
            // OpenCode: provider.<id>.options.{apiKey,baseURL}，取第一个 provider
            let mut provider_id = String::new();
            if let Some(providers) = cfg.get("provider").and_then(|v| v.as_object()) {
                if let Some((pid, pv)) = providers.iter().next() {
                    provider_id = pid.clone();
                    if let Some(opts) = pv.get("options") {
                        url = str_field(opts, "baseURL");
                        key = str_field(opts, "apiKey");
                    }
                }
            }
            // key 缺失或是文件/环境引用占位（{file:...} / {env:...}）时，
            // 从 ~/.local/share/opencode/auth.json 读真实 key。
            if key.is_empty() || key.starts_with("{file:") || key.starts_with("{env:") {
                if let Some(home) = dirs::home_dir() {
                    let auth = home
                        .join(".local")
                        .join("share")
                        .join("opencode")
                        .join("auth.json");
                    if let Ok(c) = std::fs::read_to_string(&auth) {
                        if let Ok(j) = serde_json::from_str::<serde_json::Value>(&c) {
                            // 优先按 provider id 匹配，否则取第一个 api 类型条目
                            let entry = j
                                .get(&provider_id)
                                .or_else(|| j.as_object().and_then(|m| m.values().next()));
                            if let Some(e) = entry {
                                let k = str_field(e, "key");
                                if !k.is_empty() {
                                    key = k;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(ScannedApi {
        found: !url.is_empty() || !key.is_empty(),
        api_url: url,
        api_key: key,
        provider,
        model: codex_string_field(target, &cfg, "model"),
        reasoning_effort: codex_string_field(target, &cfg, "model_reasoning_effort"),
        context_1m: codex_context_1m(target, &cfg),
        source,
    })
}

/// 从某工具当前配置文件读取共享配置（permissions/hooks/MCP/skills 等），保存到数据库
#[tauri::command]
pub async fn import_shared_config(
    target_app: String,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    use crate::adapters::get_adapter;
    let target = TargetApp::from_str(&target_app)
        .ok_or_else(|| format!("Unknown target app: {}", target_app))?;
    let adapter = get_adapter(target);
    let cfg = adapter.read_config().map_err(|e| e.to_string())?;
    let shared = adapter.extract_shared_config(&cfg);

    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.save_shared_config(target, shared.clone())
        .map_err(|e| e.to_string())?;
    Ok(shared)
}

fn str_field(v: &serde_json::Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

fn codex_string_field(
    target: TargetApp,
    cfg: &serde_json::Value,
    key: &str,
) -> Option<String> {
    if target != TargetApp::Codex {
        return None;
    }

    let value = str_field(cfg, key);
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

fn codex_context_1m(target: TargetApp, cfg: &serde_json::Value) -> Option<bool> {
    if target != TargetApp::Codex {
        return None;
    }

    cfg.get("model_context_window")
        .and_then(|w| w.as_i64())
        .map(|w| w >= 1_000_000)
}

fn default_provider(target: TargetApp) -> String {
    match target {
        TargetApp::ClaudeCode => "anthropic",
        TargetApp::Codex => "openai",
        TargetApp::Gemini => "google",
        TargetApp::OpenCode => "anthropic",
    }
    .to_string()
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

    // 覆盖当前库之前，先验证输入文件是可打开的合法数据库，
    // 避免选错文件（损坏 / 非 SQLite）时把现有数据覆盖掉才发现。
    Database::open(&input_path)
        .map_err(|e| format!("Invalid database file: {}", e))?;

    let mut db = state.db.lock().map_err(|e| e.to_string())?;

    // 覆盖前自动备份当前库。备份文件名带时间戳，保留历史备份不互相覆盖，
    // 以便多次导入后仍能回退到任意一次导入前的状态。
    if db_path.exists() {
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let backup_path = db_path.with_file_name(format!("db.backup.{}.sqlite", timestamp));
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

/// 扫描给定目录列表里的 skill 子目录名，按出现顺序去重。
/// 目录不存在/读失败的跳过。纯函数（不依赖 HOME），便于测试。
fn scan_skill_dirs(dirs_to_scan: &[std::path::PathBuf]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut skills = Vec::new();
    for skills_dir in dirs_to_scan {
        if !skills_dir.exists() {
            continue;
        }
        let entries = match std::fs::read_dir(skills_dir) {
            Ok(e) => e,
            Err(_) => continue, // 单个目录读失败不影响其余
        };
        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if entry.path().is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    if seen.insert(name.to_string()) {
                        skills.push(name.to_string());
                    }
                }
            }
        }
    }
    skills
}

fn read_local_skills(target: TargetApp) -> Result<Vec<String>, String> {
    let home = dirs::home_dir().ok_or("Failed to get home directory")?;

    // 各工具的 skills 目录。OpenCode 官方会从多个 Claude/agent 兼容目录发现 skills，
    // 这里与之一致：~/.config/opencode/skills、~/.claude/skills、~/.agents/skills。
    let dirs_to_scan: Vec<std::path::PathBuf> = match target {
        TargetApp::ClaudeCode => vec![home.join(".claude").join("skills")],
        TargetApp::Codex => vec![home.join(".codex").join("skills")],
        TargetApp::Gemini => vec![home.join(".gemini").join("skills")],
        TargetApp::OpenCode => vec![
            home.join(".config").join("opencode").join("skills"),
            home.join(".claude").join("skills"),
            home.join(".agents").join("skills"),
        ],
    };

    Ok(scan_skill_dirs(&dirs_to_scan))
}

fn default_db_path() -> Result<std::path::PathBuf, String> {
    dirs::home_dir()
        .ok_or("Failed to get home directory".to_string())
        .map(|home| home.join(".switch-api").join("db.sqlite"))
}

// ============ 从 cc-switch 导入 ============

#[derive(Debug, Serialize, Deserialize)]
pub struct CcSwitchProvider {
    pub name: String,
    pub app_type: String,
    pub api_url: String,
    pub api_key: String,
    pub provider: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub context_1m: bool,
    pub is_current: bool,
}

/// 扫描 ~/.cc-switch/cc-switch.db，列出指定工具的 provider（供预览/选择导入）
#[tauri::command]
pub async fn scan_cc_switch(target_app: String) -> Result<Vec<CcSwitchProvider>, String> {
    let home = dirs::home_dir().ok_or("无法获取主目录")?;
    let db_path = home.join(".cc-switch").join("cc-switch.db");
    if !db_path.exists() {
        return Err(format!("未找到 cc-switch 数据库: {}", db_path.display()));
    }

    let conn = rusqlite::Connection::open(&db_path)
        .map_err(|e| format!("打开 cc-switch 数据库失败: {}", e))?;

    let mut stmt = conn
        .prepare("SELECT name, settings_config, is_current FROM providers WHERE app_type = ?1 ORDER BY sort_index")
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map([&target_app], |row| {
            let name: String = row.get(0)?;
            let settings: String = row.get(1)?;
            let is_current: i64 = row.get(2).unwrap_or(0);
            Ok((name, settings, is_current != 0))
        })
        .map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for r in rows {
        let (name, settings, is_current) = r.map_err(|e| e.to_string())?;
        let parsed = parse_cc_provider(&target_app, &settings);
        out.push(CcSwitchProvider {
            name,
            app_type: target_app.clone(),
            api_url: parsed.0,
            api_key: parsed.1,
            provider: parsed.2,
            model: parsed.3,
            reasoning_effort: parsed.4,
            context_1m: parsed.5,
            is_current,
        });
    }
    Ok(out)
}

/// 将选中的 cc-switch provider 导入为我们的 ApiProfile
#[tauri::command]
pub async fn import_cc_switch(
    target_app: String,
    providers: Vec<CcSwitchProvider>,
    state: State<'_, AppState>,
) -> Result<usize, String> {
    let target = TargetApp::from_str(&target_app)
        .ok_or_else(|| format!("Unknown target app: {}", target_app))?;
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let mut count = 0;
    for p in providers {
        // 名称冲突则加后缀
        let mut name = p.name.clone();
        if db.profile_name_exists(&name, target, None).unwrap_or(false) {
            name = format!("{}-{}", p.name, target.as_str());
        }
        let profile = ApiProfile {
            name,
            provider: p.provider,
            api_url: p.api_url,
            api_key: p.api_key,
            model: p.model,
            reasoning_effort: p.reasoning_effort,
            context_1m: Some(p.context_1m),
            target_app: Some(target),
            ..Default::default()
        };
        if db.add_profile(&profile).is_ok() {
            count += 1;
        }
    }
    Ok(count)
}

/// 解析 cc-switch 的 settings_config，返回 (url, key, provider, model, reasoning, context_1m)
fn parse_cc_provider(
    app_type: &str,
    settings: &str,
) -> (String, String, String, Option<String>, Option<String>, bool) {
    let v: serde_json::Value = serde_json::from_str(settings).unwrap_or(serde_json::json!({}));

    match app_type {
        "codex" => {
            let key = v
                .get("auth")
                .and_then(|a| a.get("OPENAI_API_KEY"))
                .and_then(|k| k.as_str())
                .unwrap_or("")
                .to_string();
            let config_str = v.get("config").and_then(|c| c.as_str()).unwrap_or("");
            // 解析 TOML
            let toml_v: toml::Value = toml::from_str(config_str).unwrap_or(toml::Value::Table(Default::default()));
            let cfg: serde_json::Value = serde_json::to_value(&toml_v).unwrap_or(serde_json::json!({}));

            let url = cfg
                .get("model_providers")
                .and_then(|p| p.get("custom"))
                .and_then(|c| c.get("base_url"))
                .and_then(|u| u.as_str())
                .unwrap_or("")
                .to_string();
            let model = cfg.get("model").and_then(|m| m.as_str()).map(String::from);
            let reasoning = cfg
                .get("model_reasoning_effort")
                .and_then(|r| r.as_str())
                .map(String::from);
            let ctx_1m = cfg
                .get("model_context_window")
                .and_then(|w| w.as_i64())
                .map(|w| w >= 1_000_000)
                .unwrap_or(false);
            (url, key, "openai".to_string(), model, reasoning, ctx_1m)
        }
        "claude" | "claude-code" => {
            let env = v.get("settingsConfig").and_then(|s| s.get("env")).or_else(|| v.get("env"));
            let url = env
                .and_then(|e| e.get("ANTHROPIC_BASE_URL"))
                .and_then(|u| u.as_str())
                .unwrap_or("")
                .to_string();
            let key = env
                .and_then(|e| e.get("ANTHROPIC_AUTH_TOKEN").or_else(|| e.get("ANTHROPIC_API_KEY")))
                .and_then(|k| k.as_str())
                .unwrap_or("")
                .to_string();
            (url, key, "anthropic".to_string(), None, None, false)
        }
        _ => (String::new(), String::new(), "custom".to_string(), None, None, false),
    }
}

#[cfg(test)]
mod mcp_config_tests {
    use super::McpServerConfig;

    #[test]
    fn test_opencode_array_command_normalized() {
        let v = serde_json::json!({
            "type": "local",
            "command": ["npx", "-y", "pkg"],
            "environment": { "K": "v" },
            "enabled": true
        });
        let cfg: McpServerConfig = serde_json::from_value(v).unwrap();
        assert_eq!(cfg.command, "npx");
        assert_eq!(cfg.args, vec!["-y", "pkg"]);
        assert_eq!(cfg.env.unwrap().get("K").unwrap(), "v");
    }

    #[test]
    fn test_claude_string_command_unchanged() {
        let v = serde_json::json!({
            "type": "stdio",
            "command": "npx",
            "args": ["-y", "pkg"],
            "env": { "K": "v" }
        });
        let cfg: McpServerConfig = serde_json::from_value(v).unwrap();
        assert_eq!(cfg.command, "npx");
        assert_eq!(cfg.args, vec!["-y", "pkg"]);
        assert_eq!(cfg.env.unwrap().get("K").unwrap(), "v");
    }

    #[test]
    fn test_remote_url_only() {
        let v = serde_json::json!({ "type": "remote", "url": "https://x/mcp", "enabled": true });
        let cfg: McpServerConfig = serde_json::from_value(v).unwrap();
        assert_eq!(cfg.command, "");
        assert!(cfg.args.is_empty());
        assert_eq!(cfg.url.unwrap(), "https://x/mcp");
    }

    #[test]
    fn test_map_of_opencode_servers_deserializes() {
        let v = serde_json::json!({
            "playwright": { "type": "local", "command": ["npx", "@playwright/mcp@latest"], "enabled": true },
            "github": { "type": "remote", "url": "https://api.githubcopilot.com/mcp/", "enabled": true }
        });
        let map: std::collections::HashMap<String, McpServerConfig> =
            serde_json::from_value(v).unwrap();
        assert_eq!(map.len(), 2);
        assert_eq!(map["playwright"].command, "npx");
        assert_eq!(map["playwright"].args, vec!["@playwright/mcp@latest"]);
        assert_eq!(map["github"].url.as_deref(), Some("https://api.githubcopilot.com/mcp/"));
    }
}

#[cfg(test)]
mod skills_tests {
    use super::scan_skill_dirs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static CTR: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn test_scan_skill_dirs_multi_dir_dedup() {
        // 造两个目录：dirA/{a,shared}、dirB/{b,shared}，外加一个不存在的目录
        let n = CTR.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("helio-skills-{}-{n}", std::process::id()));
        let dir_a = base.join("a-skills");
        let dir_b = base.join("b-skills");
        let missing = base.join("does-not-exist");
        std::fs::create_dir_all(dir_a.join("a")).unwrap();
        std::fs::create_dir_all(dir_a.join("shared")).unwrap();
        std::fs::create_dir_all(dir_b.join("b")).unwrap();
        std::fs::create_dir_all(dir_b.join("shared")).unwrap();

        let mut skills = scan_skill_dirs(&[dir_a.clone(), dir_b.clone(), missing]);
        skills.sort();
        // a、b、shared(去重为一个)；不存在的目录被跳过；不报错
        assert_eq!(skills, vec!["a".to_string(), "b".to_string(), "shared".to_string()]);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn test_scan_skill_dirs_all_missing_returns_empty() {
        let skills = scan_skill_dirs(&[std::path::PathBuf::from("/no/such/dir/xyz")]);
        assert!(skills.is_empty());
    }
}

#[cfg(test)]
#[cfg(target_os = "macos")]
mod clipboard_tests {
    use super::copy_text_with_pbcopy;

    #[test]
    fn test_copy_text_with_pbcopy_accepts_empty_text() {
        copy_text_with_pbcopy("").expect("empty clipboard text should copy");
    }
}
