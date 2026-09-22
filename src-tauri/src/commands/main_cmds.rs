// Tauri commands
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use switch_api::db::Database;
use switch_api::models::{ApiProfile, TargetApp};
use tauri::State;

use crate::commands::helpers::{
    claude_extract_models, codex_context_1m, codex_string_field, default_provider, home_dir,
    str_field,
};
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

#[tauri::command]
pub async fn copy_text(text: String) -> Result<(), AppError> {
    // 平台实现保持返回 String：Windows / Linux 分支在本机（macOS）不参与编译，
    // 改它们等于改一段无法验证的代码。边界处统一包成 Io 类错误即可，
    // 原始信息原样保留在 message 里。
    copy_text_native(&text).map_err(AppError::io)
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
        Err("当前平台不支持写入剪贴板".to_string())
    }
}

#[cfg(target_os = "macos")]
fn copy_text_with_pbcopy(text: &str) -> Result<(), String> {
    let mut child = Command::new("/usr/bin/pbcopy")
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| format!("无法启动 pbcopy：{e}"))?;

    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| "无法打开 pbcopy 的标准输入".to_string())?;
        stdin
            .write_all(text.as_bytes())
            .map_err(|e| format!("写入剪贴板失败：{e}"))?;
    }

    let status = child
        .wait()
        .map_err(|e| format!("等待 pbcopy 结束失败：{e}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("pbcopy 以非零状态退出：{status}"))
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
        .map_err(|e| format!("无法启动 PowerShell 写入剪贴板：{e}"))?;

    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| "无法打开 PowerShell 的标准输入".to_string())?;
        stdin
            .write_all(text.as_bytes())
            .map_err(|e| format!("写入剪贴板失败：{e}"))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("等待 PowerShell 结束失败：{e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "Set-Clipboard 失败（状态 {}）：{}",
            output.status,
            err.trim()
        ))
    }
}

/// Linux：优先 Wayland `wl-copy`，否则 X11 `xclip`。
#[cfg(all(unix, not(target_os = "macos")))]
fn copy_text_with_linux_clipboard(text: &str) -> Result<(), String> {
    let mut last_err = String::from("未找到可用的剪贴板后端（已尝试 wl-copy、xclip）");

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
                        last_err = format!("{bin}：写入标准输入失败：{e}");
                        continue;
                    }
                } else {
                    last_err = format!("{bin}：无法打开标准输入");
                    continue;
                }
                match child.wait_with_output() {
                    Ok(out) if out.status.success() => return Ok(()),
                    Ok(out) => {
                        last_err = format!(
                            "{bin} 以状态 {} 退出：{}",
                            out.status,
                            String::from_utf8_lossy(&out.stderr).trim()
                        );
                    }
                    Err(e) => last_err = format!("{bin}：等待进程结束失败：{e}"),
                }
            }
            Err(e) => last_err = format!("{bin}：{e}"),
        }
    }

    Err(format!("写入剪贴板失败：{last_err}"))
}

