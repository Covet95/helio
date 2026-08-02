// Tauri commands
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use switch_api::db::Database;
use switch_api::models::{ApiProfile, TargetApp};
use tauri::State;

use crate::commands::helpers::{
    claude_extract_models, codex_bool_field, codex_context_1m, codex_string_field,
    default_provider, str_field,
};

pub struct AppState {
    pub db: Mutex<Database>,
    /// 全局配置写锁：所有磁盘写路径（切换/更新/failover 重切/托盘切换/raw 编辑）
    /// 互斥执行，避免并发写同一配置文件导致 DB 与磁盘状态分裂。
    /// 临界区只有同步 IO（不跨 await），std::sync::Mutex 足够。
    pub config_lock: Mutex<()>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StatusInfo {
    pub claude_code: Option<TargetStatus>,
    pub codex: Option<TargetStatus>,
    pub pi: Option<TargetStatus>,
    pub opencode: Option<TargetStatus>,
    pub hermes: Option<TargetStatus>,
    pub openclaw: Option<TargetStatus>,
    pub database: DatabaseInfo,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct TargetStatus {
    pub profile: Option<ApiProfile>,
    /// 有活跃 profile 即为 true（配置态）；批量探活成功时也会为 true
    pub connected: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe_ok: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_probed_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe_protocol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
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
    copy_text_native(&text)
}

/// 跨平台剪贴板写入：macOS pbcopy / Windows PowerShell Set-Clipboard / Linux wl-copy|xclip。
fn copy_text_native(text: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        copy_text_with_pbcopy(text)
    }

    #[cfg(target_os = "windows")]
    {
        copy_text_with_powershell(text)
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        copy_text_with_linux_clipboard(text)
    }

    #[cfg(not(any(
        target_os = "macos",
        target_os = "windows",
        all(unix, not(target_os = "macos"))
    )))]
    {
        let _ = text;
        Err("Clipboard copy is not implemented on this platform".to_string())
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

/// Windows：经 PowerShell `Set-Clipboard` 写入；stdin 用 UTF-8 文本，避免参数转义问题。
/// 空串在 Win 上 Set-Clipboard 会抛 ArgumentNullException，视为成功 no-op。
#[cfg(target_os = "windows")]
fn copy_text_with_powershell(text: &str) -> Result<(), String> {
    if text.is_empty() {
        return Ok(());
    }

    // 读 UTF-8 标准输入再 Set-Clipboard，兼容多行/特殊字符，且不把正文塞进命令行参数。
    let mut child = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[Console]::InputEncoding = [System.Text.Encoding]::UTF8; $t = [Console]::In.ReadToEnd(); if ($null -eq $t) { $t = [string]::Empty }; Set-Clipboard -Value $t",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to start PowerShell for clipboard: {}", e))?;

    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| "Failed to open PowerShell stdin".to_string())?;
        stdin
            .write_all(text.as_bytes())
            .map_err(|e| format!("Failed to write clipboard text: {}", e))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("Failed to wait for PowerShell clipboard: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "Set-Clipboard failed (status {}): {}",
            output.status,
            err.trim()
        ))
    }
}

/// Linux：优先 Wayland `wl-copy`，否则 X11 `xclip`。
#[cfg(all(unix, not(target_os = "macos")))]
fn copy_text_with_linux_clipboard(text: &str) -> Result<(), String> {
    let mut last_err = String::from("no clipboard backend found (tried wl-copy, xclip)");

    for (bin, args) in [
        ("wl-copy", vec![] as Vec<&str>),
        ("xclip", vec!["-selection", "clipboard"]),
    ] {
        match Command::new(bin)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(mut child) => {
                if let Some(stdin) = child.stdin.as_mut() {
                    if let Err(e) = stdin.write_all(text.as_bytes()) {
                        last_err = format!("{bin}: write stdin failed: {e}");
                        continue;
                    }
                } else {
                    last_err = format!("{bin}: no stdin");
                    continue;
                }
                match child.wait_with_output() {
                    Ok(out) if out.status.success() => return Ok(()),
                    Ok(out) => {
                        last_err = format!(
                            "{bin} exited {}: {}",
                            out.status,
                            String::from_utf8_lossy(&out.stderr).trim()
                        );
                    }
                    Err(e) => last_err = format!("{bin}: wait failed: {e}"),
                }
            }
            Err(e) => last_err = format!("{bin}: {e}"),
        }
    }

    Err(format!("Clipboard copy failed: {last_err}"))
}

