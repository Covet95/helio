// Tauri commands
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use switch_api::db::Database;
use switch_api::models::{ApiProfile, TargetApp};
use tauri::State;

use crate::commands::status::run_failover;
use crate::commands::{unknown_target_app, AppError};

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
    pub zcode: Option<TargetStatus>,
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

// 新增：获取完整的本地配置信息
#[tauri::command]
pub async fn get_local_config_info(target_app: String) -> Result<LocalConfigInfo, AppError> {
    let target = TargetApp::parse(&target_app).ok_or_else(|| unknown_target_app(&target_app))?;

    use switch_api::adapters::get_adapter;
    let adapter = get_adapter(target)?;

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
    info.skills = read_local_skills(target).map_err(AppError::io)?;

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
pub async fn list_profiles(state: State<'_, AppState>) -> Result<Vec<ApiProfile>, AppError> {
    let db = state.db.lock()?;
    Ok(db.list_profiles()?)
}

#[tauri::command]
pub async fn assign_legacy_profile(
    profile_id: i64,
    target_app: String,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    let target = TargetApp::parse(&target_app).ok_or_else(|| unknown_target_app(&target_app))?;
    let _write_guard = state.config_lock.lock()?;
    let db = state.db.lock()?;
    db.assign_legacy_profile(profile_id, target)
}

#[tauri::command]
pub async fn delete_legacy_profile(
    profile_id: i64,
    state: State<'_, AppState>,
) -> Result<bool, AppError> {
    let _write_guard = state.config_lock.lock()?;
    let db = state.db.lock()?;
    db.delete_legacy_profile(profile_id)
}

#[tauri::command]
pub async fn add_profile(profile: ApiProfile, state: State<'_, AppState>) -> Result<i64, AppError> {
    // target_app 的校验交给 db 层：那里是权威来源，且能返回 InvalidInput。
    // 这里不再重复一遍，避免同一句文案在两个文件里各自漂移。
    let db = state.db.lock()?;
    db.add_profile(&profile)
}

#[tauri::command]
pub async fn update_profile(
    profile: ApiProfile,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    // 全局写锁：与其他切换/写盘命令互斥，避免并发写配置。
    // 走 apply_profile_switch（含 journal）：写盘失败时立即回滚，并保持 active 语义一致。
    let _write_guard = state.config_lock.lock()?;
    let db = state.db.lock()?;
    update_profile_locked(&db, profile)
}

/// `update_profile` 的核心逻辑（从 async 包装里提出来，便于单测）。
///
/// 前四个分支在真正碰磁盘之前就返回，因此可以用临时库 hermetic 测试；
/// 涉及 `apply_profile_switch` 的部分会写真实配置目录，不在单测覆盖范围内。
fn update_profile_locked(db: &Database, profile: ApiProfile) -> Result<(), AppError> {
    // 这里取 target_app 不是重复 db 的校验，而是下面「不许改目标工具」的
    // 比较需要它；取不到就先按传参问题挡掉，避免落进冲突分支给出误导提示。
    let Some(target) = profile.target_app else {
        return Err(AppError::invalid_input(
            "API Profile 必须指定目标工具；暂不支持通用 Profile",
        ));
    };

    let profile_id = profile
        .id
        .ok_or_else(|| AppError::invalid_input("更新 Profile 缺少 id"))?;
    // NotFound 而不是 Internal：前端据此能提示「档案已被删除，请刷新」。
    // 以前这段文本会被前端正则改写掉，既丢掉 id，又误导成「尚未初始化」。
    let previous_profile = db
        .get_profile_by_id(profile_id)?
        .ok_or_else(|| AppError::not_found(format!("Profile id={profile_id} 不存在")))?;
    if previous_profile.target_app != Some(target) {
        return Err(AppError::conflict("不能在编辑时修改 Profile 的目标工具"));
    }

    // 先保存每个 active target 的共享配置；这些快照用于失败补偿，
    // 不能在数据库更新后再从磁盘推导，否则可能读到半写入配置。
    let active_targets = db.get_active_targets_for_profile(profile_id)?;
    let mut active_contexts = Vec::with_capacity(active_targets.len());
    for active_target in active_targets {
        let persisted = db.get_shared_config(active_target)?;
        let shared = switch_api::adapters::resolve_shared_config(active_target, persisted)
            .map_err(|e| AppError::io(format!("读取 {active_target} 当前共享配置失败：{e:#}")))?;
        active_contexts.push((active_target, shared));
    }

    let mut updated_profile = profile;
    updated_profile.id = Some(profile_id);
    db.update_profile(&updated_profile)?;

    let mut applied_targets = Vec::new();
    for (active_target, shared_config) in &active_contexts {
        if let Err(error) = switch_api::adapters::apply_profile_switch(
            db,
            *active_target,
            &updated_profile,
            shared_config,
            true,
        ) {
            // 已完成的 target 也必须补偿回旧 Profile；只恢复数据库行会让
            // 本地配置继续使用新凭据，造成 GUI 与工具状态分裂。
            let mut rollback_errors = Vec::new();
            for (rollback_target, rollback_shared) in active_contexts.iter().rev() {
                if applied_targets.contains(rollback_target) {
                    if let Err(rollback_error) = switch_api::adapters::apply_profile_switch(
                        db,
                        *rollback_target,
                        &previous_profile,
                        rollback_shared,
                        true,
                    ) {
                        rollback_errors.push(format!("{rollback_target}: {rollback_error:#}"));
                    }
                }
            }
            if let Err(rollback_error) = db.update_profile(&previous_profile) {
                rollback_errors.push(format!("数据库 Profile 回滚失败: {rollback_error}"));
            }
            // 数据库已改过、部分 target 已写盘，补偿也做了——这正是 PartialFailure
            // 的语义：不是「什么都没发生」。前端可据此提示用户先核对工具实际状态，
            // 而不是当成一次可以无脑重试的干净失败。
            let suffix = if rollback_errors.is_empty() {
                String::new()
            } else {
                format!("；补偿回滚失败: {}", rollback_errors.join("；"))
            };
            return Err(AppError::partial_failure(format!(
                "更新后同步配置失败（{active_target}）：{error}{suffix}"
            ))
            .with_detail(format!("{error:#}")));
        }
        applied_targets.push(*active_target);
    }
    Ok(())
}

#[tauri::command]
pub async fn delete_profile(
    name: String,
    target_app: String,
    state: State<'_, AppState>,
) -> Result<bool, AppError> {
    let target = TargetApp::parse(&target_app).ok_or_else(|| unknown_target_app(&target_app))?;
    // Provider cleanup and profile deletion both affect persistent state and,
    // for OpenCode/ZCode, local files. Serialize them with other write paths.
    let _write_guard = state.config_lock.lock()?;
    let db = state.db.lock()?;
    if let Some(profile) = db
        .list_profiles()?
        .into_iter()
        .find(|profile| profile.name == name && profile.target_app == Some(target))
    {
        let active_id = db
            .get_active_profile(target)?
            .map(|active| active.profile_id);
        if active_id == profile.id {
            return Err(AppError::conflict(
                "不能删除当前启用的 Profile，请先启用其他 Profile",
            ));
        }
    }
    // 下面三条都是「删档案 + 清理本地文件」，本质是磁盘/数据库写失败，归为 Io。
    // 原始错误链保留在 message 里，不做信息删减。
    if target == TargetApp::OpenCode {
        return switch_api::adapters::opencode::OpenCodeAdapter::delete_profile_and_cleanup_local(
            &db, &name,
        )
        .map_err(|e| AppError::io(format!("删除 OpenCode 档案失败：{e:#}")));
    }
    if target == TargetApp::ZCode {
        return switch_api::adapters::zcode::ZCodeAdapter::delete_profile_and_cleanup_local(
            &db, &name,
        )
        .map_err(|e| AppError::io(format!("删除 ZCode 档案失败：{e:#}")));
    }
    db.delete_profile(&name, target)
        .map_err(|e| AppError::io(format!("删除档案失败：{e:#}")))
}

#[tauri::command]
pub async fn switch_profile(
    target_app: String,
    profile_name: String,
    probe: Option<bool>,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    let target = TargetApp::parse(&target_app).ok_or_else(|| unknown_target_app(&target_app))?;
    let do_probe = probe.unwrap_or(false);

    if do_probe {
        // run_failover 已返回结构化 AppError（未知 Profile / 缺模型 / 无可用 Key 等
        // 都已归类），这里直接透传，不再二次包装——二次包装会把原有类别冲掉。
        let result = run_failover(&state, target, &profile_name, false).await?;
        if !result.success {
            return Err(AppError::io(format!(
                "探活失败，未写入配置：{}",
                result
                    .tried
                    .iter()
                    .filter_map(|t| t.error.as_ref())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("; ")
            )));
        }
    }

    let (mut api_profile, persisted_shared_config) = {
        let db = state.db.lock()?;
        let api_profile = db.get_profile_by_name_and_target(&profile_name, target)?;
        let persisted_shared_config = db.get_shared_config(target)?;
        (api_profile, persisted_shared_config)
    };
    api_profile.normalize_keys();
    // 全局写锁：与其他切换/写盘命令互斥
    let _write_guard = state.config_lock.lock()?;
    let shared_config =
        switch_api::adapters::resolve_shared_config(target, persisted_shared_config)
            .map_err(|e| AppError::from(e.context("读取当前共享配置失败")))?;
    let db = state.db.lock()?;
    // 用 anyhow 的 context 补中文上下文，而不是自己 format 成新字符串：
    // 这样类别（含「回滚失败 → PartialFailure」）仍按错误类型判定，
    // 根因也留在 detail 里由前端一并显示。
    switch_api::adapters::apply_profile_switch(&db, target, &api_profile, &shared_config, true)
        .map_err(|e| AppError::from(e.context("切换配置失败")))?;

    Ok(())
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
pub async fn list_config_backups(target_app: String) -> Result<Vec<ConfigBackupInfo>, AppError> {
    use switch_api::adapters::{backup, get_adapter};
    let target = TargetApp::parse(&target_app).ok_or_else(|| unknown_target_app(&target_app))?;
    let config_dir = get_adapter(target)?.config_path();
    let config_dir = config_dir
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let backups = backup::list_backups(&config_dir)
        .map_err(|e| AppError::io(format!("读取配置备份失败：{e:#}")))?;
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
) -> Result<String, AppError> {
    use switch_api::adapters::{backup, get_adapter};
    let target = TargetApp::parse(&target_app).ok_or_else(|| unknown_target_app(&target_app))?;
    let config_dir = get_adapter(target)?.config_path();
    let config_dir = config_dir
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let _guard = state.config_lock.lock()?;
    let restored = if target == TargetApp::OpenClaw
        && std::path::Path::new(&backup_file)
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("models.backup."))
    {
        backup::restore_backup_to(
            &config_dir,
            std::path::Path::new(&backup_file),
            &config_dir
                .join("agents")
                .join("main")
                .join("agent")
                .join("models.json"),
        )
    } else {
        backup::restore_backup(&config_dir, std::path::Path::new(&backup_file))
    }
    .map_err(|e| AppError::io(format!("恢复失败：{e:#}")))?;
    Ok(restored.display().to_string())
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
        TargetApp::ZCode => vec![
            home.join(".zcode").join("skills"),
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
        Some(hooks) if hooks.as_object().is_some_and(|o| !o.is_empty()) => hooks.clone(),
        _ => empty,
    }
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

        let adapter = ClaudeCodeAdapter::new().expect("测试环境应能取到 HOME");
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

// ------------------------------------------------- update_profile 错误类别
//
// update_profile 是命令层里最容易给出误导提示的一个：档案被删掉时原本返回
// `Profile id=42 不存在`，被前端正则改写成「未找到对应数据,可能尚未初始化」，
// 既丢掉 id，又指错排查方向。这里把四个「碰磁盘之前就返回」的分支钉死。
//
// 有意不覆盖：真正走 apply_profile_switch 的路径会写开发者真实的配置目录，
// 不适合放进单测；那部分依赖 db 层与 adapter 层已有的测试。

#[cfg(test)]
mod update_profile_error_kind_tests {
    use super::*;
    use switch_api::error::ErrorKind;

    fn temp_db() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Database::open(dir.path().join("live.sqlite")).expect("open db");
        (dir, db)
    }

    fn profile_with(id: Option<i64>, target: Option<TargetApp>) -> ApiProfile {
        ApiProfile {
            id,
            name: "p".into(),
            provider: "custom".into(),
            api_url: "https://x.example".into(),
            api_key: "k".into(),
            target_app: target,
            ..Default::default()
        }
    }

    #[test]
    fn missing_id_is_invalid_input() {
        let (_dir, db) = temp_db();
        let err = update_profile_locked(&db, profile_with(None, Some(TargetApp::Codex)))
            .expect_err("缺 id 必须失败");
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
    }

    #[test]
    fn missing_target_app_is_invalid_input() {
        let (_dir, db) = temp_db();
        let err = update_profile_locked(&db, profile_with(Some(1), None))
            .expect_err("缺 target_app 必须失败");
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
    }

    #[test]
    fn unknown_profile_id_is_not_found_and_keeps_the_id() {
        let (_dir, db) = temp_db();
        let err = update_profile_locked(&db, profile_with(Some(4242), Some(TargetApp::Codex)))
            .expect_err("不存在的 profile 必须失败");
        assert_eq!(
            err.kind(),
            ErrorKind::NotFound,
            "档案被删是 NotFound 而不是 Internal；前端据此才能提示「请刷新」"
        );
        assert!(
            err.message.contains("4242"),
            "文案必须带上 id，实际为：{}",
            err.message
        );
    }

    #[test]
    fn changing_target_app_is_conflict() {
        let (_dir, db) = temp_db();
        let id = db
            .add_profile(&profile_with(None, Some(TargetApp::Codex)))
            .expect("add_profile");
        let err = update_profile_locked(&db, profile_with(Some(id), Some(TargetApp::ClaudeCode)))
            .expect_err("编辑时改目标工具必须失败");
        assert_eq!(
            err.kind(),
            ErrorKind::Conflict,
            "「不能改目标工具」是状态冲突，不是传参错误"
        );
    }
}