// 新增：获取完整的本地配置信息
#[tauri::command]
pub async fn get_local_config_info(target_app: String) -> Result<LocalConfigInfo, AppError> {
    let target = TargetApp::parse(&target_app).ok_or_else(|| unknown_target_app(&target_app))?;

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
    let config_dir = get_adapter(target).config_path();
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
    let config_dir = get_adapter(target).config_path();
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

/// 读取 Codex 的 config.toml 原始文本（不经 JSON 往返，保留用户格式/注释）。
/// 文件不存在时返回空字符串。仅 Codex 提供此能力。
#[tauri::command]
pub async fn read_codex_config_raw() -> Result<String, AppError> {
    use switch_api::adapters::get_adapter;
    let path = get_adapter(TargetApp::Codex).config_path();
    if !path.exists() {
        return Ok(String::new());
    }
    std::fs::read_to_string(&path)
        .map_err(|e| AppError::from(e).with_context("读取 config.toml 失败"))
}

fn toml_string_field(value: Option<&toml::Value>, key: &str) -> Option<String> {
    value
        .and_then(|value| value.as_table())
        .and_then(|table| table.get(key))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn sync_codex_profile_from_raw_config(profile: &ApiProfile, config: &toml::Value) -> ApiProfile {
    let mut synced = profile.clone();
    let provider_id = toml_string_field(Some(config), "model_provider")
        .unwrap_or_else(|| profile.provider.clone());
    let provider = config
        .as_table()
        .and_then(|table| table.get("model_providers"))
        .and_then(|value| value.as_table())
        .and_then(|providers| providers.get(&provider_id));

    synced.provider = provider_id.clone();
    synced.target_app = Some(TargetApp::Codex);
    synced.model = toml_string_field(Some(config), "model");
    synced.context_1m = config
        .as_table()
        .and_then(|table| table.get("model_context_window"))
        .and_then(|value| value.as_integer())
        .map(|value| value >= 1_000_000);
    synced.codex.reasoning_effort = toml_string_field(Some(config), "model_reasoning_effort");
    synced.codex.reasoning_summary = toml_string_field(Some(config), "model_reasoning_summary");
    synced.codex.verbosity = toml_string_field(Some(config), "model_verbosity");
    synced.codex.service_tier = toml_string_field(Some(config), "service_tier");
    synced.codex.wire_api = toml_string_field(provider, "wire_api")
        .and_then(|w| switch_api::models::normalize_wire_api(Some(&w)).or(Some(w)));
    synced.codex.env_key = toml_string_field(provider, "env_key");
    synced.codex.experimental_bearer_token =
        toml_string_field(provider, "experimental_bearer_token");
    let auth_table = provider
        .and_then(|value| value.as_table())
        .and_then(|table| table.get("auth"))
        .and_then(|value| value.as_table());
    synced.codex.auth_command = auth_table
        .and_then(|table| table.get("command"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    synced.codex.auth_args = auth_table
        .and_then(|table| table.get("args"))
        .and_then(|value| value.as_array())
        .map(|args| {
            args.iter()
                .filter_map(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|args| !args.is_empty());
    synced.codex.auth_timeout_ms = auth_table
        .and_then(|table| table.get("timeout_ms"))
        .and_then(|value| value.as_integer())
        .filter(|value| *value > 0);
    synced.codex.auth_refresh_interval_ms = auth_table
        .and_then(|table| table.get("refresh_interval_ms"))
        .and_then(|value| value.as_integer())
        .filter(|value| *value > 0);
    synced.codex.auth_cwd = auth_table
        .and_then(|table| table.get("cwd"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    synced.codex.requires_openai_auth = provider
        .and_then(|value| value.as_table())
        .and_then(|table| table.get("requires_openai_auth"))
        .and_then(|value| value.as_bool());
    synced.codex.supports_standalone_web_search = provider
        .and_then(|value| value.as_table())
        .and_then(|table| table.get("supports_standalone_web_search"))
        .and_then(|value| value.as_bool());

    if provider_id == "amazon-bedrock" {
        synced.api_url.clear();
        synced.codex.aws_profile = toml_string_field(
            provider
                .and_then(|value| value.as_table())
                .and_then(|table| table.get("aws")),
            "profile",
        );
        synced.codex.aws_region = toml_string_field(
            provider
                .and_then(|value| value.as_table())
                .and_then(|table| table.get("aws")),
            "region",
        );
    } else {
        synced.api_url = toml_string_field(provider, "base_url")
            .or_else(|| toml_string_field(Some(config), "base_url"))
            .unwrap_or_default();
        synced.codex.aws_profile = None;
        synced.codex.aws_region = None;
    }

    synced
}

fn restore_codex_raw_file(path: &std::path::Path, previous: Option<&[u8]>) -> Result<(), String> {
    match previous {
        Some(contents) => switch_api::utils::secure_fs::atomic_write_private(path, contents)
            .map_err(|error| format!("恢复 Codex config.toml 失败：{error}")),
        None if path.exists() => {
            std::fs::remove_file(path).map_err(|error| format!("删除新 Codex config 失败：{error}"))
        }
        None => Ok(()),
    }
}

fn restore_codex_profile_state(
    db: &Database,
    previous_profile: Option<&ApiProfile>,
    previous_shared_config: Option<&serde_json::Value>,
) -> Result<(), String> {
    if let Some(profile) = previous_profile {
        db.update_profile(profile)
            .map_err(|error| format!("恢复 Codex active Profile 失败：{error}"))?;
    }
    match previous_shared_config {
        Some(config) => db
            .save_shared_config(TargetApp::Codex, config.clone())
            .map_err(|error| format!("恢复 Codex shared config 失败：{error}")),
        None => db
            .delete_shared_config(TargetApp::Codex)
            .map_err(|error| format!("删除失败的 Codex shared config 失败：{error}")),
    }
}

/// 把回滚结果并入错误：**回滚失败 = 部分生效 → PartialFailure**。
///
/// 这条判断很关键。回滚成功的失败是干净的（什么都没落地，可以放心重试）；
/// 回滚失败的失败意味着磁盘上可能留着半套配置，前端必须提示用户去核对工具
/// 实际状态，而不是让用户重试一遍。
fn with_rollback(err: AppError, rollback_errors: Vec<String>) -> AppError {
    if rollback_errors.is_empty() {
        return err;
    }
    AppError::partial_failure(format!(
        "{}；回滚失败：{}",
        err.message,
        rollback_errors.join("；")
    ))
    .with_detail(err.detail.unwrap_or_default())
}

fn persist_codex_raw_config(content: &str, state: &AppState) -> Result<(), AppError> {
    let parsed = toml::from_str::<toml::Value>(content)
        .map_err(|error| AppError::invalid_input(format!("TOML 语法错误，未保存：{error}")))?;
    let adapter = switch_api::adapters::get_adapter(TargetApp::Codex);
    let path = adapter.config_path();
    let shared = adapter.extract_shared_config(
        &serde_json::to_value(&parsed)
            .map_err(|error| AppError::internal(format!("转换 TOML 失败：{error}")))?,
    );
    let previous_contents =
        if path.exists() {
            Some(std::fs::read(&path).map_err(|error| {
                AppError::from(error).with_context("读取当前 Codex config 失败")
            })?)
        } else {
            None
        };

    let _write_guard = state.config_lock.lock()?;
    let db = state.db.lock()?;
    let previous_profile = db
        .get_active_profile_full(TargetApp::Codex)
        .map_err(|error| AppError::from(error).with_context("读取 Codex active Profile 失败"))?;
    let previous_shared_config = db
        .get_shared_config(TargetApp::Codex)
        .map_err(|error| AppError::from(error).with_context("读取 Codex shared config 失败"))?
        .map(|config| config.config);
    let synced_profile = previous_profile
        .as_ref()
        .map(|profile| sync_codex_profile_from_raw_config(profile, &parsed));

    if path.exists() {
        adapter
            .backup_config()
            .map_err(|error| AppError::from(error).with_context("备份当前配置失败"))?;
    }

    if let Some(profile) = synced_profile.as_ref() {
        // db 层已经返回 AppError，直接补上下文即可；再套一层 AppError::from 是空转。
        db.update_profile(profile)
            .map_err(|error| error.with_context("同步 Codex active Profile 失败"))?;
    }

    // 两处写盘失败走同一套补偿：数据库行 + config.toml 都要还原。
    let rollback = |db: &Database| -> Vec<String> {
        let mut errors = Vec::new();
        if let Err(e) = restore_codex_profile_state(
            db,
            previous_profile.as_ref(),
            previous_shared_config.as_ref(),
        ) {
            errors.push(e);
        }
        if let Err(e) = restore_codex_raw_file(&path, previous_contents.as_deref()) {
            errors.push(e);
        }
        errors
    };

    if let Err(error) = validate_and_write_codex_config_raw(content, &path) {
        return Err(with_rollback(AppError::io(error), rollback(&db)));
    }

    if let Err(error) = db.save_shared_config(TargetApp::Codex, shared) {
        return Err(with_rollback(
            error.with_context("保存 Codex shared config 失败"),
            rollback(&db),
        ));
    }

    Ok(())
}

/// 保存用户在 GUI 里手编的 Codex config.toml 原始文本。
/// 高风险写操作：必须「校验通过才写」+「写前备份」。
#[tauri::command]
pub async fn save_codex_config_raw(
    content: String,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    persist_codex_raw_config(&content, &state)
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

/// 编辑 Codex 全局行为字段（approval_policy / sandbox_mode 等顶层键）并写回
/// ~/.codex/config.toml。
///
/// 走**保留格式**的编辑路径：直接在 live 文本上改这几个顶层键，其余内容
/// （注释、键序、空行、子表）一律不动。早期实现是「TOML → JSON → 改字段 →
/// 全量重新序列化」，会把用户手写的注释和键序全部洗掉。
///
/// 与原始文本编辑共用同一事务路径（校验 + 备份 + 原子写 + 同步 active Profile）。
#[tauri::command]
pub async fn update_codex_fields(
    fields: serde_json::Value,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    use switch_api::adapters::get_adapter;
    let adapter = get_adapter(TargetApp::Codex);
    let path = adapter.config_path();

    let live_text = if path.exists() {
        std::fs::read_to_string(&path)
            .map_err(|e| AppError::from(e).with_context("读取 config.toml 失败"))?
    } else {
        String::new()
    };

    let updates = fields
        .as_object()
        .ok_or_else(|| AppError::invalid_input("字段更新必须是一个对象"))?;

    let content = switch_api::doc::toml::apply_top_level_updates(&live_text, updates)
        .map_err(|e| AppError::invalid_input(format!("更新 config.toml 失败：{e}")))?;

    persist_codex_raw_config(&content, &state)
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
    pub reasoning_summary: Option<String>,
    pub verbosity: Option<String>,
    pub context_1m: Option<bool>,
    pub wire_api: Option<String>,
    pub env_key: Option<String>,
    pub requires_openai_auth: Option<bool>,
    pub experimental_bearer_token: Option<String>,
    pub service_tier: Option<String>,
    pub supports_standalone_web_search: Option<bool>,
    pub aws_profile: Option<String>,
    pub aws_region: Option<String>,
    pub auth_command: Option<String>,
    pub auth_args: Option<Vec<String>>,
    pub auth_timeout_ms: Option<i64>,
    pub auth_refresh_interval_ms: Option<i64>,
    pub auth_cwd: Option<String>,
    /// Hermes / OpenClaw 协议模式（独立字段，不再借用 wire_api）
    pub api_mode: Option<String>,
    /// OpenCode provider SDK mode.
    pub opencode_api_mode: Option<String>,
    /// OpenCode provider model declarations and per-model config.
    pub opencode_models: Option<Vec<String>>,
    pub opencode_model_configs: Option<std::collections::HashMap<String, serde_json::Value>>,
    /// OpenClaw models[].maxTokens
    pub max_tokens: Option<i64>,
    /// 来源配置文件路径，便于用户确认
    pub source: String,
}

fn first_non_empty(values: impl IntoIterator<Item = String>) -> String {
    values
        .into_iter()
        .find(|value| !value.trim().is_empty())
        .unwrap_or_default()
}

fn configured_opencode_provider_id(config: &serde_json::Value) -> Option<String> {
    ["model", "small_model"]
        .into_iter()
        .filter_map(|key| config.get(key).and_then(|value| value.as_str()))
        .filter_map(|model| model.split_once('/').map(|(provider, _)| provider.trim()))
        .find(|provider| !provider.is_empty())
        .map(str::to_string)
}

/// 读取某工具当前配置文件，提取其中的 API URL / Key（不写库，仅返回供预览）
#[tauri::command]
pub async fn scan_local_api(target_app: String) -> Result<ScannedApi, AppError> {
    use switch_api::adapters::get_adapter;
    let target = TargetApp::parse(&target_app).ok_or_else(|| unknown_target_app(&target_app))?;
    let adapter = get_adapter(target);
    let source = adapter.config_path().to_string_lossy().to_string();
    let cfg = adapter
        .read_config()
        .map_err(|e| AppError::from(e).with_context("读取本地配置文件失败"))?;
    Ok(scan_api_from_config(
        target,
        &cfg,
        source,
        dirs::home_dir().as_deref(),
    ))
}

/// 单次扫描过程中累积的原始字段。
///
/// 7 个工具的解析函数只往这里写，不做任何跨工具语义转换；最终 `ScannedApi`
/// 由 [`scan_api_from_config`] 统一组装。这样每个工具的解析逻辑都能独立成
/// 函数，不必让 20+ 个可变局部变量在同一个巨型 `match` 里穿梭。
#[derive(Default)]
struct ScanParts {
    url: String,
    key: String,
    provider: String,
    /// Codex provider 块内的协议字段
    wire_api: Option<String>,
    codex_env_key: Option<String>,
    requires_openai_auth: Option<bool>,
    experimental_bearer_token: Option<String>,
    supports_standalone_web_search: Option<bool>,
    aws_profile: Option<String>,
    aws_region: Option<String>,
    auth_command: Option<String>,
    auth_args: Option<Vec<String>>,
    auth_timeout_ms: Option<i64>,
    auth_refresh_interval_ms: Option<i64>,
    auth_cwd: Option<String>,
    /// Claude Code 的默认模型（Pi / Hermes / OpenClaw / ZCode 复用此槽位）
    claude_model: Option<String>,
    /// Claude Code 的角色映射
    claude_mapping: Option<std::collections::HashMap<String, String>>,
    /// Hermes / OpenClaw 协议模式；Pi 期间借用为 provider 临时载体
    api_mode: Option<String>,
    /// OpenCode provider SDK mode
    opencode_api_mode: Option<String>,
    opencode_models: Option<Vec<String>>,
    opencode_model_configs: Option<std::collections::HashMap<String, serde_json::Value>>,
    /// OpenClaw `models[].maxTokens`
    max_tokens: Option<i64>,
    context_1m: Option<bool>,
}

/// 从已解析的工具配置中提取 API 凭据，供「从本地导入」预览。
///
/// 只接收已读好的 `cfg`；Pi / OpenCode / Codex 的 key 可能落在 `auth.json`
/// 等辅助文件或环境变量里，这些仍在本函数内读取。拆成同步函数、并把 `home`
/// 显式传入，是为了能脱离 async 运行时与进程级 `$HOME` 做行为回归测试。
fn scan_api_from_config(
    target: TargetApp,
    cfg: &serde_json::Value,
    source: String,
    home: Option<&std::path::Path>,
) -> ScannedApi {
    let mut parts = ScanParts {
        provider: default_provider(target),
        ..Default::default()
    };

    match target {
        TargetApp::ClaudeCode => scan_claude_code(cfg, &mut parts),
        TargetApp::Codex => scan_codex(cfg, home, &mut parts),
        TargetApp::Pi => scan_pi(cfg, home, &mut parts),
        TargetApp::OpenCode => scan_opencode(cfg, home, &mut parts),
        TargetApp::Hermes => scan_hermes(cfg, &mut parts),
        TargetApp::OpenClaw => scan_openclaw(cfg, &mut parts),
        TargetApp::ZCode => scan_zcode(cfg, &mut parts),
    }

    finalize_provider(target, cfg, &mut parts);

    // Codex/Claude keep their own context_1m path; Hermes/OpenClaw/ZCode use local scan.
    let resolved_context_1m = match target {
        TargetApp::Hermes | TargetApp::OpenClaw | TargetApp::ZCode => parts.context_1m,
        _ => codex_context_1m(target, cfg),
    };

    ScannedApi {
        found: !parts.url.is_empty()
            || !parts.key.is_empty()
            || (target == TargetApp::Codex && parts.provider == "amazon-bedrock"),
        api_url: parts.url,
        api_key: parts.key,
        provider: parts.provider,
        model: if target == TargetApp::Hermes
            || target == TargetApp::OpenClaw
            || target == TargetApp::Pi
            || target == TargetApp::ZCode
        {
            parts.claude_model
        } else {
            codex_string_field(target, cfg, "model").or(parts.claude_model)
        },
        model_mapping: parts.claude_mapping,
        reasoning_effort: codex_string_field(target, cfg, "model_reasoning_effort"),
        reasoning_summary: codex_string_field(target, cfg, "model_reasoning_summary"),
        verbosity: codex_string_field(target, cfg, "model_verbosity"),
        context_1m: resolved_context_1m,
        wire_api: parts.wire_api,
        env_key: parts.codex_env_key,
        requires_openai_auth: parts.requires_openai_auth,
        experimental_bearer_token: parts.experimental_bearer_token,
        service_tier: codex_string_field(target, cfg, "service_tier"),
        supports_standalone_web_search: parts.supports_standalone_web_search,
        aws_profile: parts.aws_profile,
        aws_region: parts.aws_region,
        auth_command: parts.auth_command,
        auth_args: parts.auth_args,
        auth_timeout_ms: parts.auth_timeout_ms,
        auth_refresh_interval_ms: parts.auth_refresh_interval_ms,
        auth_cwd: parts.auth_cwd,
        api_mode: parts.api_mode,
        opencode_api_mode: parts.opencode_api_mode,
        opencode_models: parts.opencode_models,
        opencode_model_configs: parts.opencode_model_configs,
        max_tokens: parts.max_tokens,
        source,
    }
}

/// Claude Code：API 在 `env.ANTHROPIC_*`，配置来自 `~/.claude/settings.json`
/// （adapter 已把该文件内容作为 `cfg` 传入）。
fn scan_claude_code(cfg: &serde_json::Value, parts: &mut ScanParts) {
    if let Some(env) = cfg.get("env") {
        parts.url = str_field(env, "ANTHROPIC_BASE_URL");
        parts.key = first_non_empty([
            str_field(env, "ANTHROPIC_AUTH_TOKEN"),
            str_field(env, "ANTHROPIC_API_KEY"),
        ]);
        claude_extract_models(env, &mut parts.claude_model, &mut parts.claude_mapping);
    }
}

/// Codex：`model_provider` 指明当前 provider，`base_url` 在
/// `[model_providers.<id>]` 块；api key 不在 `config.toml`（走 `auth.json` /
/// `OPENAI_API_KEY` 环境变量），这里尽力读取。
fn scan_codex(cfg: &serde_json::Value, home: Option<&std::path::Path>, parts: &mut ScanParts) {
    let pid = str_field(cfg, "model_provider");
    if let Some(providers) = cfg.get("model_providers").and_then(|v| v.as_object()) {
        // 优先用 model_provider 指定的块，否则取第一个
        let block = providers.get(&pid).or_else(|| providers.values().next());
        if let Some(b) = block {
            if pid == "amazon-bedrock" {
                if let Some(aws) = b.get("aws") {
                    let profile = str_field(aws, "profile");
                    if !profile.is_empty() {
                        parts.aws_profile = Some(profile);
                    }
                    let region = str_field(aws, "region");
                    if !region.is_empty() {
                        parts.aws_region = Some(region);
                    }
                }
            } else {
                parts.url = str_field(b, "base_url");
                // 某些配置把 key 写在 provider 块里
                if parts.key.is_empty() {
                    parts.key = str_field(b, "api_key");
                }
                // env_key 指向环境变量名
                let env_key = str_field(b, "env_key");
                if !env_key.trim().is_empty() {
                    parts.codex_env_key = Some(env_key.clone());
                }
                if parts.key.is_empty() && !env_key.is_empty() {
                    parts.key = std::env::var(&env_key).unwrap_or_default();
                }
                // 回带 provider 块内的协议字段，供导入还原；
                // chat 系历史值归一为 responses（官方已删除 chat）。
                let w = str_field(b, "wire_api");
                if !w.trim().is_empty() {
                    parts.wire_api = switch_api::models::normalize_wire_api(Some(&w)).or(Some(w));
                }
                parts.requires_openai_auth =
                    b.get("requires_openai_auth").and_then(|v| v.as_bool());
                parts.supports_standalone_web_search = b
                    .get("supports_standalone_web_search")
                    .and_then(|v| v.as_bool());
                let bearer = str_field(b, "experimental_bearer_token");
                if !bearer.trim().is_empty() {
                    parts.experimental_bearer_token = Some(bearer);
                }
                if let Some(auth) = b.get("auth").and_then(|v| v.as_object()) {
                    let command = auth
                        .get("command")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .trim();
                    if !command.is_empty() {
                        parts.auth_command = Some(command.to_string());
                    }
                    let args = auth
                        .get("args")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str())
                                .map(str::trim)
                                .filter(|s| !s.is_empty())
                                .map(str::to_string)
                                .collect::<Vec<_>>()
                        })
                        .filter(|args| !args.is_empty());
                    if args.is_some() {
                        parts.auth_args = args;
                    }
                    parts.auth_timeout_ms = auth
                        .get("timeout_ms")
                        .and_then(|v| v.as_i64())
                        .filter(|v| *v > 0);
                    parts.auth_refresh_interval_ms = auth
                        .get("refresh_interval_ms")
                        .and_then(|v| v.as_i64())
                        .filter(|v| *v > 0);
                    let cwd = auth
                        .get("cwd")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .trim();
                    if !cwd.is_empty() {
                        parts.auth_cwd = Some(cwd.to_string());
                    }
                }
            }
        }
    }
    // 顶层兜底
    if pid != "amazon-bedrock" && parts.url.is_empty() {
        parts.url = str_field(cfg, "base_url");
    }
    // 从 auth.json 或常见环境变量读 key
    if pid != "amazon-bedrock" && parts.key.is_empty() {
        if let Some(home) = home {
            let auth = home.join(".codex").join("auth.json");
            if let Ok(c) = std::fs::read_to_string(&auth) {
                if let Ok(j) = serde_json::from_str::<serde_json::Value>(&c) {
                    parts.key = str_field(&j, "OPENAI_API_KEY");
                    if parts.key.is_empty() {
                        parts.key = str_field(&j, "api_key");
                    }
                }
            }
        }
    }
    if pid != "amazon-bedrock" && parts.key.is_empty() {
        parts.key = std::env::var("OPENAI_API_KEY").unwrap_or_default();
    }
}

/// Pi：`defaultProvider`/`defaultModel` 在 `settings.json`；key 在 `auth.json`；
/// 自定义 `baseUrl` 在 `models.json` 的 `providers.<id>`。
fn scan_pi(cfg: &serde_json::Value, home: Option<&std::path::Path>, parts: &mut ScanParts) {
    let default_provider = str_field(cfg, "defaultProvider");
    let default_model = str_field(cfg, "defaultModel");
    if !default_model.is_empty() {
        parts.claude_model = Some(default_model);
    }
    let mut provider_id = default_provider;
    if let Some(home) = home {
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
                        parts.url = str_field(b, "baseUrl");
                        if parts.key.is_empty() {
                            parts.key = str_field(b, "apiKey");
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
                        parts.key = k;
                    }
                }
            }
        }
    }
    if !provider_id.is_empty() {
        // 借用 api_mode 槽位做临时载体，组装响应时清空
        parts.api_mode = Some(format!("__pi_provider__:{provider_id}"));
    }
}

/// OpenCode：`provider.<id>.options.{apiKey,baseURL}`，优先使用顶层
/// `model`/`small_model` 指向的 provider，不能任意取第一个。
fn scan_opencode(cfg: &serde_json::Value, home: Option<&std::path::Path>, parts: &mut ScanParts) {
    let mut provider_id = String::new();
    if let Some(providers) = cfg.get("provider").and_then(|v| v.as_object()) {
        if let Some(pid) = configured_opencode_provider_id(cfg) {
            if let Some(pv) = providers.get(&pid) {
                provider_id = pid;
                if let Some(opts) = pv.get("options") {
                    parts.url = str_field(opts, "baseURL");
                    parts.key = str_field(opts, "apiKey");
                }
                parts.opencode_api_mode = match str_field(pv, "npm").as_str() {
                    "@ai-sdk/openai" => Some("responses".to_string()),
                    "@ai-sdk/openai-compatible" => Some("chat_completions".to_string()),
                    _ => None,
                };
                if let Some(models) = pv.get("models").and_then(|v| v.as_object()) {
                    let mut ids = Vec::with_capacity(models.len());
                    let mut configs = std::collections::HashMap::new();
                    for (model_id, model_config) in models {
                        ids.push(model_id.clone());
                        if model_config.is_object() {
                            configs.insert(model_id.clone(), model_config.clone());
                        }
                    }
                    ids.sort();
                    parts.opencode_models = (!ids.is_empty()).then_some(ids);
                    parts.opencode_model_configs = (!configs.is_empty()).then_some(configs);
                }
            }
        }
    }
    // key 缺失或是文件/环境引用占位（{file:...} / {env:...}）时，
    // 从 ~/.local/share/opencode/auth.json 读真实 key。
    if parts.key.is_empty() || parts.key.starts_with("{file:") || parts.key.starts_with("{env:") {
        if let Some(home) = home {
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
                            parts.key = k;
                        }
                    }
                }
            }
        }
    }
}