// 新增：扫描本地 MCP 配置
#[tauri::command]
pub async fn scan_local_mcp_servers(
    target_app: String,
) -> Result<std::collections::HashMap<String, McpServerConfig>, String> {
    let target = TargetApp::parse(&target_app)
        .ok_or_else(|| format!("Unknown target app: {}", target_app))?;

    use switch_api::adapters::get_adapter;
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
    let target = TargetApp::parse(&target_app)
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
    let target = TargetApp::parse(&target_app)
        .ok_or_else(|| format!("Unknown target app: {}", target_app))?;

    use switch_api::adapters::get_adapter;
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
        let hooks_path = dirs::home_dir().map(|h| h.join(".codex").join("hooks.json"));
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
        let empty = permissions
            .as_object()
            .map(|o| o.is_empty())
            .unwrap_or(false);
        if !empty {
            info.permissions = permissions.clone();
        }
    }

    // 其余顶层键:展示「切换时还带着同步了哪些东西」。
    // 排除已单独归类的(mcp/hooks/permissions)和 API 凭证类(切换会改、不算共享)。
    if let Some(obj) = config.as_object() {
        const EXCLUDED: &[&str] = &[
            "mcp_servers",
            "mcpServers",
            "mcp",
            "hooks",
            "permissions",
            "model_provider",
            "model_providers",
            "api_key",
            "env",
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
pub async fn get_profile(
    name: String,
    target_app: String,
    state: State<'_, AppState>,
) -> Result<ApiProfile, String> {
    let target = TargetApp::parse(&target_app)
        .ok_or_else(|| format!("Unknown target app: {}", target_app))?;
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.get_profile_by_name_and_target(&name, target)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn add_profile(profile: ApiProfile, state: State<'_, AppState>) -> Result<i64, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.add_profile(&profile).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_profile(profile: ApiProfile, state: State<'_, AppState>) -> Result<(), String> {
    let active_profiles = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        db.update_profile(&profile).map_err(|e| e.to_string())?;
        let mut active_profiles = Vec::new();
        if let Some(id) = profile.id {
            for target in db
                .get_active_targets_for_profile(id)
                .map_err(|e| e.to_string())?
            {
                if let Some(api_profile) = db
                    .get_active_profile_full(target)
                    .map_err(|e| e.to_string())?
                {
                    let persisted_shared_config = db
                        .get_shared_config(target)
                        .map_err(|e| e.to_string())?
                        .map(|config| config.config);
                    active_profiles.push((target, api_profile, persisted_shared_config));
                }
            }
        }
        active_profiles
    };

    // 全局写锁：与其他切换/写盘命令互斥，避免并发写配置。
    // 走 apply_profile_switch（含 journal）：写盘失败时立即回滚，并保持 active 语义一致。
    let _write_guard = state.config_lock.lock().map_err(|e| e.to_string())?;
    for (target, api_profile, persisted_shared_config) in active_profiles {
        let shared_config =
            switch_api::adapters::resolve_shared_config(target, persisted_shared_config)
                .map_err(|e| e.to_string())?;
        let db = state.db.lock().map_err(|e| e.to_string())?;
        switch_api::adapters::apply_profile_switch(&db, target, &api_profile, &shared_config, true)
            .map_err(|e| format!("更新后同步配置失败 ({target}): {e}"))?;
    }
    Ok(())
}

#[tauri::command]
pub async fn delete_profile(
    name: String,
    target_app: String,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    let target = TargetApp::parse(&target_app)
        .ok_or_else(|| format!("Unknown target app: {}", target_app))?;
    let db = state.db.lock().map_err(|e| e.to_string())?;
    if target == TargetApp::OpenCode {
        return switch_api::adapters::opencode::OpenCodeAdapter::delete_profile_and_cleanup_local(
            &db, &name,
        )
        .map_err(|e| format!("删除 OpenCode 档案失败: {e}"));
    }
    db.delete_profile(&name, target).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn switch_profile(
    target_app: String,
    profile_name: String,
    probe: Option<bool>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let target = TargetApp::parse(&target_app)
        .ok_or_else(|| format!("Unknown target app: {}", target_app))?;
    let do_probe = probe.unwrap_or(false);

    if do_probe {
        let result = run_failover(&state, target, &profile_name, false).await?;
        if !result.success {
            return Err(format!(
                "探活 failover 失败，未写入配置: {}",
                result
                    .tried
                    .iter()
                    .filter_map(|t| t.error.as_ref())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
    }

    let (mut api_profile, persisted_shared_config) = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        let api_profile = db
            .get_profile_by_name_and_target(&profile_name, target)
            .map_err(|e| e.to_string())?;
        let persisted_shared_config = db
            .get_shared_config(target)
            .map_err(|e| e.to_string())?
            .map(|config| config.config);
        (api_profile, persisted_shared_config)
    };
    api_profile.normalize_keys();
    // 全局写锁：与其他切换/写盘命令互斥
    let _write_guard = state.config_lock.lock().map_err(|e| e.to_string())?;
    let shared_config =
        switch_api::adapters::resolve_shared_config(target, persisted_shared_config)
            .map_err(|e| e.to_string())?;
    let db = state.db.lock().map_err(|e| e.to_string())?;
    switch_api::adapters::apply_profile_switch(&db, target, &api_profile, &shared_config, true)
        .map_err(|e| format!("切换失败: {e}"))?;

    Ok(())
}

#[tauri::command]
pub async fn get_shared_config(
    target_app: String,
    state: State<'_, AppState>,
) -> Result<Option<serde_json::Value>, String> {
    let target = TargetApp::parse(&target_app)
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
    use switch_api::adapters::get_adapter;
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
    let target = TargetApp::parse(&target_app)
        .ok_or_else(|| format!("Unknown target app: {}", target_app))?;

    // 与切换/导入互斥，避免写 shared_config 时被 replace live 库打断。
    let _write_guard = state.config_lock.lock().map_err(|e| e.to_string())?;
    let db = state.db.lock().map_err(|e| e.to_string())?;

    db.save_shared_config(target, config)
        .map_err(|e| e.to_string())
}

#[derive(Debug, Serialize)]
pub struct ConfigBackupInfo {
    pub path: String,
    /// 备份时间，格式化为本地时间字符串（解析文件名内嵌时间戳，失败退回 mtime）
    pub time: String,
    /// 恢复时将写回的目标配置文件；文件名格式异常时为 None（不可恢复）
    pub target: Option<String>,
}

/// 列出 target_app 的配置备份（新→旧）。
#[tauri::command]
pub async fn list_config_backups(target_app: String) -> Result<Vec<ConfigBackupInfo>, String> {
    use switch_api::adapters::{backup, get_adapter};
    let target = TargetApp::parse(&target_app)
        .ok_or_else(|| format!("Unknown target app: {}", target_app))?;
    let config_dir = get_adapter(target).config_path();
    let config_dir = config_dir
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let backups = backup::list_backups(&config_dir).map_err(|e| e.to_string())?;
    Ok(backups
        .into_iter()
        .map(|b| ConfigBackupInfo {
            time: chrono::DateTime::<chrono::Local>::from(b.time)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string(),
            path: b.path.display().to_string(),
            target: b.target.map(|t| t.display().to_string()),
        })
        .collect())
}

/// 恢复配置备份（写盘操作，走全局配置写锁；恢复前自动备份当前配置）。
/// 返回恢复写回的配置文件路径。
#[tauri::command]
pub async fn restore_config_backup(
    target_app: String,
    backup_file: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    use switch_api::adapters::{backup, get_adapter};
    let target = TargetApp::parse(&target_app)
        .ok_or_else(|| format!("Unknown target app: {}", target_app))?;
    let config_dir = get_adapter(target).config_path();
    let config_dir = config_dir
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let _guard = state
        .config_lock
        .lock()
        .map_err(|e| format!("获取配置写锁失败：{e}"))?;
    let restored = backup::restore_backup(&config_dir, std::path::Path::new(&backup_file))
        .map_err(|e| format!("恢复失败：{e:#}"))?;
    Ok(restored.display().to_string())
}

/// 读取 Codex 的 config.toml 原始文本（不经 JSON 往返，保留用户格式/注释）。
/// 文件不存在时返回空字符串。仅 Codex 提供此能力。
#[tauri::command]
pub async fn read_codex_config_raw() -> Result<String, String> {
    use switch_api::adapters::get_adapter;
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
    use switch_api::adapters::get_adapter;
    let adapter = get_adapter(TargetApp::Codex);
    let path = adapter.config_path();

    // 先校验语法：非法 TOML 直接返回，绝不触碰磁盘（不备份、不写入）。
    toml::from_str::<toml::Value>(&content).map_err(|e| format!("TOML 语法错误，未保存：{}", e))?;

    // 全局写锁：与切换等写盘命令互斥
    let _write_guard = state.config_lock.lock().map_err(|e| e.to_string())?;

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

    switch_api::utils::secure_fs::atomic_write_private(path, content.as_bytes())
        .map_err(|e| format!("替换 config.toml 失败：{}", e))?;

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
    use switch_api::adapters::get_adapter;
    let adapter = get_adapter(TargetApp::Codex);
    let path = adapter.config_path();

    // 读 live config（不存在则为空对象），在完整配置上做最小改动。
    let mut config = adapter
        .read_config()
        .map_err(|e| format!("读取 config.toml 失败：{}", e))?;
    apply_field_updates(&mut config, &fields);

    // JSON → TOML 文本。toml::Value::try_from 走 Serialize，自动处理表/值排序。
    let toml_value =
        toml::Value::try_from(&config).map_err(|e| format!("转换为 TOML 失败：{}", e))?;
    let content =
        toml::to_string_pretty(&toml_value).map_err(|e| format!("序列化 TOML 失败：{}", e))?;

    // 全局写锁：与切换等写盘命令互斥
    let _write_guard = state.config_lock.lock().map_err(|e| e.to_string())?;

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
    pub env_key: Option<String>,
    pub requires_openai_auth: Option<bool>,
    pub experimental_bearer_token: Option<String>,
    pub model_thinking_enabled: Option<bool>,
    pub service_tier: Option<String>,
    /// Hermes / OpenClaw 协议模式（独立字段，不再借用 wire_api）
    pub api_mode: Option<String>,
    /// OpenClaw models[].maxTokens
    pub max_tokens: Option<i64>,
    /// 来源配置文件路径，便于用户确认
    pub source: String,
}

/// 读取某工具当前配置文件，提取其中的 API URL / Key（不写库，仅返回供预览）
#[tauri::command]
pub async fn scan_local_api(target_app: String) -> Result<ScannedApi, String> {
    use switch_api::adapters::get_adapter;
    let target = TargetApp::parse(&target_app)
        .ok_or_else(|| format!("Unknown target app: {}", target_app))?;
    let adapter = get_adapter(target);
    let source = adapter.config_path().to_string_lossy().to_string();
    let cfg = adapter.read_config().map_err(|e| e.to_string())?;

    let (mut url, mut key, provider) = (String::new(), String::new(), default_provider(target));
    // Codex provider 块内的协议字段（仅 Codex 用到）
    let mut wire_api: Option<String> = None;
    let mut codex_env_key: Option<String> = None;
    let mut requires_openai_auth: Option<bool> = None;
    let mut experimental_bearer_token: Option<String> = None;
    // Claude Code 的默认模型 / 角色映射（仅 ClaudeCode 用到）
    let mut claude_model: Option<String> = None;
    let mut claude_mapping: Option<std::collections::HashMap<String, String>> = None;
    // Hermes / OpenClaw
    let mut api_mode: Option<String> = None;
    let mut max_tokens: Option<i64> = None;
    let mut context_1m: Option<bool> = None;

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
                let block = providers.get(&pid).or_else(|| providers.values().next());
                if let Some(b) = block {
                    url = str_field(b, "base_url");
                    // 某些配置把 key 写在 provider 块里
                    if key.is_empty() {
                        key = str_field(b, "api_key");
                    }
                    // env_key 指向环境变量名
                    let env_key = str_field(b, "env_key");
                    if !env_key.trim().is_empty() {
                        codex_env_key = Some(env_key.clone());
                    }
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
        TargetApp::Pi => {
            // Pi: defaultProvider/defaultModel in settings.json; key in auth.json;
            // custom baseUrl in models.json.providers.<id>
            let default_provider = str_field(&cfg, "defaultProvider");
            let default_model = str_field(&cfg, "defaultModel");
            if !default_model.is_empty() {
                claude_model = Some(default_model);
            }
            let mut provider_id = default_provider;
            if let Some(home) = dirs::home_dir() {
                let agent = home.join(".pi").join("agent");
                let models_path = agent.join("models.json");
                if let Ok(c) = std::fs::read_to_string(&models_path) {
                    if let Ok(j) = serde_json::from_str::<serde_json::Value>(&c) {
                        if let Some(providers) = j.get("providers").and_then(|v| v.as_object()) {
                            let block = if !provider_id.is_empty() {
                                providers.get(&provider_id)
                            } else {
                                None
                            }
                            .or_else(|| providers.values().next());
                            if let Some(b) = block {
                                url = str_field(b, "baseUrl");
                                if key.is_empty() {
                                    key = str_field(b, "apiKey");
                                }
                                if provider_id.is_empty() {
                                    if let Some((pid, _)) = providers.iter().next() {
                                        provider_id = pid.clone();
                                    }
                                }
                            }
                        }
                    }
                }
                let auth_path = agent.join("auth.json");
                if let Ok(c) = std::fs::read_to_string(&auth_path) {
                    if let Ok(j) = serde_json::from_str::<serde_json::Value>(&c) {
                        let entry = if !provider_id.is_empty() {
                            j.get(&provider_id)
                        } else {
                            None
                        }
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
            if !provider_id.is_empty() {
                // reuse api_mode slot as temporary carrier; cleared when building response
                api_mode = Some(format!("__pi_provider__:{provider_id}"));
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
        TargetApp::Hermes => {
            // model.provider = custom:<name> + custom_providers[].{base_url,api_key,api_mode}
            let model_obj = cfg.get("model");
            let provider_slug = model_obj
                .and_then(|m| m.get("provider"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let name = provider_slug
                .strip_prefix("custom:")
                .unwrap_or(provider_slug)
                .to_lowercase();
            if let Some(arr) = cfg.get("custom_providers").and_then(|v| v.as_array()) {
                for entry in arr {
                    let ename = entry
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_lowercase()
                        .replace(' ', "-");
                    if ename == name {
                        url = str_field(entry, "base_url");
                        key = str_field(entry, "api_key");
                        let mode = str_field(entry, "api_mode");
                        if !mode.is_empty() {
                            api_mode = Some(mode);
                        }
                        break;
                    }
                }
            }
            if let Some(m) = model_obj {
                claude_model = m
                    .get("default")
                    .or_else(|| m.get("model"))
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string());
                if api_mode.is_none() {
                    let mode = str_field(m, "api_mode");
                    if !mode.is_empty() {
                        api_mode = Some(mode);
                    }
                }
                if let Some(ctx) = m.get("context_length").and_then(|v| v.as_i64()) {
                    context_1m = Some(ctx >= 1_000_000);
                }
            }
        }
        TargetApp::OpenClaw => {
            // agents.defaults.model.primary = "provider/model"
            // models.providers.<id>.{baseUrl,apiKey,api,models[]}
            let primary = cfg
                .pointer("/agents/defaults/model/primary")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let (pid, mid) = if let Some((p, m)) = primary.split_once('/') {
                (p.to_string(), m.to_string())
            } else {
                (String::new(), String::new())
            };
            if let Some(providers) = cfg.pointer("/models/providers").and_then(|v| v.as_object()) {
                let block = if !pid.is_empty() {
                    providers.get(&pid)
                } else {
                    providers.values().next()
                };
                if let Some(b) = block {
                    url = str_field(b, "baseUrl");
                    if url.is_empty() {
                        url = str_field(b, "base_url");
                    }
                    key = str_field(b, "apiKey");
                    if key.is_empty() {
                        key = str_field(b, "api_key");
                    }
                    let mode = str_field(b, "api");
                    if !mode.is_empty() {
                        // normalize OpenClaw api string toward Helio form
                        api_mode = Some(match mode.as_str() {
                            "openai-completions" => "chat_completions".into(),
                            "anthropic-messages" => "anthropic_messages".into(),
                            "openai-responses" => "codex_responses".into(),
                            other => other.to_string(),
                        });
                    }
                    if !mid.is_empty() {
                        if let Some(models) = b.get("models").and_then(|v| v.as_array()) {
                            if let Some(m) = models.iter().find(|m| {
                                m.get("id").and_then(|v| v.as_str()) == Some(mid.as_str())
                            }) {
                                if let Some(cw) = m.get("contextWindow").and_then(|v| v.as_i64()) {
                                    context_1m = Some(cw >= 1_000_000);
                                }
                                if let Some(mt) = m.get("maxTokens").and_then(|v| v.as_i64()) {
                                    if mt > 0 {
                                        max_tokens = Some(mt);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if !mid.is_empty() {
                claude_model = Some(mid);
            }
            // agents.defaults.contextTokens as fallback for 1M detection
            if context_1m.is_none() {
                if let Some(ct) = cfg
                    .pointer("/agents/defaults/contextTokens")
                    .and_then(|v| v.as_i64())
                {
                    context_1m = Some(ct >= 1_000_000);
                }
            }
        }
    }

    // Hermes 把 provider 名从 model.provider 还原（去 custom:）
    // OpenClaw 从 agents.defaults.model.primary 的 provider/ 前缀还原
    let mut provider = provider;
    if target == TargetApp::Hermes {
        let slug = cfg
            .get("model")
            .and_then(|m| m.get("provider"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let name = slug.strip_prefix("custom:").unwrap_or(slug);
        if !name.is_empty() {
            provider = name.to_string();
        }
    }
    if target == TargetApp::OpenClaw {
        let primary = cfg
            .pointer("/agents/defaults/model/primary")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if let Some((p, _)) = primary.split_once('/') {
            if !p.is_empty() {
                provider = p.to_string();
            }
        }
    }
    if target == TargetApp::Pi {
        if let Some(marker) = api_mode.clone() {
            if let Some(pid) = marker.strip_prefix("__pi_provider__:") {
                if !pid.is_empty() {
                    provider = pid.to_string();
                }
            }
        }
        api_mode = None;
    }

    // Codex/Claude keep their own context_1m path; Hermes/OpenClaw use local scan.
    let resolved_context_1m = match target {
        TargetApp::Hermes | TargetApp::OpenClaw => context_1m,
        _ => codex_context_1m(target, &cfg),
    };

    Ok(ScannedApi {
        found: !url.is_empty() || !key.is_empty(),
        api_url: url,
        api_key: key,
        provider,
        model: if target == TargetApp::Hermes
            || target == TargetApp::OpenClaw
            || target == TargetApp::Pi
        {
            claude_model
        } else {
            codex_string_field(target, &cfg, "model").or(claude_model)
        },
        model_mapping: claude_mapping,
        reasoning_effort: codex_string_field(target, &cfg, "model_reasoning_effort"),
        context_1m: resolved_context_1m,
        wire_api,
        env_key: codex_env_key,
        requires_openai_auth,
        experimental_bearer_token,
        model_thinking_enabled: codex_bool_field(target, &cfg, "model_thinking_enabled"),
        service_tier: codex_string_field(target, &cfg, "service_tier"),
        api_mode,
        max_tokens,
        source,
    })
}

/// 从某工具当前配置文件读取共享配置（permissions/hooks/MCP/skills 等），保存到数据库
#[tauri::command]
pub async fn import_shared_config(
    target_app: String,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    use switch_api::adapters::get_adapter;
    let target = TargetApp::parse(&target_app)
        .ok_or_else(|| format!("Unknown target app: {}", target_app))?;
    let adapter = get_adapter(target);
    let cfg = adapter.read_config().map_err(|e| e.to_string())?;
    let shared = adapter.extract_shared_config(&cfg);

    let _write_guard = state.config_lock.lock().map_err(|e| e.to_string())?;
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.save_shared_config(target, shared.clone())
        .map_err(|e| e.to_string())?;
    Ok(shared)
}

#[tauri::command]
pub async fn export_database(
    output_path: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let _db = state.db.lock().map_err(|e| e.to_string())?;
    let db_path = default_db_path()?;

    // 快照而非文件拷贝：拷主文件会漏掉还在 -wal 里的已提交数据（实测可导出成空档案库）。
    // 导出目标由用户选择，snapshot_to 只收紧文件本身权限，不动其所在目录。
    Database::snapshot_to(&db_path, std::path::Path::new(&output_path))
        .map_err(|e| format!("Failed to export database: {}", e))?;

    Ok(())
}

/// 把全部 skills 目录打包为 tar.gz（manifest + {app}/{skill}/...）。
/// 与数据库备份正交：skills 是文件系统资产，不入库。
#[tauri::command]
pub async fn export_skills(
    output_path: String,
    state: State<'_, AppState>,
) -> Result<switch_api::utils::skills_backup::SkillsExportResult, String> {
    // 与配置写路径互斥：导出期间避免并发切换改到半截 skill 目录。
    let _write_guard = state.config_lock.lock().map_err(|e| e.to_string())?;
    let home = dirs::home_dir().ok_or("Failed to get home directory")?;
    switch_api::utils::skills_backup::export_skills(&home, std::path::Path::new(&output_path))
        .map_err(|e| format!("Failed to export skills: {e}"))
}

/// 从 tar.gz 归档恢复 skills。整体校验不通过则拒绝且不写盘；
/// 同名 skill 目录已存在时跳过（不覆盖）。
#[tauri::command]
pub async fn import_skills(
    input_path: String,
    state: State<'_, AppState>,
) -> Result<switch_api::utils::skills_backup::SkillsImportResult, String> {
    // 与切换/写配置互斥；skills_backup 内部另有进程级 IMPORT_LOCK 防重入。
    let _write_guard = state.config_lock.lock().map_err(|e| e.to_string())?;
    let home = dirs::home_dir().ok_or("Failed to get home directory")?;
    switch_api::utils::skills_backup::import_skills(&home, std::path::Path::new(&input_path))
        .map_err(|e| format!("Failed to import skills: {e}"))
}

#[tauri::command]
pub async fn import_database(input_path: String, state: State<'_, AppState>) -> Result<(), String> {
    let db_path = default_db_path()?;

    // 与切换/写配置互斥：导入会替换 live 库文件，期间绝不能有其他命令仍持有旧连接写盘。
    let _write_guard = state.config_lock.lock().map_err(|e| e.to_string())?;
    let mut db = state.db.lock().map_err(|e| e.to_string())?;
    let placeholder = Database::open(":memory:").map_err(|e| e.to_string())?;
    let previous = std::mem::replace(&mut *db, placeholder);
    drop(previous);

    let backup = match Database::replace_file_from_import(
        std::path::Path::new(&input_path),
        &db_path,
    ) {
        Ok(backup) => backup,
        Err(error) => {
            *db = Database::open(&db_path)
                .map_err(|restore| format!("Failed to stage and import database: {error}; failed to reopen current database: {restore}"))?;
            return Err(format!("Failed to stage and import database: {error}"));
        }
    };
    match Database::open(&db_path) {
        Ok(reloaded) => {
            *db = reloaded;
            // 导入可能带来/抹掉 switch journal 旁车；启动式恢复一次，保持 active/配置一致。
            if let Err(error) = switch_api::adapters::journal::recover_interrupted_switch(&db) {
                eprintln!("[Helio] recover after import failed: {error:#}");
            }
        }
        Err(error) => {
            if let Some(backup_path) = backup.as_ref() {
                Database::restore_replaced_file(&db_path, backup_path).map_err(|restore| {
                    format!(
                        "Failed to reload imported database: {error}; rollback failed: {restore}"
                    )
                })?;
                *db = Database::open(&db_path)
                    .map_err(|restore| format!("Failed to reload imported database: {error}; failed to reopen restored database: {restore}"))?;
            }
            return Err(format!("Failed to reload imported database: {error}"));
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn get_status(state: State<'_, AppState>) -> Result<StatusInfo, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;

    // Claude Code status
    let claude_code_profile = db
        .get_active_profile_full(TargetApp::ClaudeCode)
        .map_err(|e| e.to_string())?;
    // connected = 已配置活跃 profile（不发外网探活）
    let claude_code = Some(TargetStatus {
        connected: claude_code_profile.is_some(),
        profile: claude_code_profile,
        ..Default::default()
    });

    // Codex status
    let codex_profile = db
        .get_active_profile_full(TargetApp::Codex)
        .map_err(|e| e.to_string())?;
    let codex = Some(TargetStatus {
        connected: codex_profile.is_some(),
        profile: codex_profile,
        ..Default::default()
    });

    // Pi status
    let pi_profile = db
        .get_active_profile_full(TargetApp::Pi)
        .map_err(|e| e.to_string())?;
    let pi = Some(TargetStatus {
        connected: pi_profile.is_some(),
        profile: pi_profile,
        ..Default::default()
    });

    // OpenCode status
    let opencode_profile = db
        .get_active_profile_full(TargetApp::OpenCode)
        .map_err(|e| e.to_string())?;
    let opencode = Some(TargetStatus {
        connected: opencode_profile.is_some(),
        profile: opencode_profile,
        ..Default::default()
    });

    // Hermes status
    let hermes_profile = db
        .get_active_profile_full(TargetApp::Hermes)
        .map_err(|e| e.to_string())?;
    let hermes = Some(TargetStatus {
        connected: hermes_profile.is_some(),
        profile: hermes_profile,
        ..Default::default()
    });

    // OpenClaw status
    let openclaw_profile = db
        .get_active_profile_full(TargetApp::OpenClaw)
        .map_err(|e| e.to_string())?;
    let openclaw = Some(TargetStatus {
        connected: openclaw_profile.is_some(),
        profile: openclaw_profile,
        ..Default::default()
    });

    // Database info
    let profiles = db.list_profiles().map_err(|e| e.to_string())?;
    let db_path = dirs::home_dir()
        .ok_or_else(|| "Failed to get home directory".to_string())?
        .join(".switch-api")
        .join("db.sqlite");
    let size = std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);

    Ok(StatusInfo {
        claude_code,
        codex,
        pi,
        opencode,
        hermes,
        openclaw,
        database: DatabaseInfo {
            size,
            profile_count: profiles.len(),
            path: db_path.to_string_lossy().to_string(),
        },
    })
}

fn profile_protocol_fields(profile: &ApiProfile) -> Option<String> {
    match profile.target_app {
        Some(TargetApp::Hermes) => profile.hermes.api_mode.clone(),
        Some(TargetApp::OpenClaw) => profile.openclaw.api_mode.clone(),
        _ => profile
            .hermes
            .api_mode
            .clone()
            .or_else(|| profile.openclaw.api_mode.clone()),
    }
}

fn model_for_probe(profile: &ApiProfile) -> String {
    profile
        .model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| {
            profile
                .opencode
                .models
                .as_ref()
                .and_then(|m| m.iter().map(|s| s.trim()).find(|s| !s.is_empty()))
                .map(|s| s.to_string())
        })
        .unwrap_or_default()
}

/// Helio 侧 failover：按 active 优先顺序探活，成功则设活跃并可选 re-switch。
async fn run_failover(
    state: &State<'_, AppState>,
    target: TargetApp,
    profile_name: &str,
    re_switch: bool,
) -> Result<crate::model_fetch::FailoverResult, String> {
    use crate::model_fetch::{probe_with_params, FailoverResult, KeyProbeResult};

    let (mut profile, was_active) = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        let mut p = db
            .get_profile_by_name_and_target(profile_name, target)
            .map_err(|e| e.to_string())?;
        p.normalize_keys();
        let active_id = db
            .get_active_profile(target)
            .map_err(|e| e.to_string())?
            .map(|a| a.profile_id);
        let was = p.id.zip(active_id).map(|(a, b)| a == b).unwrap_or(false);
        (p, was)
    };

    let model = model_for_probe(&profile);
    if model.is_empty() {
        return Err("先为该 Profile 填写默认模型再 failover".into());
    }
    let mode = profile_protocol_fields(&profile);
    let app_str = target.as_str().to_string();

    let mut keys = profile.api_keys.clone().unwrap_or_default();
    if keys.is_empty() && !profile.api_key.trim().is_empty() {
        profile.normalize_keys();
        keys = profile.api_keys.clone().unwrap_or_default();
    }
    if keys.is_empty() {
        return Err("没有可 failover 的 Key".into());
    }

    keys.sort_by_key(|e| if e.is_active { 0 } else { 1 });

    let now = chrono::Utc::now().timestamp();
    let probe_timeout = std::time::Duration::from_secs(20);
    // 并发探测所有 key（串行 N×15s 太慢）；结果按 keys 原顺序收集，
    // 仍按「active 优先、列表顺序优先」选择第一个成功者。
    let probes: Vec<_> = keys
        .iter()
        .map(|entry| {
            let app_str = app_str.clone();
            let api_url = profile.api_url.clone();
            let model = model.clone();
            let mode = mode.clone();
            async move {
                (
                    entry.id.clone(),
                    entry.label.clone(),
                    tokio::time::timeout(
                        probe_timeout,
                        probe_with_params(switch_api::probe::ProbeRequest {
                            target_app: &app_str,
                            api_url: &api_url,
                            api_key: &entry.key,
                            model: &model,
                            wire_api: None,
                            api_mode: mode.as_deref(),
                            experimental_bearer_token: None,
                            key_label: Some(entry.label.clone()),
                        }),
                    )
                    .await,
                )
            }
        })
        .collect::<Vec<_>>();
    // tokio 无 futures 时需 join_all；这里用 futures crate
    let probe_results: Vec<(String, String, Result<Result<_, String>, _>)> =
        futures::future::join_all(probes).await;

    let mut tried: Vec<KeyProbeResult> = Vec::new();
    let mut winner: Option<(String, String)> = None;
    for (entry, (id, label, res)) in keys.iter().zip(probe_results) {
        match res {
            Ok(Ok(ok)) => {
                tried.push(KeyProbeResult {
                    key_id: id.clone(),
                    label: label.clone(),
                    ok: true,
                    error: None,
                    endpoint: Some(ok.endpoint),
                    protocol: Some(ok.protocol),
                });
                if let Some(list) = profile.api_keys.as_mut() {
                    for e in list.iter_mut() {
                        if e.id == entry.id {
                            e.last_probe_ok = Some(true);
                            e.last_probed_at = Some(now);
                        }
                    }
                }
                winner = Some((id.clone(), label.clone()));
                break;
            }
            Ok(Err(err)) => {
                tried.push(KeyProbeResult {
                    key_id: id.clone(),
                    label: label.clone(),
                    ok: false,
                    error: Some(err),
                    endpoint: None,
                    protocol: None,
                });
                if let Some(list) = profile.api_keys.as_mut() {
                    for e in list.iter_mut() {
                        if e.id == entry.id {
                            e.last_probe_ok = Some(false);
                            e.last_probed_at = Some(now);
                        }
                    }
                }
            }
            Err(_elapsed) => {
                tried.push(KeyProbeResult {
                    key_id: id.clone(),
                    label: label.clone(),
                    ok: false,
                    error: Some(format!(
                        "{} 探活超时（{}s）",
                        label,
                        probe_timeout.as_secs()
                    )),
                    endpoint: None,
                    protocol: None,
                });
                if let Some(list) = profile.api_keys.as_mut() {
                    for e in list.iter_mut() {
                        if e.id == entry.id {
                            e.last_probe_ok = Some(false);
                            e.last_probed_at = Some(now);
                        }
                    }
                }
            }
        }
    }

    let success = winner.is_some();
    if let Some((id, _)) = &winner {
        let _ = profile.set_active_key_id(id);
    }

    let persisted_shared_config = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        db.update_profile(&profile).map_err(|e| e.to_string())?;
        let should_switch = re_switch || (was_active && success);
        if should_switch && success {
            Some(
                db.get_shared_config(target)
                    .map_err(|e| e.to_string())?
                    .map(|config| config.config),
            )
        } else {
            None
        }
    };

    let mut re_switched = false;
    if let Some(persisted_shared_config) = persisted_shared_config {
        // 全局写锁：与其他切换/写盘命令互斥
        let _write_guard = state.config_lock.lock().map_err(|e| e.to_string())?;
        let shared_config =
            switch_api::adapters::resolve_shared_config(target, persisted_shared_config)
                .map_err(|e| e.to_string())?;
        let db = state.db.lock().map_err(|e| e.to_string())?;
        switch_api::adapters::apply_profile_switch(&db, target, &profile, &shared_config, true)
            .map_err(|e| format!("切换失败: {e}"))?;
        re_switched = true;
    }

    Ok(FailoverResult {
        success,
        active_key_id: winner.as_ref().map(|(id, _)| id.clone()),
        active_label: winner.map(|(_, l)| l),
        tried,
        re_switched,
    })
}

#[tauri::command]
pub async fn failover_profile_keys(
    target_app: String,
    profile_name: String,
    re_switch: Option<bool>,
    state: State<'_, AppState>,
) -> Result<crate::model_fetch::FailoverResult, String> {
    let target = TargetApp::parse(&target_app)
        .ok_or_else(|| format!("Unknown target app: {}", target_app))?;
    // re_switch=Some(true) 强制 re-switch；None/false 时由 run_failover 在「已是 active profile」时自动 re-switch
    let force = re_switch == Some(true);
    run_failover(&state, target, &profile_name, force).await
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolProbeResult {
    pub target_app: String,
    pub configured: bool,
    /// 与 CC Switch stream_check 一致：任意 HTTP 响应 = 可达
    pub ok: bool,
    /// operational | degraded | failed（对齐 CC Switch HealthStatus）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// 保留字段；可达性探测不验协议，恒为 "reachability"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    pub probed_at: i64,
}

/// 状态页「检测可用性」：对齐 CC Switch stream_check。
/// 对每个已配置工具的 `api_url` 做 GET 可达性探测（不发模型请求、不验 key）。
#[tauri::command]
pub async fn probe_active_profiles(
    state: State<'_, AppState>,
) -> Result<Vec<ToolProbeResult>, String> {
    use crate::model_fetch::{probe_reachability, ReachabilityConfig};

    let snapshots: Vec<(TargetApp, Option<ApiProfile>)> = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        let tools = [
            TargetApp::ClaudeCode,
            TargetApp::Codex,
            TargetApp::Pi,
            TargetApp::OpenCode,
            TargetApp::Hermes,
            TargetApp::OpenClaw,
        ];
        let mut out = Vec::new();
        for t in tools {
            let p = db.get_active_profile_full(t).map_err(|e| e.to_string())?;
            out.push((t, p));
        }
        out
    };

    let cfg = ReachabilityConfig::default();
    // 并发探测 6 个工具（每个可能超时 8s+），串行最坏 ~48s → 并行一次超时
    let futures: Vec<_> = snapshots
        .into_iter()
        .map(|(target, profile)| {
            let app = target.as_str().to_string();
            let cfg = cfg.clone();
            async move {
                let Some(profile) = profile else {
                    return ToolProbeResult {
                        target_app: app,
                        configured: false,
                        ok: false,
                        status: None,
                        profile_name: None,
                        error: None,
                        protocol: None,
                        endpoint: None,
                        latency_ms: None,
                        http_status: None,
                        probed_at: chrono::Utc::now().timestamp(),
                    };
                };
                let url = profile.api_url.trim();
                if url.is_empty() {
                    return ToolProbeResult {
                        target_app: app,
                        configured: true,
                        ok: false,
                        status: Some("failed".into()),
                        profile_name: Some(profile.name),
                        error: Some("API URL 为空".into()),
                        protocol: Some("reachability".into()),
                        endpoint: None,
                        latency_ms: None,
                        http_status: None,
                        probed_at: chrono::Utc::now().timestamp(),
                    };
                }
                let r = probe_reachability(url, &cfg).await;
                let status_str = match r.status {
                    switch_api::probe::ReachabilityStatus::Operational => "operational",
                    switch_api::probe::ReachabilityStatus::Degraded => "degraded",
                    switch_api::probe::ReachabilityStatus::Failed => "failed",
                };
                ToolProbeResult {
                    target_app: app,
                    configured: true,
                    ok: r.success,
                    status: Some(status_str.into()),
                    profile_name: Some(profile.name),
                    error: if r.success { None } else { Some(r.message) },
                    protocol: Some("reachability".into()),
                    endpoint: Some(r.endpoint),
                    latency_ms: r.response_time_ms,
                    http_status: r.http_status,
                    probed_at: r.tested_at,
                }
            }
        })
        .collect();
    Ok(futures::future::join_all(futures).await)
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
        TargetApp::Pi => vec![home.join(".pi").join("agent").join("skills")],
        TargetApp::OpenCode => vec![
            home.join(".config").join("opencode").join("skills"),
            home.join(".claude").join("skills"),
            home.join(".agents").join("skills"),
        ],
        TargetApp::Hermes => vec![home.join(".hermes").join("skills")],
        TargetApp::OpenClaw => vec![
            home.join(".openclaw").join("skills"),
            home.join(".openclaw").join("workspace").join("skills"),
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
        Some(hooks) if hooks.as_object().is_some_and(|o| !o.is_empty()) => hooks.clone(),
        _ => empty,
    }
}

fn default_db_path() -> Result<std::path::PathBuf, String> {
    dirs::home_dir()
        .ok_or("Failed to get home directory".to_string())
        .map(|home| home.join(".switch-api").join("db.sqlite"))
}

#[cfg(test)]
mod claude_extract_tests {
    use crate::commands::helpers::claude_extract_models;
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
        assert!(!m.contains_key("haiku_model"));
        assert!(!m.contains_key("opus_name"));
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
        assert!(!m.contains_key("haiku_one_m"));
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
        use std::collections::HashMap;
        use switch_api::adapters::{claude_code::ClaudeCodeAdapter, ConfigAdapter};
        use switch_api::models::ApiProfile;

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
            claude: switch_api::models::ClaudeProfileFields {
                model_mapping: Some(mm),
            },
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
        assert!(!m.contains_key("haiku_model"));
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
        assert_eq!(hooks["PreToolUse"][0]["hooks"][0]["command"], "echo hi");

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
    use switch_api::adapters::backfill_missing_top_level;

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
        assert_eq!(
            map["github"].url.as_deref(),
            Some("https://api.githubcopilot.com/mcp/")
        );
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
        assert_eq!(
            skills,
            vec!["a".to_string(), "b".to_string(), "shared".to_string()]
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn test_scan_skill_dirs_all_missing_returns_empty() {
        let skills = scan_skill_dirs(&[std::path::PathBuf::from("/no/such/dir/xyz")]);
        assert!(skills.is_empty());
    }
}

#[cfg(test)]
mod clipboard_tests {
    use super::copy_text_native;

    #[test]
    fn test_copy_text_native_accepts_empty_text() {
        // 空串在各平台后端都应可接受（不崩、不拒）。
        copy_text_native("").expect("empty clipboard text should copy");
    }

    #[test]
    fn test_copy_text_native_accepts_unicode() {
        copy_text_native("Helio 剪贴板 ✓")
            .expect("unicode clipboard text should copy on this platform");
    }
}

#[cfg(test)]
mod codex_raw_config_tests {
    use super::validate_and_write_codex_config_raw;
    use std::sync::atomic::{AtomicU64, Ordering};

    static CTR: AtomicU64 = AtomicU64::new(0);

    fn temp_path(name: &str) -> std::path::PathBuf {
        let n = CTR.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("switch-api-codex-raw-{}-{n}", std::process::id()));
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
