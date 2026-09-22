//! 状态查询与探活：`get_status` / `run_failover` / `probe_active_profiles`。
//!
//! 与其余命令层的区别：这里做的是**网络编排**——并发探测多个 Profile 的
//! 可达性、按结果决定是否故障转移。业务规则（探活协议、模型发现）在核心库
//! 的 `probe` / `model_fetch`，本模块只负责调度与状态组装。

use crate::commands::helpers::default_db_path;
use crate::commands::main_cmds::{DatabaseInfo, StatusInfo, TargetStatus};
use crate::commands::{unknown_target_app, AppError, AppState};
use serde::{Deserialize, Serialize};
use switch_api::db::Database;
use switch_api::models::{ApiProfile, TargetApp};
use tauri::State;

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
pub(crate) async fn run_failover(
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