/// Hermes：`model.provider = custom:<name>` +
/// `custom_providers[].{base_url,api_key,api_mode}`。
fn scan_hermes(cfg: &serde_json::Value, parts: &mut ScanParts) {
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
                parts.url = str_field(entry, "base_url");
                parts.key = str_field(entry, "api_key");
                let mode = str_field(entry, "api_mode");
                if !mode.is_empty() {
                    parts.api_mode = Some(mode);
                }
                break;
            }
        }
    }
    if let Some(m) = model_obj {
        parts.claude_model = m
            .get("default")
            .or_else(|| m.get("model"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        if parts.api_mode.is_none() {
            let mode = str_field(m, "api_mode");
            if !mode.is_empty() {
                parts.api_mode = Some(mode);
            }
        }
        if let Some(ctx) = m.get("context_length").and_then(|v| v.as_i64()) {
            parts.context_1m = Some(ctx >= 1_000_000);
        }
    }
}

/// OpenClaw：`agents.defaults.model.primary = "provider/model"` +
/// `models.providers.<id>.{baseUrl,apiKey,api,models[]}`。
fn scan_openclaw(cfg: &serde_json::Value, parts: &mut ScanParts) {
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
            parts.url = str_field(b, "baseUrl");
            if parts.url.is_empty() {
                parts.url = str_field(b, "base_url");
            }
            parts.key = str_field(b, "apiKey");
            if parts.key.is_empty() {
                parts.key = str_field(b, "api_key");
            }
            let mode = str_field(b, "api");
            if !mode.is_empty() {
                // 把 OpenClaw 的 api 字符串归一到 Helio 形式
                parts.api_mode = Some(match mode.as_str() {
                    "openai-completions" => "chat_completions".into(),
                    "anthropic-messages" => "anthropic_messages".into(),
                    "openai-responses" => "codex_responses".into(),
                    other => other.to_string(),
                });
            }
            if !mid.is_empty() {
                if let Some(models) = b.get("models").and_then(|v| v.as_array()) {
                    if let Some(m) = models
                        .iter()
                        .find(|m| m.get("id").and_then(|v| v.as_str()) == Some(mid.as_str()))
                    {
                        if let Some(cw) = m.get("contextWindow").and_then(|v| v.as_i64()) {
                            parts.context_1m = Some(cw >= 1_000_000);
                        }
                        if let Some(mt) = m.get("maxTokens").and_then(|v| v.as_i64()) {
                            if mt > 0 {
                                parts.max_tokens = Some(mt);
                            }
                        }
                    }
                }
            }
        }
    }
    if !mid.is_empty() {
        parts.claude_model = Some(mid);
    }
    // agents.defaults.contextTokens 作为 1M 判定的兜底
    if parts.context_1m.is_none() {
        if let Some(ct) = cfg
            .pointer("/agents/defaults/contextTokens")
            .and_then(|v| v.as_i64())
        {
            parts.context_1m = Some(ct >= 1_000_000);
        }
    }
}

