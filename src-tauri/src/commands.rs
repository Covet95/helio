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
    /// 其余被同步但未单独归类的顶层配置(tui / plugins / features /
    /// skills_configuration / marketplaces / projects 等)。只读展示用,
    /// 让用户看到「切换时还带着同步了哪些东西」。
    pub other: serde_json::Value,
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
        other: serde_json::json!({}),
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

    // Hooks
    // Codex 的真正 hook 定义在 ~/.codex/hooks.json（{"hooks":{...}}），而 config.toml 里
    // 的 [hooks.state."..."] 只是 trusted_hash 校验记录，不能当 hook 展示。
    // 其他工具仍从 config 的 hooks 键读取。
    if target == TargetApp::Codex {
        let hooks_path = dirs::home_dir()
            .map(|h| h.join(".codex").join("hooks.json"));
        if let Some(path) = hooks_path {
            info.hooks = read_codex_hooks(&path);
        }
    } else if let Some(hooks) = config.get("hooks") {
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

    // 其余顶层键:展示「切换时还带着同步了哪些东西」。
    // 排除已单独归类的(mcp/hooks/permissions)和 API 凭证类(切换会改、不算共享)。
    if let Some(obj) = config.as_object() {
        const EXCLUDED: &[&str] = &[
            "mcp_servers", "mcpServers", "mcp",
            "hooks", "permissions",
            "model_provider", "model_providers", "api_key", "env",
        ];
        let mut other = serde_json::Map::new();
        for (k, v) in obj {
            if !EXCLUDED.contains(&k.as_str()) {
                other.insert(k.clone(), v.clone());
            }
        }
        if !other.is_empty() {
            info.other = serde_json::Value::Object(other);
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
    let mut shared_config = adapter.extract_shared_config(&current_config);

    // 防残缺护栏:外部工具(如 Codex 自升级)可能把 config 写残,导致 live 里
    // 顶层键大量消失。若直接用残缺的 live 覆盖 DB 快照并 merge,会把残缺状态固化
    // 并持续传播。这里用 DB 里更全的旧快照补回 live 缺失的顶层键(只补 live 没有的,
    // 不覆盖 live 已有的较新值),既止损又自动恢复。
    if let Ok(Some(prev)) = db.get_shared_config(target) {
        backfill_missing_top_level(&mut shared_config, &prev.config);
    }

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

/// 用 `prev`(DB 已存的较全快照)里的顶层键补回 `live` 缺失的键。
/// 只补 `live` 完全没有的顶层键;`live` 已有的键(哪怕值更旧)一律保留不动,
/// 避免覆盖用户在外部刚改的较新值。
fn backfill_missing_top_level(live: &mut serde_json::Value, prev: &serde_json::Value) {
    if let (Some(live_obj), Some(prev_obj)) = (live.as_object_mut(), prev.as_object()) {
        for (k, v) in prev_obj {
            if !live_obj.contains_key(k) {
                live_obj.insert(k.clone(), v.clone());
            }
        }
    }
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

/// 读取 Codex 的 config.toml 原始文本（不经 JSON 往返，保留用户格式/注释）。
/// 文件不存在时返回空字符串。仅 Codex 提供此能力。
#[tauri::command]
pub async fn read_codex_config_raw() -> Result<String, String> {
    use crate::adapters::get_adapter;
    let path = get_adapter(TargetApp::Codex).config_path();
    if !path.exists() {
        return Ok(String::new());
    }
    std::fs::read_to_string(&path).map_err(|e| format!("读取 config.toml 失败：{}", e))
}

/// 保存用户在 GUI 里手编的 Codex config.toml 原始文本。
/// 高风险写操作：必须「校验通过才写」+「写前备份」。
#[tauri::command]
pub async fn save_codex_config_raw(
    content: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    use crate::adapters::get_adapter;
    let adapter = get_adapter(TargetApp::Codex);
    let path = adapter.config_path();

    // 先校验语法：非法 TOML 直接返回，绝不触碰磁盘（不备份、不写入）。
    toml::from_str::<toml::Value>(&content)
        .map_err(|e| format!("TOML 语法错误，未保存：{}", e))?;

    // 校验通过后，写前备份当前配置（config.toml + auth.json）。
    if path.exists() {
        adapter
            .backup_config()
            .map_err(|e| format!("备份当前配置失败：{}", e))?;
    }

    // 原子写入原始文本（再次校验 + 临时文件 rename）。
    let parsed = validate_and_write_codex_config_raw(&content, &path)?;

    // 写盘成功后，把解析结果的共享部分同步进 DB，保持库与磁盘一致。
    // 这一步失败不影响整体结果——磁盘已写成功是主目标。
    let json = serde_json::to_value(&parsed).unwrap_or_else(|_| serde_json::json!({}));
    let shared = adapter.extract_shared_config(&json);
    if let Ok(db) = state.db.lock() {
        let _ = db.save_shared_config(TargetApp::Codex, shared);
    }

    Ok(())
}

/// 「校验 + 原子写入」核心逻辑，接受路径参数便于单测（不依赖真实 HOME）。
/// 先用 toml::from_str 校验，非法则返回 Err 且不写盘；合法则临时文件 + rename
/// 原子写入原始文本，返回解析出的 toml::Value。
fn validate_and_write_codex_config_raw(
    content: &str,
    path: &std::path::Path,
) -> Result<toml::Value, String> {
    let parsed = toml::from_str::<toml::Value>(content)
        .map_err(|e| format!("TOML 语法错误，未保存：{}", e))?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败：{}", e))?;
    }

    let temp_path = path.with_extension("toml.tmp");
    std::fs::write(&temp_path, content).map_err(|e| format!("写入临时文件失败：{}", e))?;
    if let Ok(file) = std::fs::File::open(&temp_path) {
        let _ = file.sync_all();
    }
    std::fs::rename(&temp_path, path).map_err(|e| format!("替换 config.toml 失败：{}", e))?;

    Ok(parsed)
}

/// 在完整 config（JSON）上对若干顶层字段做最小改动：
/// - value 非 null → set 该顶层键（覆盖旧值）
/// - value 为 null → remove 该顶层键
///
/// 其余字段一律不动。纯函数，便于单测。
fn apply_field_updates(config: &mut serde_json::Value, fields: &serde_json::Value) {
    let updates = match fields.as_object() {
        Some(m) => m,
        None => return,
    };
    let obj = match config.as_object_mut() {
        Some(o) => o,
        None => return,
    };
    for (key, value) in updates {
        if value.is_null() {
            obj.remove(key);
        } else {
            obj.insert(key.clone(), value.clone());
        }
    }
}

/// 编辑 Codex 全局行为字段（approval_policy / sandbox_mode 等顶层键）并写回
/// ~/.codex/config.toml。复用磁盘写路径：读 live config → 在完整配置上做最小
/// 改动 → 转回 TOML 文本 → 校验+备份+原子写 → 同步 DB。绝不因改一个字段丢失其他字段。
#[tauri::command]
pub async fn update_codex_fields(
    fields: serde_json::Value,
    state: State<'_, AppState>,
) -> Result<(), String> {
    use crate::adapters::get_adapter;
    let adapter = get_adapter(TargetApp::Codex);
    let path = adapter.config_path();

    // 读 live config（不存在则为空对象），在完整配置上做最小改动。
    let mut config = adapter
        .read_config()
        .map_err(|e| format!("读取 config.toml 失败：{}", e))?;
    apply_field_updates(&mut config, &fields);

    // JSON → TOML 文本。toml::Value::try_from 走 Serialize，自动处理表/值排序。
    let toml_value = toml::Value::try_from(&config)
        .map_err(|e| format!("转换为 TOML 失败：{}", e))?;
    let content = toml::to_string_pretty(&toml_value)
        .map_err(|e| format!("序列化 TOML 失败：{}", e))?;

    // 写前备份当前配置（config.toml + auth.json）。
    if path.exists() {
        adapter
            .backup_config()
            .map_err(|e| format!("备份当前配置失败：{}", e))?;
    }

    // 校验 + 原子写入（与 save_codex_config_raw 同一写路径）。
    let parsed = validate_and_write_codex_config_raw(&content, &path)?;

    // 同步 DB 的共享部分，保持库与磁盘一致。失败不影响主目标（磁盘已写成功）。
    let json = serde_json::to_value(&parsed).unwrap_or_else(|_| serde_json::json!({}));
    let shared = adapter.extract_shared_config(&json);
    if let Ok(db) = state.db.lock() {
        let _ = db.save_shared_config(TargetApp::Codex, shared);
    }

    Ok(())
}

/// 从本地配置文件扫描出的 API 凭据（用于导入为 Profile）
#[derive(Debug, Serialize, Deserialize)]
pub struct ScannedApi {
    pub found: bool,
    pub api_url: String,
    pub api_key: String,
    pub provider: String,
    pub model: Option<String>,
    /// Claude Code 专用：Sonnet/Opus/Fable/Haiku 角色映射（从 ANTHROPIC_DEFAULT_*_MODEL 反向重建）
    pub model_mapping: Option<std::collections::HashMap<String, String>>,
    pub reasoning_effort: Option<String>,
    pub context_1m: Option<bool>,
    pub wire_api: Option<String>,
    pub requires_openai_auth: Option<bool>,
    pub experimental_bearer_token: Option<String>,
    pub model_thinking_enabled: Option<bool>,
    pub service_tier: Option<String>,
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
    // Codex provider 块内的协议字段（仅 Codex 用到）
    let mut wire_api: Option<String> = None;
    let mut requires_openai_auth: Option<bool> = None;
    let mut experimental_bearer_token: Option<String> = None;
    // Claude Code 的默认模型 / 角色映射（仅 ClaudeCode 用到）
    let mut claude_model: Option<String> = None;
    let mut claude_mapping: Option<std::collections::HashMap<String, String>> = None;

    match target {
        TargetApp::ClaudeCode => {
            // Claude: API 在 env.ANTHROPIC_*，全局配置 = ~/.claude/settings.json
            // （adapter.read_config 已直接返回该文件内容）
            if let Some(env) = cfg.get("env") {
                url = str_field(env, "ANTHROPIC_BASE_URL");
                key = str_field(env, "ANTHROPIC_AUTH_TOKEN");
                claude_extract_models(env, &mut claude_model, &mut claude_mapping);
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
                    // 回带 provider 块内的协议字段，供导入还原
                    let w = str_field(b, "wire_api");
                    if !w.trim().is_empty() {
                        wire_api = Some(w);
                    }
                    requires_openai_auth = b.get("requires_openai_auth").and_then(|v| v.as_bool());
                    let bearer = str_field(b, "experimental_bearer_token");
                    if !bearer.trim().is_empty() {
                        experimental_bearer_token = Some(bearer);
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
        model: codex_string_field(target, &cfg, "model").or(claude_model),
        model_mapping: claude_mapping,
        reasoning_effort: codex_string_field(target, &cfg, "model_reasoning_effort"),
        context_1m: codex_context_1m(target, &cfg),
        wire_api,
        requires_openai_auth,
        experimental_bearer_token,
        model_thinking_enabled: codex_bool_field(target, &cfg, "model_thinking_enabled"),
        service_tier: codex_string_field(target, &cfg, "service_tier"),
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

fn codex_bool_field(target: TargetApp, cfg: &serde_json::Value, key: &str) -> Option<bool> {
    if target != TargetApp::Codex {
        return None;
    }
    cfg.get(key).and_then(|v| v.as_bool())
}

/// 从 Claude Code 的 env 对象反向提取默认模型与角色映射，与 `ClaudeCodeAdapter::merge_config`
/// 的写入格式对称：
/// - `ANTHROPIC_MODEL` → 默认模型
/// - `ANTHROPIC_DEFAULT_{ROLE}_MODEL`（可能带 `[1M]` 后缀）→ mapping 的 `{role}_model` / `{role}_one_m`
/// - `ANTHROPIC_DEFAULT_{ROLE}_MODEL_NAME` → mapping 的 `{role}_name`
///
/// 用 `&mut Option` 累加：已有值不覆盖，仅补空（便于多个 env 来源依次补齐）。
fn claude_extract_models(
    env: &serde_json::Value,
    model: &mut Option<String>,
    mapping: &mut Option<std::collections::HashMap<String, String>>,
) {
    if model.is_none() {
        let m = str_field(env, "ANTHROPIC_MODEL");
        if !m.trim().is_empty() {
            *model = Some(m);
        }
    }

    let mut found = std::collections::HashMap::new();
    for role in ["sonnet", "opus", "fable", "haiku"] {
        let upper = role.to_uppercase();
        let raw = str_field(env, &format!("ANTHROPIC_DEFAULT_{upper}_MODEL"));
        if raw.trim().is_empty() {
            continue;
        }
        // [1M] 后缀 = 1M 上下文标记，剥离后才是真实模型 id
        let (base, one_m) = match raw.strip_suffix("[1M]") {
            Some(b) => (b.to_string(), true),
            None => (raw.clone(), false),
        };
        found.insert(format!("{role}_model"), base);
        if one_m {
            found.insert(format!("{role}_one_m"), "true".to_string());
        }
        let name = str_field(env, &format!("ANTHROPIC_DEFAULT_{upper}_MODEL_NAME"));
        if !name.trim().is_empty() {
            found.insert(format!("{role}_name"), name);
        }
    }

    if found.is_empty() {
        return;
    }
    match mapping {
        // 已有更高优先级的映射，只补尚未出现的键
        Some(existing) => {
            for (k, v) in found {
                existing.entry(k).or_insert(v);
            }
        }
        None => *mapping = Some(found),
    }
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

/// 从 Codex 独立的 hooks.json 提取 `hooks` 字段用于展示。
/// 文件结构：{"hooks":{"PreToolUse":[...]}}。
/// 文件不存在或解析失败时返回空对象（不报错）。
fn read_codex_hooks(path: &std::path::Path) -> serde_json::Value {
    let empty = serde_json::json!({});
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return empty,
    };
    let parsed: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return empty,
    };
    match parsed.get("hooks") {
        Some(hooks) if hooks.is_object() && !hooks.as_object().unwrap().is_empty() => hooks.clone(),
        _ => empty,
    }
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
            let model = env
                .and_then(|e| e.get("ANTHROPIC_MODEL"))
                .and_then(|m| m.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(String::from);
            (url, key, "anthropic".to_string(), model, None, false)
        }
        _ => (String::new(), String::new(), "custom".to_string(), None, None, false),
    }
}

#[cfg(test)]
mod claude_extract_tests {
    use super::claude_extract_models;
    use serde_json::json;

    #[test]
    fn test_extracts_default_model() {
        let env = json!({ "ANTHROPIC_MODEL": "claude-opus-4" });
        let mut model = None;
        let mut mapping = None;
        claude_extract_models(&env, &mut model, &mut mapping);
        assert_eq!(model, Some("claude-opus-4".to_string()));
        assert!(mapping.is_none());
    }

    #[test]
    fn test_extracts_role_mapping() {
        let env = json!({
            "ANTHROPIC_DEFAULT_SONNET_MODEL": "gpt-5.5",
            "ANTHROPIC_DEFAULT_SONNET_MODEL_NAME": "Sonnet-Proxy",
            "ANTHROPIC_DEFAULT_OPUS_MODEL": "grok-4.3",
        });
        let mut model = None;
        let mut mapping = None;
        claude_extract_models(&env, &mut model, &mut mapping);
        let m = mapping.expect("mapping should exist");
        assert_eq!(m.get("sonnet_model"), Some(&"gpt-5.5".to_string()));
        assert_eq!(m.get("sonnet_name"), Some(&"Sonnet-Proxy".to_string()));
        assert_eq!(m.get("opus_model"), Some(&"grok-4.3".to_string()));
        assert!(m.get("haiku_model").is_none());
        assert!(m.get("opus_name").is_none());
    }

    #[test]
    fn test_strips_one_m_suffix() {
        let env = json!({ "ANTHROPIC_DEFAULT_OPUS_MODEL": "claude-opus-4[1M]" });
        let mut model = None;
        let mut mapping = None;
        claude_extract_models(&env, &mut model, &mut mapping);
        let m = mapping.unwrap();
        assert_eq!(m.get("opus_model"), Some(&"claude-opus-4".to_string()));
        assert_eq!(m.get("opus_one_m"), Some(&"true".to_string()));
    }

    #[test]
    fn test_no_one_m_flag_when_absent() {
        let env = json!({ "ANTHROPIC_DEFAULT_HAIKU_MODEL": "fast-model" });
        let mut model = None;
        let mut mapping = None;
        claude_extract_models(&env, &mut model, &mut mapping);
        let m = mapping.unwrap();
        assert!(m.get("haiku_one_m").is_none());
    }

    #[test]
    fn test_empty_env_yields_nothing() {
        let env = json!({});
        let mut model = None;
        let mut mapping = None;
        claude_extract_models(&env, &mut model, &mut mapping);
        assert!(model.is_none());
        assert!(mapping.is_none());
    }

    #[test]
    fn test_does_not_override_higher_priority() {
        let local = json!({
            "ANTHROPIC_MODEL": "local-model",
            "ANTHROPIC_DEFAULT_SONNET_MODEL": "local-sonnet",
        });
        let global = json!({
            "ANTHROPIC_MODEL": "global-model",
            "ANTHROPIC_DEFAULT_SONNET_MODEL": "global-sonnet",
            "ANTHROPIC_DEFAULT_OPUS_MODEL": "global-opus",
        });
        let mut model = None;
        let mut mapping = None;
        claude_extract_models(&local, &mut model, &mut mapping);
        claude_extract_models(&global, &mut model, &mut mapping);
        assert_eq!(model, Some("local-model".to_string()));
        let m = mapping.unwrap();
        assert_eq!(m.get("sonnet_model"), Some(&"local-sonnet".to_string()));
        assert_eq!(m.get("opus_model"), Some(&"global-opus".to_string()));
    }

    #[test]
    fn test_round_trip_with_merge_config() {
        // 关键回归：merge_config 写出的 env 必须能被 claude_extract_models 完整读回
        use crate::adapters::{claude_code::ClaudeCodeAdapter, ConfigAdapter};
        use crate::models::ApiProfile;
        use std::collections::HashMap;

        let mut mm = HashMap::new();
        mm.insert("sonnet_model".to_string(), "gpt-5.5".to_string());
        mm.insert("sonnet_name".to_string(), "Sonnet-Proxy".to_string());
        mm.insert("opus_model".to_string(), "claude-opus-4".to_string());
        mm.insert("opus_one_m".to_string(), "true".to_string());

        let profile = ApiProfile {
            name: "rt".to_string(),
            provider: "anthropic".to_string(),
            api_url: "https://x".to_string(),
            api_key: "sk-x".to_string(),
            model: Some("claude-sonnet-4".to_string()),
            model_mapping: Some(mm),
            ..Default::default()
        };

        let adapter = ClaudeCodeAdapter::new();
        let merged = adapter.merge_config(&profile, &json!({}));
        let env = merged.get("env").cloned().unwrap();

        let mut model = None;
        let mut mapping = None;
        claude_extract_models(&env, &mut model, &mut mapping);

        assert_eq!(model, Some("claude-sonnet-4".to_string()));
        let m = mapping.expect("round-trip mapping");
        assert_eq!(m.get("sonnet_model"), Some(&"gpt-5.5".to_string()));
        assert_eq!(m.get("sonnet_name"), Some(&"Sonnet-Proxy".to_string()));
        // [1M] 后缀被正确剥离 + 标记还原
        assert_eq!(m.get("opus_model"), Some(&"claude-opus-4".to_string()));
        assert_eq!(m.get("opus_one_m"), Some(&"true".to_string()));
        // 没设的 haiku 不应出现
        assert!(m.get("haiku_model").is_none());
    }
}

#[cfg(test)]
mod codex_hooks_tests {
    use super::read_codex_hooks;
    use std::sync::atomic::{AtomicU64, Ordering};

    static CTR: AtomicU64 = AtomicU64::new(0);

    fn temp_path(name: &str) -> std::path::PathBuf {
        let n = CTR.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "switch-api-codex-hooks-{}-{n}-{name}",
            std::process::id()
        ))
    }

    #[test]
    fn test_reads_hooks_field_from_hooks_json() {
        let path = temp_path("hooks.json");
        std::fs::write(
            &path,
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"echo hi"}]}]}}"#,
        )
        .unwrap();

        let hooks = read_codex_hooks(&path);
        assert_eq!(hooks["PreToolUse"][0]["matcher"], "Bash");
        assert_eq!(
            hooks["PreToolUse"][0]["hooks"][0]["command"],
            "echo hi"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_missing_file_returns_empty_object() {
        let hooks = read_codex_hooks(&std::path::PathBuf::from("/no/such/codex/hooks.json"));
        assert!(hooks.as_object().unwrap().is_empty());
    }

    #[test]
    fn test_invalid_json_returns_empty_object() {
        let path = temp_path("bad.json");
        std::fs::write(&path, "{not valid json").unwrap();
        let hooks = read_codex_hooks(&path);
        assert!(hooks.as_object().unwrap().is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_does_not_use_config_toml_hooks_state() {
        // hooks.json 无 hooks 字段（只有 config.toml 风格的 state hash）→ 返回空
        let path = temp_path("state-only.json");
        std::fs::write(
            &path,
            r#"{"state":{"some.hooks.json:pre_tool_use:0:0":{"trusted_hash":"abc"}}}"#,
        )
        .unwrap();
        let hooks = read_codex_hooks(&path);
        assert!(hooks.as_object().unwrap().is_empty());
        let _ = std::fs::remove_file(&path);
    }
}

#[cfg(test)]
mod backfill_tests {
    use super::backfill_missing_top_level;

    #[test]
    fn test_backfill_restores_missing_keys_without_overwriting() {
        // live 被外部写残(只剩 model_provider + 较新的 base_url),
        // prev(DB 旧快照)更全。补回缺失键,但不动 live 已有的较新值。
        let mut live = serde_json::json!({
            "model_provider": "openai-custom",
            "model_providers": { "openai-custom": { "base_url": "https://new.api.com/v1" } },
        });
        let prev = serde_json::json!({
            "model_provider": "openai-custom",
            "model_providers": { "openai-custom": { "base_url": "https://OLD.api.com/v1" } },
            "plugins": { "browser": { "enabled": true } },
            "skills_configuration": { "enabled_skills": ["brainstorming"] },
            "tui": { "status_line": ["model", "git-branch"] },
        });

        backfill_missing_top_level(&mut live, &prev);

        // 缺失的顶层键被补回
        assert!(live.get("plugins").is_some());
        assert!(live.get("skills_configuration").is_some());
        assert_eq!(live["tui"]["status_line"].as_array().unwrap().len(), 2);
        // live 已有的较新值不被旧快照覆盖
        assert_eq!(
            live["model_providers"]["openai-custom"]["base_url"],
            "https://new.api.com/v1"
        );
    }

    #[test]
    fn test_backfill_noop_when_live_complete() {
        let mut live = serde_json::json!({ "a": 1, "b": 2 });
        let prev = serde_json::json!({ "a": 9 });
        backfill_missing_top_level(&mut live, &prev);
        // live 已有 a,不被覆盖;没有新增键
        assert_eq!(live["a"], 1);
        assert_eq!(live.as_object().unwrap().len(), 2);
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

#[cfg(test)]
mod codex_raw_config_tests {
    use super::validate_and_write_codex_config_raw;
    use std::sync::atomic::{AtomicU64, Ordering};

    static CTR: AtomicU64 = AtomicU64::new(0);

    fn temp_path(name: &str) -> std::path::PathBuf {
        let n = CTR.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "switch-api-codex-raw-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn test_bad_toml_returns_err_and_does_not_write() {
        let path = temp_path("config.toml");
        // 预置一个已存在的合法文件，验证坏 TOML 不会覆盖它
        std::fs::write(&path, "model_provider = \"openai\"\n").unwrap();

        let result = validate_and_write_codex_config_raw("this is = = not valid", &path);
        assert!(result.is_err());
        // 原文件未被改动
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert_eq!(on_disk, "model_provider = \"openai\"\n");
        // 不留临时文件
        assert!(!path.with_extension("toml.tmp").exists());

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn test_good_toml_writes_raw_content_verbatim() {
        let path = temp_path("config.toml");
        // 带注释和格式，验证原始文本被逐字写入（不经序列化往返）
        let content = "# my codex config\nmodel_provider = \"openai-custom\"\n\n[model_providers.openai-custom]\nbase_url = \"https://api.example.com/v1\"\n";

        let parsed = validate_and_write_codex_config_raw(content, &path).unwrap();
        // 返回解析结果可用
        assert_eq!(parsed["model_provider"].as_str(), Some("openai-custom"));
        // 磁盘内容与输入逐字一致（注释保留）
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert_eq!(on_disk, content);
        // 不留临时文件
        assert!(!path.with_extension("toml.tmp").exists());

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}

#[cfg(test)]
mod codex_field_update_tests {
    use super::apply_field_updates;
    use serde_json::json;

    #[test]
    fn test_set_new_field() {
        let mut config = json!({ "model_provider": "openai" });
        apply_field_updates(&mut config, &json!({ "approval_policy": "never" }));
        assert_eq!(config["approval_policy"], "never");
        // 原有字段不受影响
        assert_eq!(config["model_provider"], "openai");
    }

    #[test]
    fn test_override_existing_field() {
        let mut config = json!({ "sandbox_mode": "read-only" });
        apply_field_updates(&mut config, &json!({ "sandbox_mode": "workspace-write" }));
        assert_eq!(config["sandbox_mode"], "workspace-write");
    }

    #[test]
    fn test_null_removes_field() {
        let mut config = json!({ "service_tier": "fast", "model_provider": "openai" });
        apply_field_updates(&mut config, &json!({ "service_tier": null }));
        assert!(config.get("service_tier").is_none());
        // 其他字段保留
        assert_eq!(config["model_provider"], "openai");
    }

    #[test]
    fn test_does_not_touch_other_fields() {
        let mut config = json!({
            "model_provider": "openai",
            "model_providers": { "openai": { "base_url": "https://api.com" } },
            "mcp_servers": { "fs": { "command": "npx" } },
            "approval_policy": "on-request",
        });
        apply_field_updates(
            &mut config,
            &json!({
                "approval_policy": "untrusted",
                "model_auto_compact_token_limit": 200000,
                "disable_response_storage": true,
            }),
        );
        // 改了/加了指定字段
        assert_eq!(config["approval_policy"], "untrusted");
        assert_eq!(config["model_auto_compact_token_limit"], 200000);
        assert_eq!(config["disable_response_storage"], true);
        // 完整保留嵌套结构
        assert_eq!(
            config["model_providers"]["openai"]["base_url"],
            "https://api.com"
        );
        assert_eq!(config["mcp_servers"]["fs"]["command"], "npx");
        assert_eq!(config["model_provider"], "openai");
    }

    #[test]
    fn test_mixed_set_and_remove() {
        let mut config = json!({
            "personality": "friendly",
            "enable_workflows": true,
        });
        apply_field_updates(
            &mut config,
            &json!({
                "personality": null,
                "model_reasoning_effort": "high",
                "enable_workflows": false,
            }),
        );
        assert!(config.get("personality").is_none());
        assert_eq!(config["model_reasoning_effort"], "high");
        assert_eq!(config["enable_workflows"], false);
    }
}
