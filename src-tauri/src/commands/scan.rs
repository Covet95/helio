//! 本地配置扫描：从各工具的 live 配置文件里还原出 API 凭据。
//!
//! 七个工具各有一套「方言」——同样的 API URL/Key 在不同工具里落在不同路径、
//! 不同键名下。本模块把它们统一解析成 [`ScannedApi`]，供 GUI 导入为 Profile。
//!
//! ## 为什么独立成模块
//!
//! 这 650 行是**纯业务知识**（知道每个工具的配置长什么样），此前挤在命令层
//! 与剪贴板子进程、sqlite 换库、并发探活混在一起。它是命令层最大的知识泄漏。
//!
//! 依赖 `adapters` 的读取能力，但不写库、不改文件——只读 + 解析。

use crate::commands::helpers::{
    claude_extract_models, codex_context_1m, codex_string_field, default_provider, str_field,
};
use crate::commands::{unknown_target_app, AppError};
use serde::{Deserialize, Serialize};
use switch_api::models::TargetApp;

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
    let adapter =
        get_adapter(target).map_err(|e| AppError::from(e).with_context("定位配置目录失败"))?;
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

/// 「从本地导入」扫描逻辑的行为基线。
///
/// `scan_api_from_config` 是 7 个工具解析逻辑的总入口，重构（拆分巨型函数）
///
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