/// ZCode：OpenCode 形状的 `provider.<id>.options.{apiKey,baseURL}`。
/// 依次优先：顶层 model 指向的 provider → 自定义 Helio Anthropic 条目 → 第一个 provider。
fn scan_zcode(cfg: &serde_json::Value, parts: &mut ScanParts) {
    let preferred = cfg
        .get("model")
        .and_then(|v| v.as_str())
        .and_then(|m| m.split_once('/'))
        .map(|(p, m)| (p.to_string(), m.to_string()));
    if let Some(providers) = cfg.get("provider").and_then(|v| v.as_object()) {
        let selected = preferred
            .as_ref()
            .and_then(|(pid, _)| providers.get_key_value(pid.as_str()))
            .or_else(|| {
                providers.iter().find(|(_, pv)| {
                    pv.get("source").and_then(|v| v.as_str()) == Some("custom")
                        && pv.get("kind").and_then(|v| v.as_str()) == Some("anthropic")
                })
            })
            .or_else(|| providers.iter().next());
        if let Some((pid, pv)) = selected {
            parts.provider = pid.clone();
            if let Some(opts) = pv.get("options") {
                parts.url = str_field(opts, "baseURL");
                parts.key = str_field(opts, "apiKey");
            }
            if let Some((_, mid)) = preferred.as_ref() {
                if !mid.is_empty() {
                    parts.claude_model = Some(mid.clone());
                }
            } else if let Some(models) = pv.get("models").and_then(|v| v.as_object()) {
                if let Some((mid, _)) = models.iter().next() {
                    parts.claude_model = Some(mid.clone());
                }
            }
            if let Some(mid) = parts.claude_model.as_deref() {
                if let Some(ctx) = pv
                    .get("models")
                    .and_then(|v| v.get(mid))
                    .and_then(|m| m.get("limit"))
                    .and_then(|l| l.get("context"))
                    .and_then(|v| v.as_i64())
                {
                    parts.context_1m = Some(ctx >= 1_000_000);
                }
            }
        }
    }
}

/// 各工具对 `provider` 的还原规则差异很大，统一收口在这里。
fn finalize_provider(target: TargetApp, cfg: &serde_json::Value, parts: &mut ScanParts) {
    // Hermes 把 provider 名从 model.provider 还原（去 custom:）
    // OpenClaw 从 agents.defaults.model.primary 的 provider/ 前缀还原
    if target == TargetApp::Hermes {
        let slug = cfg
            .get("model")
            .and_then(|m| m.get("provider"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let name = slug.strip_prefix("custom:").unwrap_or(slug);
        if !name.is_empty() {
            parts.provider = name.to_string();
        }
    }
    if target == TargetApp::OpenClaw {
        let primary = cfg
            .pointer("/agents/defaults/model/primary")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if let Some((prefix, _)) = primary.split_once('/') {
            if !prefix.is_empty() {
                parts.provider = prefix.to_string();
            }
        }
    }
    if target == TargetApp::Pi {
        if let Some(marker) = parts.api_mode.clone() {
            if let Some(pid) = marker.strip_prefix("__pi_provider__:") {
                if !pid.is_empty() {
                    parts.provider = pid.to_string();
                }
            }
        }
        parts.api_mode = None;
    }
    if target == TargetApp::Codex {
        let active_provider = str_field(cfg, "model_provider");
        if !active_provider.is_empty() {
            parts.provider = active_provider;
        }
    }
}

#[tauri::command]
pub async fn export_database(
    output_path: String,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    let _db = state.db.lock()?;
    let db_path = default_db_path()?;

    // 快照而非文件拷贝：拷主文件会漏掉还在 -wal 里的已提交数据（实测可导出成空档案库）。
    // 导出目标由用户选择，snapshot_to 只收紧文件本身权限，不动其所在目录。
    Database::snapshot_to(&db_path, std::path::Path::new(&output_path))
        .map_err(|e| AppError::from(e).with_context("导出数据库失败"))?;

    Ok(())
}

#[tauri::command]
pub async fn export_portable_backup(
    output_path: String,
    state: State<'_, AppState>,
) -> Result<switch_api::utils::portable_backup::PortableBackupExportResult, AppError> {
    let _write_guard = state.config_lock.lock()?;
    let db = state.db.lock()?;
    switch_api::adapters::sync_all_shared_configs(&db)
        .map_err(|e| AppError::from(e).with_context("导出前同步共享配置失败"))?;
    let db_path = default_db_path()?;
    let home = home_dir()?;
    switch_api::utils::portable_backup::export_portable_backup(
        &home,
        &db_path,
        std::path::Path::new(&output_path),
    )
    .map_err(|e| AppError::from(e).with_context("导出便携备份失败"))
}

/// 把全部 skills 目录打包为 tar.gz（manifest + {app}/{skill}/...）。
/// 与数据库备份正交：skills 是文件系统资产，不入库。
#[tauri::command]
pub async fn export_skills(
    output_path: String,
    state: State<'_, AppState>,
) -> Result<switch_api::utils::skills_backup::SkillsExportResult, AppError> {
    // 与配置写路径互斥：导出期间避免并发切换改到半截 skill 目录。
    let _write_guard = state.config_lock.lock()?;
    let home = home_dir()?;
    switch_api::utils::skills_backup::export_skills(&home, std::path::Path::new(&output_path))
        .map_err(|e| AppError::from(e).with_context("导出 Skills 失败"))
}

/// 从 tar.gz 归档恢复 skills。整体校验不通过则拒绝且不写盘；
/// 同名 skill 目录已存在时跳过（不覆盖）。
#[tauri::command]
pub async fn import_skills(
    input_path: String,
    state: State<'_, AppState>,
) -> Result<switch_api::utils::skills_backup::SkillsImportResult, AppError> {
    // 与切换/写配置互斥；skills_backup 内部另有进程级 IMPORT_LOCK 防重入。
    let _write_guard = state.config_lock.lock()?;
    let home = home_dir()?;
    switch_api::utils::skills_backup::import_skills(&home, std::path::Path::new(&input_path))
        .map_err(|e| AppError::from(e).with_context("导入 Skills 失败"))
}

#[tauri::command]
pub async fn import_database(
    input_path: String,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    let db_path = default_db_path()?;

    // 与切换/写配置互斥：导入会替换 live 库文件，期间绝不能有其他命令仍持有旧连接写盘。
    let _write_guard = state.config_lock.lock()?;
    let snapshots = switch_api::adapters::snapshot_all_managed_files()
        .map_err(|e| AppError::from(e).with_context("导入前快照工具配置失败"))?;
    let mut db = state.db.lock()?;
    let backup = replace_database_locked(std::path::Path::new(&input_path), &db_path, &mut db)?;
    if let Err(error) = switch_api::adapters::materialize_active_profiles(&db) {
        let rollback =
            rollback_import_state(&db_path, &mut db, backup.as_deref(), &snapshots, None);
        return Err(import_after_apply_failure("恢复生效配置", error, rollback));
    }
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct PortableBackupImportResult {
    pub restored_targets: Vec<String>,
    pub skills: switch_api::utils::skills_backup::SkillsImportResult,
}

#[tauri::command]
pub async fn import_portable_backup(
    input_path: String,
    state: State<'_, AppState>,
) -> Result<PortableBackupImportResult, AppError> {
    let _write_guard = state.config_lock.lock()?;
    let archive = switch_api::utils::portable_backup::extract_portable_backup(
        std::path::Path::new(&input_path),
    )
    .map_err(|e| AppError::from(e).with_context("便携备份校验失败"))?;
    Database::validate_import_candidate(&archive.database_path)
        .map_err(|e| AppError::from(e).with_context("便携备份中的数据库无效"))?;

    let db_path = default_db_path()?;
    let snapshots = switch_api::adapters::snapshot_all_managed_files()
        .map_err(|e| AppError::from(e).with_context("导入前快照工具配置失败"))?;
    let mut db = state.db.lock()?;
    let backup = replace_database_locked(&archive.database_path, &db_path, &mut db)?;

    let home = home_dir()?;
    let skills = match switch_api::utils::skills_backup::import_skills(&home, &archive.skills_path)
    {
        Ok(skills) => skills,
        Err(error) => {
            let rollback =
                rollback_import_state(&db_path, &mut db, backup.as_deref(), &snapshots, None);
            return Err(import_after_apply_failure("恢复 Skills", error, rollback));
        }
    };
    let restored_targets = match switch_api::adapters::materialize_active_profiles(&db) {
        Ok(targets) => targets
            .into_iter()
            .map(|target| target.as_str().to_string())
            .collect(),
        Err(error) => {
            let rollback = rollback_import_state(
                &db_path,
                &mut db,
                backup.as_deref(),
                &snapshots,
                Some((&home, &skills)),
            );
            return Err(import_after_apply_failure("恢复生效配置", error, rollback));
        }
    };
    Ok(PortableBackupImportResult {
        restored_targets,
        skills,
    })
}

/// 导入流程「数据库已替换、但后续步骤失败」的统一构造。
///
/// 两个分支的语义差别很大，不能共用一句话：
///
/// - **回滚成功**：磁盘已复原到导入前，用户什么都不用做，只是这次导入没生效。
///   类别沿用底层错误的真实类型（`AppError::from` 按类型分类），不要一律标成
///   `PartialFailure`——否则前端会提示「请核对工具实际状态」，而实际什么都没落地。
/// - **回滚失败**：数据库/工具配置留在半成品状态，用户必须去核对，才叫
///   `PartialFailure`。这与 `error.rs` 里「回滚失败才算部分生效」的判定一致。
fn import_after_apply_failure(
    what_failed: &str,
    error: anyhow::Error,
    rollback: Result<(), String>,
) -> AppError {
    match rollback {
        Ok(()) => AppError::from(error)
            .with_context(format!("导入失败（{what_failed}出错），已回滚到导入前状态")),
        Err(rollback_error) => AppError::partial_failure(format!(
            "导入已部分生效（{what_failed}出错），回滚也失败，请核对工具当前配置"
        ))
        .with_detail(format!("{error:#}；回滚失败：{rollback_error}")),
    }
}

fn replace_database_locked(
    input_path: &std::path::Path,
    db_path: &std::path::Path,
    db: &mut Database,
) -> Result<Option<std::path::PathBuf>, AppError> {
    let placeholder = Database::open(":memory:")
        .map_err(|e| AppError::from(e).with_context("导入前创建数据库占位连接失败"))?;
    let previous = std::mem::replace(db, placeholder);
    drop(previous);

    let backup = match Database::replace_file_from_import(input_path, db_path) {
        Ok(backup) => backup,
        Err(error) => {
            *db = Database::open(db_path).map_err(|restore| {
                AppError::partial_failure("导入失败，且无法重新打开原数据库")
                    .with_detail(format!("{error:#}；重新打开失败：{restore:#}"))
            })?;
            return Err(AppError::from(error).with_context("导入数据库文件失败"));
        }
    };
    match Database::open(db_path) {
        Ok(reloaded) => {
            *db = reloaded;
            if let Err(error) = switch_api::adapters::journal::recover_interrupted_switch(db) {
                eprintln!("[Helio] recover after import failed: {error:#}");
            }
            Ok(backup)
        }
        Err(error) => {
            if let Some(backup_path) = backup.as_ref() {
                if let Err(restore) = Database::restore_replaced_file(db_path, backup_path) {
                    return Err(AppError::partial_failure(
                        "导入后无法重新打开数据库，且回滚失败，请核对数据库文件",
                    )
                    .with_detail(format!("{error:#}；回滚失败：{restore:#}")));
                }
                *db = Database::open(db_path).map_err(|restore| {
                    AppError::partial_failure("导入失败，已回滚但无法重新打开数据库").with_detail(
                        format!("{error:#}；重新打开回滚后的数据库失败：{restore:#}"),
                    )
                })?;
            }
            Err(AppError::from(error).with_context("导入后重新打开数据库失败"))
        }
    }
}

fn rollback_import_state(
    db_path: &std::path::Path,
    db: &mut Database,
    backup_path: Option<&std::path::Path>,
    snapshots: &[switch_api::adapters::FileSnapshot],
    skills: Option<(
        &std::path::Path,
        &switch_api::utils::skills_backup::SkillsImportResult,
    )>,
) -> Result<(), String> {
    let mut errors = Vec::new();

    if let Err(error) = switch_api::adapters::restore_snapshots(snapshots) {
        errors.push(format!("恢复工具配置失败: {error:#}"));
    }
    if let Some((home, result)) = skills {
        if let Err(error) = switch_api::utils::skills_backup::remove_restored_skills(home, result) {
            errors.push(format!("恢复 Skills 失败: {error:#}"));
        }
    }

    let placeholder =
        Database::open(":memory:").map_err(|error| format!("创建数据库占位连接失败: {error}"))?;
    let previous = std::mem::replace(db, placeholder);
    drop(previous);

    match backup_path {
        Some(backup_path) => {
            if let Err(error) = Database::restore_replaced_file(db_path, backup_path) {
                errors.push(format!("恢复数据库文件失败: {error:#}"));
            }
        }
        None => {
            if let Err(error) = std::fs::remove_file(db_path) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    errors.push(format!("删除导入失败的数据库失败: {error}"));
                }
            }
        }
    }
    let _ = std::fs::remove_file(format!("{}-wal", db_path.display()));
    let _ = std::fs::remove_file(format!("{}-shm", db_path.display()));

    match Database::open(db_path) {
        Ok(restored) => *db = restored,
        Err(error) => errors.push(format!("重新打开回滚后的数据库失败: {error:#}")),
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("；"))
    }
}

/// 读取某工具的活跃 Profile；失败时补上「是哪个工具」的上下文。
///
/// `get_status` / `probe_active_profiles` 都要对全部工具做同样的事，逐处写
/// `map_err` 会得到 7 段除了工具名完全相同的代码，且报错时用户看不出是哪个工具。
fn read_active_profile(db: &Database, target: TargetApp) -> Result<Option<ApiProfile>, AppError> {
    db.get_active_profile_full(target)
        .map_err(|e| AppError::from(e).with_context(format!("读取 {} 状态失败", target.as_str())))
}

#[tauri::command]
pub async fn get_status(state: State<'_, AppState>) -> Result<StatusInfo, AppError> {
    let db = state.db.lock()?;

    // Claude Code status
    let claude_code_profile = read_active_profile(&db, TargetApp::ClaudeCode)?;
    // connected = 已配置活跃 profile（不发外网探活）
    let claude_code = Some(TargetStatus {
        connected: claude_code_profile.is_some(),
        profile: claude_code_profile,
        ..Default::default()
    });

    // Codex status
    let codex_profile = read_active_profile(&db, TargetApp::Codex)?;
    let codex = Some(TargetStatus {
        connected: codex_profile.is_some(),
        profile: codex_profile,
        ..Default::default()
    });

    // Pi status
    let pi_profile = read_active_profile(&db, TargetApp::Pi)?;
    let pi = Some(TargetStatus {
        connected: pi_profile.is_some(),
        profile: pi_profile,
        ..Default::default()
    });

    // OpenCode status
    let opencode_profile = read_active_profile(&db, TargetApp::OpenCode)?;
    let opencode = Some(TargetStatus {
        connected: opencode_profile.is_some(),
        profile: opencode_profile,
        ..Default::default()
    });

    // Hermes status
    let hermes_profile = read_active_profile(&db, TargetApp::Hermes)?;
    let hermes = Some(TargetStatus {
        connected: hermes_profile.is_some(),
        profile: hermes_profile,
        ..Default::default()
    });

    // OpenClaw status
    let openclaw_profile = read_active_profile(&db, TargetApp::OpenClaw)?;
    let openclaw = Some(TargetStatus {
        connected: openclaw_profile.is_some(),
        profile: openclaw_profile,
        ..Default::default()
    });

    // ZCode status
    let zcode_profile = read_active_profile(&db, TargetApp::ZCode)?;
    let zcode = Some(TargetStatus {
        connected: zcode_profile.is_some(),
        profile: zcode_profile,
        ..Default::default()
    });

    // Database info
    let profiles = db
        .list_profiles()
        .map_err(|e| AppError::from(e).with_context("读取 Profile 列表失败"))?;
    let db_path = default_db_path()?;
    let size = std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);

    Ok(StatusInfo {
        claude_code,
        codex,
        pi,
        opencode,
        hermes,
        openclaw,
        zcode,
        database: DatabaseInfo {
            size,
            profile_count: profiles.len(),
            path: db_path.to_string_lossy().to_string(),
        },
    })
}

fn profile_protocol_fields(profile: &ApiProfile) -> Option<String> {
    match profile.target_app {
        Some(TargetApp::OpenCode) => profile.opencode.opencode_api_mode.clone(),
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
) -> Result<crate::model_fetch::FailoverResult, AppError> {
    use crate::model_fetch::{probe_with_params, FailoverResult, KeyProbeResult};

    let (mut profile, was_active) = {
        let db = state.db.lock()?;
        let mut p = db
            .get_profile_by_name_and_target(profile_name, target)
            .map_err(|e| {
                AppError::from(e).with_context(format!(
                    "读取 {} 的 Profile「{profile_name}」失败",
                    target.as_str()
                ))
            })?;
        p.normalize_keys();
        let active_id = db
            .get_active_profile(target)
            .map_err(|e| AppError::from(e).with_context("读取当前活跃 Profile 失败"))?
            .map(|a| a.profile_id);
        let was = p.id.zip(active_id).map(|(a, b)| a == b).unwrap_or(false);
        (p, was)
    };

    if target == TargetApp::Codex
        && switch_api::adapters::codex::CodexAdapter::is_amazon_bedrock_profile(&profile)
    {
        return Err(AppError::invalid_input(
            "Amazon Bedrock 使用 Codex 内置 AWS 认证，不能通过 HTTP API Key 探活或 failover",
        ));
    }

    let model = model_for_probe(&profile);
    if model.is_empty() {
        return Err(AppError::invalid_input(
            "先为该 Profile 填写默认模型再 failover",
        ));
    }
    let wire_api = if target == TargetApp::Codex {
        profile.codex.wire_api.clone()
    } else {
        None
    };
    let mode = profile_protocol_fields(&profile);
    let experimental_bearer_token = if target == TargetApp::Codex {
        profile.codex.experimental_bearer_token.clone()
    } else {
        None
    };
    let app_str = target.as_str().to_string();

    let mut keys = profile.api_keys.clone().unwrap_or_default();
    if keys.is_empty() && !profile.api_key.trim().is_empty() {
        profile.normalize_keys();
        keys = profile.api_keys.clone().unwrap_or_default();
    }
    if keys.is_empty() {
        return Err(AppError::invalid_input("没有可 failover 的 Key"));
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
            let wire_api = wire_api.clone();
            let mode = mode.clone();
            let experimental_bearer_token = experimental_bearer_token.clone();
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
                            wire_api: wire_api.as_deref(),
                            api_mode: mode.as_deref(),
                            experimental_bearer_token: experimental_bearer_token.as_deref(),
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
                if winner.is_none() {
                    // 保持 active 优先、列表顺序优先的选择策略，但继续收集
                    // 其他并发探测结果，避免成功后丢失已完成 Key 的健康记录。
                    winner = Some((id.clone(), label.clone()));
                }
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
        let db = state.db.lock()?;
        db.update_profile(&profile)?;
        let should_switch = re_switch || (was_active && success);
        if should_switch && success {
            Some(
                db.get_shared_config(target)
                    .map_err(|e| AppError::from(e).with_context("读取共享配置失败"))?,
            )
        } else {
            None
        }
    };

    let mut re_switched = false;
    if let Some(persisted_shared_config) = persisted_shared_config {
        // 全局写锁：与其他切换/写盘命令互斥
        let _write_guard = state.config_lock.lock()?;
        let shared_config =
            switch_api::adapters::resolve_shared_config(target, persisted_shared_config)
                .map_err(|e| AppError::from(e).with_context("解析共享配置失败"))?;
        let db = state.db.lock()?;
        switch_api::adapters::apply_profile_switch(&db, target, &profile, &shared_config, true)
            .map_err(|e| AppError::from(e).with_context("切换配置失败"))?;
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
) -> Result<crate::model_fetch::FailoverResult, AppError> {
    let target = TargetApp::parse(&target_app).ok_or_else(|| unknown_target_app(&target_app))?;
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
    /// 托管 Provider（例如 Codex 内置 Bedrock）没有可探测的 URL。
    #[serde(default)]
    pub managed: bool,
    pub probed_at: i64,
}

/// 状态页「检测可用性」：对齐 CC Switch stream_check。
/// 对每个已配置工具的 `api_url` 做 GET 可达性探测（不发模型请求、不验 key）。
#[tauri::command]
pub async fn probe_active_profiles(
    state: State<'_, AppState>,
) -> Result<Vec<ToolProbeResult>, AppError> {
    use crate::model_fetch::{probe_reachability, ReachabilityConfig};

    let snapshots: Vec<(TargetApp, Option<ApiProfile>)> = {
        let db = state.db.lock()?;
        let tools = [
            TargetApp::ClaudeCode,
            TargetApp::Codex,
            TargetApp::Pi,
            TargetApp::OpenCode,
            TargetApp::Hermes,
            TargetApp::OpenClaw,
            TargetApp::ZCode,
        ];
        let mut out = Vec::new();
        for t in tools {
            out.push((t, read_active_profile(&db, t)?));
        }
        out
    };

    let cfg = ReachabilityConfig::default();
    // 并发探测全部已注册工具（每个可能超时 8s+），串行最坏随工具数线性增长 → 并行一次超时
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
                        managed: false,
                        probed_at: chrono::Utc::now().timestamp(),
                    };
                };
                let url = profile.api_url.trim();
                if url.is_empty() {
                    if target == TargetApp::Codex
                        && switch_api::adapters::codex::CodexAdapter::is_amazon_bedrock_profile(
                            &profile,
                        )
                    {
                        return ToolProbeResult {
                            target_app: app,
                            configured: true,
                            ok: true,
                            status: Some("managed".into()),
                            profile_name: Some(profile.name),
                            error: None,
                            protocol: Some("managed".into()),
                            endpoint: None,
                            latency_ms: None,
                            http_status: None,
                            managed: true,
                            probed_at: chrono::Utc::now().timestamp(),
                        };
                    }
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
                        managed: false,
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
                    managed: false,
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

fn default_db_path() -> Result<std::path::PathBuf, AppError> {
    Ok(home_dir()?.join(".switch-api").join("db.sqlite"))
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

    /// 无头 Linux（如 CI）既无 wl-copy 也无 xclip，硬断言只会让 CI 常红；
    /// 后端缺失时跳过，有后端的开发机照常真测。macOS/Windows 后端随系统自带，不跳过。
    #[cfg(all(unix, not(target_os = "macos")))]
    fn backend_in_path(path_var: Option<std::ffi::OsString>) -> bool {
        let Some(path_var) = path_var else {
            return false;
        };
        std::env::split_paths(&path_var)
            .filter(|dir| !dir.as_os_str().is_empty())
            .any(|dir| dir.join("wl-copy").is_file() || dir.join("xclip").is_file())
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    fn backend_available() -> bool {
        backend_in_path(std::env::var_os("PATH"))
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    macro_rules! require_backend {
        () => {
            if !backend_available() {
                println!("SKIP: no wl-copy/xclip on PATH (headless environment)");
                return;
            }
        };
    }

    #[test]
    fn test_copy_text_native_accepts_empty_text() {
        #[cfg(all(unix, not(target_os = "macos")))]
        require_backend!();
        // 空串在各平台后端都应可接受（不崩、不拒）。
        copy_text_native("").expect("empty clipboard text should copy");
    }

    #[test]
    fn test_copy_text_native_accepts_unicode() {
        #[cfg(all(unix, not(target_os = "macos")))]
        require_backend!();
        copy_text_native("Helio 剪贴板 ✓")
            .expect("unicode clipboard text should copy on this platform");
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn test_backend_detection_matches_tool_presence() {
        assert!(!backend_in_path(None));
        let dir = tempfile::tempdir().unwrap();
        assert!(!backend_in_path(Some(dir.path().as_os_str().to_owned())));
        std::fs::write(dir.path().join("wl-copy"), "fake").unwrap();
        assert!(backend_in_path(Some(dir.path().as_os_str().to_owned())));
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

/// `update_codex_fields` 的字段编辑语义基线。
///
/// 这些断言原先针对 `apply_field_updates`（在 JSON 值上改字段，再由调用方全量
/// 重新序列化）。改为保真路径后语义不变，但**额外**保证：注释、键序、空行与
/// 未受管子表原样存活——旧实现会把它们全部洗掉。
#[cfg(test)]
mod codex_field_update_tests {
    use serde_json::json;
    use switch_api::doc::toml::apply_top_level_updates;

    /// 把 JSON 对象转成 `apply_top_level_updates` 需要的 Map。
    fn updates(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        value.as_object().expect("updates 必须是对象").clone()
    }

    #[test]
    fn test_set_new_field() {
        let live = "model_provider = \"openai\"\n";
        let result =
            apply_top_level_updates(live, &updates(json!({ "approval_policy": "never" }))).unwrap();
        assert!(result.contains("approval_policy = \"never\""), "{result}");
        // 原有字段不受影响
        assert!(result.contains("model_provider = \"openai\""), "{result}");
    }

    #[test]
    fn test_override_existing_field() {
        let live = "sandbox_mode = \"read-only\"\n";
        let result =
            apply_top_level_updates(live, &updates(json!({ "sandbox_mode": "workspace-write" })))
                .unwrap();
        assert!(
            result.contains("sandbox_mode = \"workspace-write\""),
            "{result}"
        );
        assert!(!result.contains("read-only"), "{result}");
    }

    #[test]
    fn test_null_removes_field() {
        let live = "service_tier = \"fast\"\nmodel_provider = \"openai\"\n";
        let result =
            apply_top_level_updates(live, &updates(json!({ "service_tier": null }))).unwrap();
        assert!(
            !result.contains("service_tier"),
            "null 应删除该键:\n{result}"
        );
        // 其他字段保留
        assert!(result.contains("model_provider = \"openai\""), "{result}");
    }

    #[test]
    fn test_does_not_touch_other_fields() {
        let live = "\
model_provider = \"openai\"
approval_policy = \"on-request\"

[model_providers.openai]
base_url = \"https://api.com\"

[mcp_servers.fs]
command = \"npx\"
";
        let result = apply_top_level_updates(
            live,
            &updates(json!({
                "approval_policy": "untrusted",
                "model_auto_compact_token_limit": 200000,
                "disable_response_storage": true,
            })),
        )
        .unwrap();

        // 改了/加了指定字段
        assert!(
            result.contains("approval_policy = \"untrusted\""),
            "{result}"
        );
        assert!(
            result.contains("model_auto_compact_token_limit = 200000"),
            "{result}"
        );
        assert!(
            result.contains("disable_response_storage = true"),
            "{result}"
        );
        // 完整保留嵌套结构
        assert!(result.contains("[model_providers.openai]"), "{result}");
        assert!(
            result.contains("base_url = \"https://api.com\""),
            "{result}"
        );
        assert!(result.contains("[mcp_servers.fs]"), "{result}");
        assert!(result.contains("command = \"npx\""), "{result}");
        assert!(result.contains("model_provider = \"openai\""), "{result}");
    }

    #[test]
    fn test_mixed_set_and_remove() {
        let live = "personality = \"friendly\"\nenable_workflows = true\n";
        let result = apply_top_level_updates(
            live,
            &updates(json!({
                "personality": null,
                "model_reasoning_effort": "high",
                "enable_workflows": false,
            })),
        )
        .unwrap();

        assert!(!result.contains("personality"), "{result}");
        assert!(
            result.contains("model_reasoning_effort = \"high\""),
            "{result}"
        );
        assert!(result.contains("enable_workflows = false"), "{result}");
    }

    /// 回归：旧实现（JSON 往返 + 全量序列化）会洗掉注释与键序，新实现必须保住。
    #[test]
    fn test_preserves_comments_and_key_order() {
        let live = "\
# 我的 Codex 配置
model = \"gpt-5\"     # 行尾注释
approval_policy = \"never\"

# 下面是中转配置
[model_providers.custom]
base_url = \"https://x.example\"
";
        let result =
            apply_top_level_updates(live, &updates(json!({ "approval_policy": "on-request" })))
                .unwrap();

        assert!(
            result.contains("# 我的 Codex 配置"),
            "顶层注释应保留:\n{result}"
        );
        assert!(result.contains("# 行尾注释"), "行尾注释应保留:\n{result}");
        assert!(
            result.contains("# 下面是中转配置"),
            "子表前注释应保留:\n{result}"
        );
        assert!(
            result.contains("approval_policy = \"on-request\""),
            "{result}"
        );

        let model_pos = result.find("model =").expect("model 应存在");
        let policy_pos = result
            .find("approval_policy")
            .expect("approval_policy 应存在");
        assert!(model_pos < policy_pos, "键序应保留:\n{result}");
    }
}

/// 「从本地导入」扫描逻辑的行为基线。
///
/// `scan_api_from_config` 是 7 个工具解析逻辑的总入口，重构（拆分巨型函数）
/// 期间它必须保持**逐字段等价**。这里为每个工具构造一份真实形状的配置，
/// 把当前行为逐字段钉死；重构后本模块的断言不得有任何改动。
///
/// `home` 由参数注入（临时目录），因此不触碰进程级 `$HOME`，可并行执行。
#[cfg(test)]
mod scan_api_baseline_tests {
    use super::{scan_api_from_config, ScannedApi};
    use serde_json::json;
    use std::path::Path;
    use switch_api::models::TargetApp;

    /// 在临时 home 下写一个辅助文件（auth.json / models.json 等）。
    fn write_home_file(home: &Path, rel: &str, content: &str) {
        let path = home.join(rel);
        std::fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
        std::fs::write(&path, content).expect("write fixture");
    }

    fn scan(target: TargetApp, cfg: serde_json::Value, home: &Path) -> ScannedApi {
        scan_api_from_config(target, &cfg, "fixture.json".to_string(), Some(home))
    }

    fn empty_home() -> tempfile::TempDir {
        tempfile::tempdir().expect("temp home")
    }

    #[test]
    fn claude_code_reads_env_url_key_and_role_mapping() {
        let home = empty_home();
        let cfg = json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://claude.example/v1",
                "ANTHROPIC_AUTH_TOKEN": "sk-claude-token",
                "ANTHROPIC_API_KEY": "sk-should-be-ignored",
                "ANTHROPIC_MODEL": "claude-sonnet-4",
                "ANTHROPIC_DEFAULT_OPUS_MODEL": "claude-opus-4[1M]",
                "ANTHROPIC_DEFAULT_OPUS_MODEL_NAME": "Opus",
                "ANTHROPIC_DEFAULT_SONNET_MODEL": "claude-sonnet-4",
            }
        });
        let s = scan(TargetApp::ClaudeCode, cfg, home.path());

        assert!(s.found);
        assert_eq!(s.api_url, "https://claude.example/v1");
        // AUTH_TOKEN 优先于 API_KEY
        assert_eq!(s.api_key, "sk-claude-token");
        assert_eq!(s.provider, "anthropic");
        assert_eq!(s.model.as_deref(), Some("claude-sonnet-4"));
        // [1M] 后缀剥离 + one_m 标记
        let m = s.model_mapping.expect("mapping");
        assert_eq!(
            m.get("opus_model").map(String::as_str),
            Some("claude-opus-4")
        );
        assert_eq!(m.get("opus_one_m").map(String::as_str), Some("true"));
        assert_eq!(m.get("opus_name").map(String::as_str), Some("Opus"));
        assert_eq!(
            m.get("sonnet_model").map(String::as_str),
            Some("claude-sonnet-4")
        );
        assert!(!m.contains_key("haiku_model"));
        assert_eq!(s.source, "fixture.json");
        // ClaudeCode 不参与 Codex 的 context_1m 推断
        assert_eq!(s.context_1m, None);
        assert_eq!(s.wire_api, None);
        assert_eq!(s.api_mode, None);
    }

    #[test]
    fn empty_config_is_not_found() {
        let home = empty_home();
        let s = scan(TargetApp::ClaudeCode, json!({}), home.path());
        assert!(!s.found);
        assert_eq!(s.api_url, "");
        assert_eq!(s.api_key, "");
        assert_eq!(s.provider, "anthropic");
        assert_eq!(s.model, None);
        assert_eq!(s.model_mapping, None);
    }

    #[test]
    fn codex_reads_provider_block_and_normalizes_removed_chat_wire_api() {
        const ENV_KEY: &str = "HELIO_SCAN_TEST_CODEX_ENV_KEY";
        let home = empty_home();
        // auth.json 存在，但 env_key 已提供 key，故不应被读取
        write_home_file(
            home.path(),
            ".codex/auth.json",
            r#"{"OPENAI_API_KEY":"sk-codex-auth-should-not-win"}"#,
        );
        std::env::set_var(ENV_KEY, "sk-codex-from-env");

        let cfg = json!({
            "model_provider": "myprov",
            "model": "gpt-5-codex",
            "model_reasoning_effort": "high",
            "model_reasoning_summary": "auto",
            "model_verbosity": "medium",
            "model_context_window": 1_000_000,
            "service_tier": "flex",
            "model_providers": {
                "myprov": {
                    "base_url": "https://codex.example/v1",
                    "wire_api": "chat",
                    "requires_openai_auth": true,
                    "supports_standalone_web_search": true,
                    "experimental_bearer_token": "bearer-xyz",
                    "env_key": ENV_KEY,
                    "auth": {
                        "command": "my-auth",
                        "args": ["--flag", "   ", "val"],
                        "timeout_ms": 5000,
                        "refresh_interval_ms": 0,
                        "cwd": " /tmp ",
                    }
                }
            }
        });
        let s = scan(TargetApp::Codex, cfg, home.path());

        assert!(s.found);
        assert_eq!(s.api_url, "https://codex.example/v1");
        assert_eq!(s.api_key, "sk-codex-from-env");
        assert_eq!(s.provider, "myprov");
        assert_eq!(s.model.as_deref(), Some("gpt-5-codex"));
        assert_eq!(s.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(s.reasoning_summary.as_deref(), Some("auto"));
        assert_eq!(s.verbosity.as_deref(), Some("medium"));
        assert_eq!(s.service_tier.as_deref(), Some("flex"));
        assert_eq!(s.context_1m, Some(true));
        // 已下线的 chat 归一为 responses
        assert_eq!(s.wire_api.as_deref(), Some("responses"));
        assert_eq!(s.env_key.as_deref(), Some(ENV_KEY));
        assert_eq!(s.requires_openai_auth, Some(true));
        assert_eq!(s.supports_standalone_web_search, Some(true));
        assert_eq!(s.experimental_bearer_token.as_deref(), Some("bearer-xyz"));
        // args 去空白 + 丢空项；timeout 保留，0 值丢弃；cwd 去首尾空白
        assert_eq!(
            s.auth_args.as_deref(),
            Some(["--flag".to_string(), "val".to_string()].as_slice())
        );
        assert_eq!(s.auth_command.as_deref(), Some("my-auth"));
        assert_eq!(s.auth_timeout_ms, Some(5000));
        assert_eq!(s.auth_refresh_interval_ms, None);
        assert_eq!(s.auth_cwd.as_deref(), Some("/tmp"));

        std::env::remove_var(ENV_KEY);
    }

    #[test]
    fn codex_falls_back_to_auth_json_when_env_key_absent() {
        let home = empty_home();
        write_home_file(
            home.path(),
            ".codex/auth.json",
            r#"{"api_key":"sk-codex-from-auth-json"}"#,
        );
        let cfg = json!({
            "model_provider": "myprov",
            "model_providers": {"myprov": {"base_url": "https://codex.example/v1"}}
        });
        let s = scan(TargetApp::Codex, cfg, home.path());
        assert_eq!(s.api_key, "sk-codex-from-auth-json");
        assert_eq!(s.provider, "myprov");
        // 无 model_context_window → 无法判定
        assert_eq!(s.context_1m, None);
        assert_eq!(s.wire_api, None);
    }

    #[test]
    fn codex_amazon_bedrock_reads_aws_block_and_counts_as_found() {
        let home = empty_home();
        let cfg = json!({
            "model_provider": "amazon-bedrock",
            "model_providers": {
                "amazon-bedrock": {
                    "aws": {"profile": "work", "region": "us-west-2"},
                    "base_url": "https://should-be-ignored",
                }
            }
        });
        let s = scan(TargetApp::Codex, cfg, home.path());
        // url/key 皆空，但 bedrock 分支仍算「扫到」
        assert!(s.found);
        assert_eq!(s.api_url, "");
        assert_eq!(s.api_key, "");
        assert_eq!(s.provider, "amazon-bedrock");
        assert_eq!(s.aws_profile.as_deref(), Some("work"));
        assert_eq!(s.aws_region.as_deref(), Some("us-west-2"));
    }

    #[test]
    fn pi_reads_models_then_auth_json_overrides_key() {
        let home = empty_home();
        write_home_file(
            home.path(),
            ".pi/agent/models.json",
            r#"{"providers":{"myprov":{"baseUrl":"https://pi.example/v1","apiKey":"sk-pi-models"}}}"#,
        );
        write_home_file(
            home.path(),
            ".pi/agent/auth.json",
            r#"{"myprov":{"key":"sk-pi-auth"}}"#,
        );
        let cfg = json!({"defaultProvider": "myprov", "defaultModel": "pi-model"});
        let s = scan(TargetApp::Pi, cfg, home.path());

        assert!(s.found);
        assert_eq!(s.api_url, "https://pi.example/v1");
        // auth.json 的 key 覆盖 models.json 的 apiKey
        assert_eq!(s.api_key, "sk-pi-auth");
        assert_eq!(s.provider, "myprov");
        assert_eq!(s.model.as_deref(), Some("pi-model"));
        // Pi 借用 api_mode 传 provider，最终必须清空
        assert_eq!(s.api_mode, None);
        assert_eq!(s.context_1m, None);
    }

    #[test]
    fn opencode_uses_configured_provider_and_reads_auth_json_for_env_placeholder() {
        let home = empty_home();
        write_home_file(
            home.path(),
            ".local/share/opencode/auth.json",
            r#"{"myprov":{"key":"sk-oc-auth"}}"#,
        );
        let cfg = json!({
            "model": "myprov/big-model",
            "provider": {
                "myprov": {
                    "npm": "@ai-sdk/openai",
                    "options": {"baseURL": "https://oc.example/v1", "apiKey": "{env:MY_KEY}"},
                    "models": {
                        "zeta": {},
                        "alpha": {"limit": {"context": 200000}},
                        "beta": 5
                    }
                }
            }
        });
        let s = scan(TargetApp::OpenCode, cfg, home.path());

        assert!(s.found);
        assert_eq!(s.api_url, "https://oc.example/v1");
        // {env:...} 占位符必须换成 auth.json 里的真实 key
        assert_eq!(s.api_key, "sk-oc-auth");
        // OpenCode 不做 provider 还原，保持默认
        assert_eq!(s.provider, "anthropic");
        assert_eq!(s.opencode_api_mode.as_deref(), Some("responses"));
        assert_eq!(
            s.opencode_models.as_deref(),
            Some(["alpha".to_string(), "beta".to_string(), "zeta".to_string()].as_slice())
        );
        // 仅对象形态的 model 配置被收集（beta: 5 被丢弃）
        let cfgs = s.opencode_model_configs.expect("model configs");
        assert_eq!(cfgs.len(), 2);
        assert!(cfgs.contains_key("alpha"));
        assert!(cfgs.contains_key("zeta"));
        assert!(!cfgs.contains_key("beta"));
    }

    #[test]
    fn opencode_without_top_level_model_does_not_guess_a_provider() {
        let home = empty_home();
        let cfg = json!({
            "provider": {
                "first": {"options": {"baseURL": "https://first", "apiKey": "sk-first"}},
                "second": {"options": {"baseURL": "https://second", "apiKey": "sk-second"}}
            }
        });
        let s = scan(TargetApp::OpenCode, cfg, home.path());
        // 没有 model/small_model 指向 → 不取任意 provider
        assert!(!s.found);
        assert_eq!(s.api_url, "");
        assert_eq!(s.api_key, "");
        assert_eq!(s.opencode_models, None);
    }

    #[test]
    fn hermes_matches_custom_provider_by_normalized_name() {
        let home = empty_home();
        let cfg = json!({
            "model": {
                "provider": "custom:My-Name",
                "default": "hermes-model",
                "api_mode": "anthropic_messages",
                "context_length": 1_000_000,
            },
            "custom_providers": [
                {"name": "Other", "base_url": "https://nope", "api_key": "sk-nope"},
                {
                    "name": "My Name",
                    "base_url": "https://hermes.example/v1",
                    "api_key": "sk-hermes",
                    "api_mode": "chat_completions",
                }
            ]
        });
        let s = scan(TargetApp::Hermes, cfg, home.path());

        assert!(s.found);
        assert_eq!(s.api_url, "https://hermes.example/v1");
        assert_eq!(s.api_key, "sk-hermes");
        // custom_providers 的 api_mode 优先于 model.api_mode
        assert_eq!(s.api_mode.as_deref(), Some("chat_completions"));
        // provider 还原只去 custom: 前缀，不做小写归一
        assert_eq!(s.provider, "My-Name");
        assert_eq!(s.model.as_deref(), Some("hermes-model"));
        assert_eq!(s.context_1m, Some(true));
    }

    #[test]
    fn openclaw_reads_primary_model_and_maps_api_mode() {
        let home = empty_home();
        let cfg = json!({
            "agents": {
                "defaults": {
                    "model": {"primary": "myprov/big-model"},
                    "contextTokens": 200000,
                }
            },
            "models": {
                "providers": {
                    "myprov": {
                        "baseUrl": "https://claw.example/v1",
                        "apiKey": "sk-claw",
                        "api": "openai-completions",
                        "models": [
                            {"id": "big-model", "contextWindow": 1_000_000, "maxTokens": 64000}
                        ],
                    }
                }
            }
        });
        let s = scan(TargetApp::OpenClaw, cfg, home.path());

        assert!(s.found);
        assert_eq!(s.api_url, "https://claw.example/v1");
        assert_eq!(s.api_key, "sk-claw");
        assert_eq!(s.provider, "myprov");
        assert_eq!(s.model.as_deref(), Some("big-model"));
        // openai-completions → chat_completions
        assert_eq!(s.api_mode.as_deref(), Some("chat_completions"));
        assert_eq!(s.context_1m, Some(true));
        assert_eq!(s.max_tokens, Some(64000));
    }

    #[test]
    fn openclaw_falls_back_to_agents_context_tokens() {
        let home = empty_home();
        let cfg = json!({
            "agents": {
                "defaults": {
                    "model": {"primary": "myprov/small"},
                    "contextTokens": 200000,
                }
            },
            "models": {
                "providers": {
                    "myprov": {
                        "baseUrl": "https://claw.example/v1",
                        "apiKey": "sk-claw",
                        "models": [{"id": "small"}],
                    }
                }
            }
        });
        let s = scan(TargetApp::OpenClaw, cfg, home.path());
        assert_eq!(s.context_1m, Some(false));
        assert_eq!(s.max_tokens, None);
        assert_eq!(s.api_mode, None);
    }

    #[test]
    fn zcode_prefers_top_level_model_provider() {
        let home = empty_home();
        let cfg = json!({
            "model": "myprov/big-model",
            "provider": {
                "other": {
                    "source": "custom",
                    "kind": "openai",
                    "options": {"baseURL": "https://nope", "apiKey": "sk-nope"},
                },
                "myprov": {
                    "source": "custom",
                    "kind": "anthropic",
                    "options": {"baseURL": "https://zcode.example/v1", "apiKey": "sk-zcode"},
                    "models": {"big-model": {"limit": {"context": 1_000_000}}},
                },
            }
        });
        let s = scan(TargetApp::ZCode, cfg, home.path());

        assert!(s.found);
        assert_eq!(s.api_url, "https://zcode.example/v1");
        assert_eq!(s.api_key, "sk-zcode");
        assert_eq!(s.provider, "myprov");
        assert_eq!(s.model.as_deref(), Some("big-model"));
        assert_eq!(s.context_1m, Some(true));
    }

    #[test]
    fn zcode_without_model_prefers_custom_anthropic_provider() {
        let home = empty_home();
        let cfg = json!({
            "provider": {
                "a": {
                    "source": "builtin",
                    "kind": "openai",
                    "options": {"baseURL": "https://a", "apiKey": "sk-a"},
                },
                "b": {
                    "source": "custom",
                    "kind": "anthropic",
                    "options": {"baseURL": "https://b", "apiKey": "sk-b"},
                },
            }
        });
        let s = scan(TargetApp::ZCode, cfg, home.path());

        assert!(s.found);
        assert_eq!(s.api_url, "https://b");
        assert_eq!(s.api_key, "sk-b");
        assert_eq!(s.provider, "b");
        // 无 model 指向且该 provider 未声明 models → 无模型信息
        assert_eq!(s.model, None);
        assert_eq!(s.context_1m, None);
    }

    #[test]
    fn every_tool_returns_the_source_it_was_given() {
        let home = empty_home();
        for target in [
            TargetApp::ClaudeCode,
            TargetApp::Codex,
            TargetApp::Pi,
            TargetApp::OpenCode,
            TargetApp::Hermes,
            TargetApp::OpenClaw,
            TargetApp::ZCode,
        ] {
            let s = scan_api_from_config(
                target,
                &json!({}),
                "fixture.json".to_string(),
                Some(home.path()),
            );
            assert_eq!(s.source, "fixture.json", "source 必须原样回传: {target:?}");
        }
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

/// 导入流程「已落地后失败」的归类。
///
/// `import_after_apply_failure` 是纯函数（不改磁盘、不碰数据库），
/// 所以两个分支都能直接测——这正是把它从 `import_database` /
/// `import_portable_backup` 里抽出来的原因：留在命令里就只能靠集成测试碰运气。
#[cfg(test)]
mod import_failure_tests {
    use super::import_after_apply_failure;
    use switch_api::error::ErrorKind;

    fn io_error() -> anyhow::Error {
        anyhow::Error::from(std::io::Error::from(std::io::ErrorKind::NotFound))
    }

    #[test]
    fn a_clean_rollback_keeps_the_underlying_kind_instead_of_partial_failure() {
        // 回滚成功 = 磁盘已复原，用户什么都不用做。此时报「部分生效」会误导用户
        // 去核对工具状态，而实际上什么都没落地。类别应沿用底层错误的真实类型。
        let err = import_after_apply_failure("恢复生效配置", io_error(), Ok(()));

        assert_eq!(
            err.kind(),
            ErrorKind::Io,
            "已回滚的失败要保留底层类别（io），不能被改写成 PartialFailure"
        );
        assert!(
            err.message.contains("已回滚到导入前状态"),
            "文案要告诉用户「没留下痕迹」，实际为：{}",
            err.message
        );
        assert!(
            err.message.contains("恢复生效配置"),
            "文案要说清是哪一步失败，实际为：{}",
            err.message
        );
    }

    #[test]
    fn a_failed_rollback_is_partial_failure_and_keeps_both_errors_in_detail() {
        let err = import_after_apply_failure(
            "恢复生效配置",
            io_error(),
            Err("恢复工具配置失败: 权限不足".to_string()),
        );

        assert_eq!(
            err.kind(),
            ErrorKind::PartialFailure,
            "回滚也失败才叫「部分生效」"
        );
        assert!(
            err.message.contains("请核对"),
            "部分生效必须给出可行动的下一步，实际为：{}",
            err.message
        );
        let detail = err.detail.as_deref().expect("detail 必须保留原始错误");
        assert!(
            detail.contains("恢复工具配置失败"),
            "detail 要带上回滚错误，实际为：{detail}"
        );
    }

    #[test]
    fn a_failed_rollback_does_not_claim_the_state_was_restored() {
        // 回归保护：两个分支曾共用同一句文案「Database imported but ...」，
        // 回滚失败时也在说「导入已生效」，用户无法判断该不该动手。
        let err = import_after_apply_failure("恢复 Skills", io_error(), Err("磁盘只读".into()));
        assert!(
            !err.message.contains("已回滚到导入前状态"),
            "回滚失败时绝不能声称已复原，实际为：{}",
            err.message
        );
    }
}

/// `replace_database_locked` 的真实文件级测试。
///
/// 这是本次迁移改动最激进的一段：占位连接 → 替换文件 → 重新打开 → 失败时还原。
/// 纯函数单测覆盖不到它，只跑「编译通过」也不算验证——必须拿真实 sqlite 文件
/// 跑一遍，才能确认换库和失败恢复都还对。
#[cfg(test)]
mod replace_database_tests {
    use super::*;
    use switch_api::error::ErrorKind;

    fn named_profile(name: &str) -> ApiProfile {
        ApiProfile {
            name: name.into(),
            provider: "custom".into(),
            api_url: "https://x.example".into(),
            api_key: "k".into(),
            target_app: Some(TargetApp::Codex),
            ..Default::default()
        }
    }

    /// 建一个含若干 profile 的库，返回其路径。
    fn make_db(path: &std::path::Path, names: &[&str]) -> std::path::PathBuf {
        let db = Database::open(path).expect("open");
        for name in names {
            db.add_profile(&named_profile(name)).expect("add_profile");
        }
        drop(db);
        path.to_path_buf()
    }

    fn profile_names(db: &Database) -> Vec<String> {
        db.list_profiles()
            .expect("list_profiles")
            .into_iter()
            .map(|p| p.name)
            .collect()
    }

    #[test]
    fn swapping_in_an_import_keeps_the_new_content_and_returns_a_usable_backup() {
        let dir = tempfile::tempdir().expect("tempdir");
        let live = dir.path().join("live.sqlite");
        let incoming = dir.path().join("incoming.sqlite");
        make_db(&live, &["old"]);
        make_db(&incoming, &["new-a", "new-b"]);

        let mut db = Database::open(&live).expect("open live");
        let backup = replace_database_locked(&incoming, &live, &mut db).expect("导入应成功");

        // 换进来的是候选库的内容
        let names = profile_names(&db);
        assert_eq!(names.len(), 2, "实际：{names:?}");
        assert!(names.contains(&"new-a".to_string()), "实际：{names:?}");

        // 原库被快照成备份，且文件真实存在、内容确为替换前的样子——
        // 只返回一个路径但文件是空的，回滚能力就是假的。
        let backup = backup.expect("原库存在时必须返回备份路径");
        assert!(
            backup.exists(),
            "备份文件必须真实存在：{}",
            backup.display()
        );
        let backup_db = Database::open(&backup).expect("备份库必须可打开");
        assert_eq!(profile_names(&backup_db), vec!["old".to_string()]);
    }

    #[test]
    fn a_missing_import_file_fails_cleanly_and_leaves_the_live_db_usable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let live = dir.path().join("live.sqlite");
        make_db(&live, &["old"]);

        let mut db = Database::open(&live).expect("open live");
        let err = replace_database_locked(&dir.path().join("nope.sqlite"), &live, &mut db)
            .expect_err("不存在的候选库必须失败");

        assert_ne!(
            err.kind(),
            ErrorKind::PartialFailure,
            "替换前就中止、且原库能重新打开，不算「部分生效」"
        );
        // 关键：db 句柄必须被重新打开回真实库，而不是停在 :memory: 占位连接上。
        // 停在占位连接上会让后续命令读到空库——这是最难在人工点测里发现的失败形态。
        assert_eq!(
            profile_names(&db),
            vec!["old".to_string()],
            "原库内容必须仍然可读"
        );
    }
}
