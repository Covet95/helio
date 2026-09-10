use super::{backup, ConfigAdapter};
use crate::models::{
    is_removed_chat_wire_api, is_supported_wire_api, ApiProfile, CodexCatalogModel,
};
use crate::utils::secure_fs::{atomic_write_private, ensure_private_dir};
use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

/// 非 1M 时 catalog 条目默认上下文（与常见 Codex 内置条目对齐）
const CATALOG_CONTEXT_STANDARD: i64 = 272_000;
const CATALOG_CONTEXT_1M: i64 = 1_000_000;
const CODEX_REASONING_LEVELS: &[&str] = &["minimal", "low", "medium", "high", "xhigh"];
/// Catalog（model_catalog.json）侧允许的档位：顶层 5 档之外，
/// 官方新模型（如 gpt-5.6-sol）还在 catalog 里声明 none/max/ultra，
/// 这里透传用户显式声明，顶层 model_reasoning_effort 仍只认 5 档。
const CODEX_CATALOG_REASONING_LEVELS: &[&str] = &[
    "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
];
/// 官方 service_tier 取值：priority / flex，fast 为 legacy 别名（仍可用）。
const CODEX_SERVICE_TIERS: &[&str] = &["fast", "flex", "priority"];
const CODEX_REASONING_SUMMARIES: &[&str] = &["auto", "concise", "detailed", "none"];
const CODEX_VERBOSITY_LEVELS: &[&str] = &["low", "medium", "high"];

const FALLBACK_BASE_INSTRUCTIONS: &str = "You are Codex, a coding agent based on GPT-5. You and the user share one workspace, and your job is to collaborate with them until their goal is genuinely handled.";

pub struct CodexAdapter {
    config_dir: PathBuf,
}

impl CodexAdapter {
    pub fn new() -> Self {
        let home = dirs::home_dir().expect("Failed to get home directory");
        let config_dir = home.join(".codex");
        Self { config_dir }
    }

    fn config_file_path(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    fn auth_file_path(&self) -> PathBuf {
        self.config_dir.join("auth.json")
    }

    fn model_catalog_path(&self) -> PathBuf {
        self.config_dir.join("model_catalog.json")
    }

    pub fn is_amazon_bedrock_profile(api_profile: &ApiProfile) -> bool {
        api_profile
            .provider
            .trim()
            .eq_ignore_ascii_case("amazon-bedrock")
    }

    fn normalized_reasoning_levels(entry: &CodexCatalogModel) -> Vec<String> {
        // 显式声明按 catalog 八档过滤；legacy 布尔沿用经典 5 档（不虚增 max/ultra/none）。
        if let Some(declared) = entry.reasoning_levels.as_ref() {
            let mut seen = std::collections::HashSet::new();
            return declared
                .iter()
                .map(|level| level.trim().to_ascii_lowercase())
                .filter(|level| CODEX_CATALOG_REASONING_LEVELS.contains(&level.as_str()))
                .filter(|level| seen.insert(level.clone()))
                .collect();
        }
        if entry.supports_reasoning != Some(true) {
            return Vec::new();
        }
        CODEX_REASONING_LEVELS
            .iter()
            .map(|level| (*level).to_string())
            .collect()
    }

    /// 有效 catalog 列表：过滤空 slug、按首次出现去重，默认 model 不在列表时 prepend。
    fn effective_catalog_models(api_profile: &ApiProfile) -> Vec<CodexCatalogModel> {
        let mut out: Vec<CodexCatalogModel> = Vec::new();
        let mut seen = std::collections::HashSet::new();

        if let Some(list) = api_profile.codex.catalog_models.as_ref() {
            for entry in list {
                let slug = entry.slug.as_str();
                // 不强制改写 slug 内容；仅跳过纯空白
                if slug.trim().is_empty() {
                    continue;
                }
                if !seen.insert(slug.to_string()) {
                    continue;
                }
                let display_name = entry
                    .display_name
                    .as_ref()
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string());
                out.push(CodexCatalogModel {
                    slug: slug.to_string(),
                    display_name,
                    context_window: entry.context_window,
                    reasoning_levels: entry.reasoning_levels.clone(),
                    supports_reasoning: entry.supports_reasoning,
                    supports_images: entry.supports_images,
                    supports_tool_calls: entry.supports_tool_calls,
                    supports_web_search: entry.supports_web_search,
                });
            }
        }

        if let Some(model) = api_profile
            .model
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            if !seen.contains(model) {
                out.insert(
                    0,
                    CodexCatalogModel {
                        slug: model.to_string(),
                        display_name: None,
                        ..Default::default()
                    },
                );
            }
        }

        out
    }

    fn catalog_template_base_instructions(&self) -> String {
        let path = self.model_catalog_path();
        if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(s) = v
                        .get("models")
                        .and_then(|m| m.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|m| m.get("base_instructions"))
                        .and_then(|b| b.as_str())
                        .filter(|s| !s.is_empty())
                    {
                        return s.to_string();
                    }
                }
            }
        }
        FALLBACK_BASE_INSTRUCTIONS.to_string()
    }

    fn build_catalog_json(
        entries: &[CodexCatalogModel],
        context_1m: Option<bool>,
        base_instructions: &str,
    ) -> serde_json::Value {
        let default_context = if context_1m == Some(true) {
            CATALOG_CONTEXT_1M
        } else {
            CATALOG_CONTEXT_STANDARD
        };
        let models: Vec<serde_json::Value> = entries
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let display = e
                    .display_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(e.slug.as_str());
                let context_window = e
                    .context_window
                    .filter(|value| *value > 0)
                    .unwrap_or(default_context);
                let reasoning_levels = Self::normalized_reasoning_levels(e);
                let supports_reasoning = !reasoning_levels.is_empty();
                // 默认档取第一个真实档位（跳过 none）；无真实档位则写 "none"。
                let default_reasoning_level = reasoning_levels
                    .iter()
                    .find(|level| level.as_str() != "none")
                    .cloned()
                    .unwrap_or_else(|| "none".to_string());
                let supports_images = e.supports_images.unwrap_or(false);
                let supports_tool_calls = e.supports_tool_calls.unwrap_or(false);
                let supports_web_search = e.supports_web_search.unwrap_or(false);
                let reasoning_levels = serde_json::Value::Array(
                    reasoning_levels
                        .iter()
                        .map(|effort| {
                            serde_json::json!({
                                "effort": effort,
                                "description": format!("{effort} reasoning effort")
                            })
                        })
                        .collect(),
                );
                let input_modalities = if supports_images {
                    serde_json::json!(["text", "image"])
                } else {
                    serde_json::json!(["text"])
                };
                serde_json::json!({
                    "slug": e.slug,
                    "display_name": display,
                    "description": format!("Custom {} model via proxy provider.", e.slug),
                    "default_reasoning_level": default_reasoning_level,
                    "supported_reasoning_levels": reasoning_levels,
                    "shell_type": "unified_exec",
                    "visibility": "list",
                    "supported_in_api": true,
                    "priority": i,
                    "additional_speed_tiers": ["fast"],
                    "service_tiers": [{
                        "id": "priority",
                        "name": "Fast",
                        "description": "1.5x speed, increased usage"
                    }],
                    "upgrade": null,
                    "base_instructions": base_instructions,
                    "supports_reasoning_summaries": supports_reasoning,
                    "default_reasoning_summary": "none",
                    "support_verbosity": true,
                    "default_verbosity": "low",
                    "apply_patch_tool_type": "freeform",
                    "web_search_tool_type": "text_and_image",
                    "truncation_policy": { "mode": "tokens", "limit": 10000 },
                    "supports_parallel_tool_calls": supports_tool_calls,
                    "supports_image_detail_original": supports_images,
                    "context_window": context_window,
                    "max_context_window": context_window,
                    "effective_context_window_percent": 95,
                    "experimental_supported_tools": [],
                    "input_modalities": input_modalities,
                    "supports_search_tool": supports_web_search,
                    "use_responses_lite": false
                })
            })
            .collect();
        serde_json::json!({ "models": models })
    }

    /// 有有效列表时整表覆盖 model_catalog.json。
    fn write_model_catalog(&self, api_profile: &ApiProfile) -> Result<()> {
        let entries = Self::effective_catalog_models(api_profile);
        if entries.is_empty() {
            return Ok(());
        }

        if let Some(parent) = self.model_catalog_path().parent() {
            ensure_private_dir(parent).context("Failed to create Codex config directory")?;
        }

        let base = self.catalog_template_base_instructions();
        let catalog = Self::build_catalog_json(&entries, api_profile.context_1m, &base);
        let content = serde_json::to_string_pretty(&catalog)
            .context("Failed to serialize model_catalog.json")?;

        let path = self.model_catalog_path();
        atomic_write_private(&path, content.as_bytes())
            .context("Failed to write model_catalog.json")?;
        Ok(())
    }

    /// 解析本次切换的目标 provider id（与 merge_config 共用，保证校验与写入一致）。
    /// 返回 (provider_id, 是否 bedrock)。
    fn active_provider_id(api_profile: &ApiProfile) -> (String, bool) {
        if Self::is_amazon_bedrock_profile(api_profile) {
            return ("amazon-bedrock".to_string(), true);
        }
        // 非 bedrock：用 profile.provider 作为 id（默认沿用 custom），保留字加后缀。
        let raw_id = if api_profile.provider.is_empty() {
            "custom".to_string()
        } else {
            api_profile.provider.to_lowercase()
        };
        if Self::is_reserved_provider_id(&raw_id) {
            (format!("{raw_id}-custom"), false)
        } else {
            (raw_id, false)
        }
    }

    /// Codex 内置（保留）的 provider id —— 不允许在 model_providers 中覆盖。
    /// 参见 Codex 报错：`model_providers contains reserved built-in provider IDs`。
    fn is_reserved_provider_id(id: &str) -> bool {
        matches!(id, "openai" | "ollama" | "lmstudio")
    }

    fn env_key(api_profile: &ApiProfile) -> Option<&str> {
        api_profile
            .codex
            .env_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    /// 将 toml::Value 转换为 serde_json::Value
    fn toml_to_json(value: toml::Value) -> serde_json::Value {
        match value {
            toml::Value::String(s) => serde_json::Value::String(s),
            toml::Value::Integer(i) => serde_json::Value::Number(i.into()),
            toml::Value::Float(f) => serde_json::Number::from_f64(f)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null),
            toml::Value::Boolean(b) => serde_json::Value::Bool(b),
            toml::Value::Datetime(dt) => serde_json::Value::String(dt.to_string()),
            toml::Value::Array(arr) => {
                serde_json::Value::Array(arr.into_iter().map(Self::toml_to_json).collect())
            }
            toml::Value::Table(table) => {
                let map = table
                    .into_iter()
                    .map(|(k, v)| (k, Self::toml_to_json(v)))
                    .collect();
                serde_json::Value::Object(map)
            }
        }
    }

    /// 将 serde_json::Value 转换为 toml::Value
    fn json_to_toml(value: &serde_json::Value) -> Result<toml::Value> {
        Ok(match value {
            serde_json::Value::Null => {
                // TOML 不支持 null，跳过（用空字符串占位会污染配置，调用方应过滤）
                anyhow::bail!("TOML does not support null values")
            }
            serde_json::Value::Bool(b) => toml::Value::Boolean(*b),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    toml::Value::Integer(i)
                } else if let Some(f) = n.as_f64() {
                    toml::Value::Float(f)
                } else {
                    anyhow::bail!("Unsupported number type")
                }
            }
            serde_json::Value::String(s) => toml::Value::String(s.clone()),
            serde_json::Value::Array(arr) => {
                let mut out = Vec::new();
                for item in arr {
                    out.push(Self::json_to_toml(item)?);
                }
                toml::Value::Array(out)
            }
            serde_json::Value::Object(map) => {
                let mut table = toml::map::Map::new();
                for (k, v) in map {
                    // 跳过 null 值
                    if v.is_null() {
                        continue;
                    }
                    table.insert(k.clone(), Self::json_to_toml(v)?);
                }
                toml::Value::Table(table)
            }
        })
    }
}

impl Default for CodexAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfigAdapter for CodexAdapter {
    fn config_path(&self) -> PathBuf {
        self.config_file_path()
    }

    fn read_config(&self) -> Result<serde_json::Value> {
        let path = self.config_path();

        if !path.exists() {
            return Ok(serde_json::json!({}));
        }

        let content = fs::read_to_string(&path).context("Failed to read Codex config")?;
        let toml_value: toml::Value =
            toml::from_str(&content).context("Failed to parse Codex TOML config")?;

        Ok(Self::toml_to_json(toml_value))
    }

    fn extract_shared_config(&self, config: &serde_json::Value) -> serde_json::Value {
        let mut shared = config.clone();

        // Codex 的 API key 存在独立的 ~/.codex/auth.json，不在 config.toml 里。
        // config.toml 里由 Profile 管理的端点/凭据信息是各 provider 的
        // base_url、env_key、experimental_bearer_token 和 auth（命令式 token）。
        // 它们不属于 shared 配置，但协议字段仍要保留，切换时再由 Profile 还原。
        if let Some(obj) = shared.as_object_mut() {
            // 兼容历史版本误写入的顶层 api_key
            obj.remove("api_key");

            let active_provider = obj
                .get("model_provider")
                .and_then(|value| value.as_str())
                .map(str::to_string);
            if let Some(providers) = obj
                .get_mut("model_providers")
                .and_then(|v| v.as_object_mut())
            {
                // Never persist provider credentials in shared_configs. This
                // applies to inactive providers too, because portable backups
                // contain the complete shared configuration snapshot.
                for provider in providers.values_mut() {
                    if let Some(provider) = provider.as_object_mut() {
                        provider.remove("api_key");
                        provider.remove("experimental_bearer_token");
                        // auth 命令参数可能含敏感内容，不进 shared/便携备份；
                        // 切换时由 Profile 重新写入。
                        provider.remove("auth");
                    }
                }
                if let Some(active_provider) = active_provider {
                    if let Some(p) = providers
                        .get_mut(&active_provider)
                        .and_then(|value| value.as_object_mut())
                    {
                        p.remove("base_url");
                        p.remove("env_key");
                    }
                }
            }
        }

        shared
    }

    fn validate_profile(&self, api_profile: &ApiProfile) -> Result<()> {
        if !Self::is_amazon_bedrock_profile(api_profile) {
            if api_profile.api_url.trim().is_empty() {
                anyhow::bail!("Codex custom provider requires an API URL");
            }
            if Self::env_key(api_profile).is_none()
                && api_profile.api_key.trim().is_empty()
                && api_profile
                    .codex
                    .experimental_bearer_token
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .is_none()
                && !api_profile.codex.has_command_auth()
            {
                anyhow::bail!(
                    "Codex custom provider requires an API key, env_key, bearer token, or auth command"
                );
            }
            // auth 命令式 token 与其它静态凭据互斥（官方要求）。
            if api_profile.codex.has_command_auth() {
                if Self::env_key(api_profile).is_some() {
                    anyhow::bail!("Codex auth command cannot be combined with env_key");
                }
                if api_profile
                    .codex
                    .experimental_bearer_token
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .is_some()
                {
                    anyhow::bail!(
                        "Codex auth command cannot be combined with experimental_bearer_token"
                    );
                }
                if api_profile.codex.requires_openai_auth == Some(true) {
                    anyhow::bail!(
                        "Codex auth command cannot be combined with requires_openai_auth"
                    );
                }
                for (label, value) in [
                    ("auth timeout", api_profile.codex.auth_timeout_ms),
                    (
                        "auth refresh interval",
                        api_profile.codex.auth_refresh_interval_ms,
                    ),
                ] {
                    if let Some(ms) = value {
                        if ms <= 0 {
                            anyhow::bail!(
                                "Codex {label} must be a positive number of milliseconds"
                            );
                        }
                    }
                }
            }
        }

        // wire_api="chat" 已于 2026-02 被官方删除（discussion #7782），残留即报错，
        // 指引用户切 responses；未知取值同样拒绝，避免写出无法启动的配置。
        if let Some(wire) = api_profile
            .codex
            .wire_api
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            if is_removed_chat_wire_api(wire) {
                anyhow::bail!(
                    "Codex wire_api = \"{wire}\" was removed in Feb 2026 (only \"responses\" is supported); re-save the profile in Helio to migrate it (https://github.com/openai/codex/discussions/7782)"
                );
            }
            if !is_supported_wire_api(wire) {
                anyhow::bail!("Unsupported Codex wire_api: {wire}");
            }
        }

        if let Some(effort) = api_profile
            .codex
            .reasoning_effort
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            if !CODEX_REASONING_LEVELS.contains(&effort) {
                anyhow::bail!("Unsupported Codex reasoning effort: {effort}");
            }
        }

        for (label, allowed, value) in [
            (
                "service_tier",
                CODEX_SERVICE_TIERS,
                api_profile.codex.service_tier.as_deref(),
            ),
            (
                "reasoning_summary",
                CODEX_REASONING_SUMMARIES,
                api_profile.codex.reasoning_summary.as_deref(),
            ),
            (
                "verbosity",
                CODEX_VERBOSITY_LEVELS,
                api_profile.codex.verbosity.as_deref(),
            ),
        ] {
            if let Some(v) = value.map(str::trim).filter(|v| !v.is_empty()) {
                if !allowed.contains(&v) {
                    anyhow::bail!("Unsupported Codex {label}: {v}");
                }
            }
        }

        if let Some(entries) = api_profile.codex.catalog_models.as_ref() {
            for entry in entries {
                if let Some(levels) = entry.reasoning_levels.as_ref() {
                    for level in levels {
                        let normalized = level.trim().to_ascii_lowercase();
                        // catalog 侧按八档校验（含 none/max/ultra），顶层仍只认 5 档。
                        if !CODEX_CATALOG_REASONING_LEVELS.contains(&normalized.as_str()) {
                            anyhow::bail!(
                                "Unsupported Codex catalog reasoning level for {}: {}",
                                entry.slug,
                                level
                            );
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn merge_config(
        &self,
        api_profile: &ApiProfile,
        shared_config: &serde_json::Value,
    ) -> serde_json::Value {
        // 防御：非对象共享配置从空对象起步，避免后续下标写入 panic。正常路径无影响。
        let mut config = if shared_config.is_object() {
            shared_config.clone()
        } else {
            serde_json::json!({})
        };

        let is_bedrock = Self::is_amazon_bedrock_profile(api_profile);
        if config.get("model_providers").is_none() {
            config["model_providers"] = serde_json::json!({});
        }

        if is_bedrock {
            config["model_provider"] = serde_json::Value::String("amazon-bedrock".to_string());
            if let Some(providers) = config
                .get_mut("model_providers")
                .and_then(|value| value.as_object_mut())
            {
                providers.remove("amazon-bedrock-custom");
                let profile = api_profile
                    .codex
                    .aws_profile
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty());
                let region = api_profile
                    .codex
                    .aws_region
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty());
                if profile.is_some() || region.is_some() {
                    let mut aws = serde_json::Map::new();
                    if let Some(profile) = profile {
                        aws.insert(
                            "profile".to_string(),
                            serde_json::Value::String(profile.to_string()),
                        );
                    }
                    if let Some(region) = region {
                        aws.insert(
                            "region".to_string(),
                            serde_json::Value::String(region.to_string()),
                        );
                    }
                    providers.insert(
                        "amazon-bedrock".to_string(),
                        serde_json::json!({ "aws": aws }),
                    );
                } else {
                    providers.remove("amazon-bedrock");
                }
            }
        } else {
            // 使用 profile.provider 作为 provider id（默认沿用 "custom"）。
            // Codex 保留了内置 provider id（如 `openai`），不允许在 model_providers
            // 中覆盖；若撞上保留字则加 `-custom` 后缀（与 Codex 报错建议一致）。
            let (provider_id, _) = Self::active_provider_id(api_profile);

            // 写入目标 provider 配置并保留 Profile 指定的协议与鉴权模式；其他 provider 不动。
            if let Some(providers) = config
                .get_mut("model_providers")
                .and_then(|v| v.as_object_mut())
            {
                let is_new = !providers.contains_key(&provider_id);
                let entry = providers
                    .entry(provider_id.clone())
                    .or_insert_with(|| serde_json::json!({}));
                if let Some(p) = entry.as_object_mut() {
                    p.insert(
                        "base_url".to_string(),
                        serde_json::Value::String(api_profile.api_url.clone()),
                    );
                    let bearer_token = api_profile
                        .codex
                        .experimental_bearer_token
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty());
                    if api_profile.codex.has_command_auth() {
                        // 命令式 token：写 [model_providers.<id>.auth]，清掉互斥的静态凭据键。
                        let command = api_profile
                            .codex
                            .auth_command
                            .as_deref()
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .unwrap_or_default();
                        let mut auth = serde_json::Map::new();
                        auth.insert(
                            "command".to_string(),
                            serde_json::Value::String(command.to_string()),
                        );
                        let args: Vec<serde_json::Value> = api_profile
                            .codex
                            .auth_args
                            .as_ref()
                            .map(|list| {
                                list.iter()
                                    .map(|arg| arg.trim())
                                    .filter(|arg| !arg.is_empty())
                                    .map(|arg| serde_json::Value::String(arg.to_string()))
                                    .collect()
                            })
                            .unwrap_or_default();
                        if !args.is_empty() {
                            auth.insert("args".to_string(), serde_json::Value::Array(args));
                        }
                        for (key, value) in [
                            ("timeout_ms", api_profile.codex.auth_timeout_ms),
                            (
                                "refresh_interval_ms",
                                api_profile.codex.auth_refresh_interval_ms,
                            ),
                        ] {
                            if let Some(ms) = value.filter(|ms| *ms > 0) {
                                auth.insert(key.to_string(), serde_json::Value::Number(ms.into()));
                            }
                        }
                        if let Some(cwd) = api_profile
                            .codex
                            .auth_cwd
                            .as_deref()
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                        {
                            auth.insert(
                                "cwd".to_string(),
                                serde_json::Value::String(cwd.to_string()),
                            );
                        }
                        p.insert("auth".to_string(), serde_json::Value::Object(auth));
                        p.remove("env_key");
                        p.remove("experimental_bearer_token");
                        p.remove("requires_openai_auth");
                    } else {
                        p.remove("auth");
                        if let Some(env_key) = Self::env_key(api_profile) {
                            p.insert(
                                "env_key".to_string(),
                                serde_json::Value::String(env_key.to_string()),
                            );
                        } else {
                            p.remove("env_key");
                        }
                        // 鉴权默认：显式值优先；env_key / bearer 模式不需要登录态 → false；
                        // 其余（auth.json 写 key）保持 true，否则 Codex 不会读取 auth.json。
                        let requires_openai_auth = api_profile
                            .codex
                            .requires_openai_auth
                            .or_else(|| Self::env_key(api_profile).map(|_| false))
                            .or_else(|| bearer_token.map(|_| false))
                            .or(Some(true));
                        if let Some(requires_openai_auth) = requires_openai_auth {
                            p.insert(
                                "requires_openai_auth".to_string(),
                                serde_json::Value::Bool(requires_openai_auth),
                            );
                        }
                        if api_profile.codex.supports_standalone_web_search == Some(true) {
                            p.insert(
                                "supports_standalone_web_search".to_string(),
                                serde_json::Value::Bool(true),
                            );
                        } else {
                            p.remove("supports_standalone_web_search");
                        }
                        match bearer_token {
                            Some(token) => {
                                p.insert(
                                    "experimental_bearer_token".to_string(),
                                    serde_json::Value::String(token.to_string()),
                                );
                            }
                            None => {
                                p.remove("experimental_bearer_token");
                            }
                        }
                    } // 命令式 token 分支结束；以下对两种鉴权模式通用
                      // wire_api 固定 responses：chat 已于 2026-02 被官方删除，
                      // 历史值在这里自愈（validate 会提示用户清理存量）。
                    p.insert(
                        "wire_api".to_string(),
                        serde_json::Value::String("responses".to_string()),
                    );
                    if is_new {
                        // 全新 provider：补上 Codex 必需的 name 默认值。
                        p.entry("name".to_string())
                            .or_insert_with(|| serde_json::Value::String(provider_id.clone()));
                    }
                }
            }
            config["model_provider"] = serde_json::Value::String(provider_id);
        }

        // API key 不写 config.toml —— 走 auth.json（见 apply_api_credentials），
        // 且清掉历史版本误写入的顶层 api_key。
        if let Some(obj) = config.as_object_mut() {
            obj.remove("api_key");
            obj.remove("aws_profile");
            obj.remove("aws_region");
            // 只有 Helio 直写 auth.json 的模式才强制 file；
            // env_key / auth 命令 / bedrock 模式不经 auth.json，顺手清掉残留，
            // 避免覆盖用户自己的 keyring/auto 偏好。
            let manages_auth_json = !is_bedrock
                && Self::env_key(api_profile).is_none()
                && !api_profile.codex.has_command_auth();
            if manages_auth_json {
                obj.insert(
                    "cli_auth_credentials_store".to_string(),
                    serde_json::Value::String("file".to_string()),
                );
            } else {
                obj.remove("cli_auth_credentials_store");
            }

            match api_profile
                .model
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                Some(model) => {
                    obj.insert(
                        "model".to_string(),
                        serde_json::Value::String(model.to_string()),
                    );
                }
                None => {
                    obj.remove("model");
                }
            }

            match api_profile
                .codex
                .reasoning_effort
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                Some(reasoning_effort) => {
                    obj.insert(
                        "model_reasoning_effort".to_string(),
                        serde_json::Value::String(reasoning_effort.to_string()),
                    );
                }
                None => {
                    obj.remove("model_reasoning_effort");
                }
            }

            // reasoning_summary / verbosity：Some → 写入；None → 不动。
            // 行为设置页的手填值不应被“未管理该字段”的 Profile 切换清掉。
            for (field, key) in [
                (
                    api_profile.codex.reasoning_summary.as_deref(),
                    "model_reasoning_summary",
                ),
                (api_profile.codex.verbosity.as_deref(), "model_verbosity"),
            ] {
                if let Some(value) = field.map(str::trim).filter(|v| !v.is_empty()) {
                    obj.insert(
                        key.to_string(),
                        serde_json::Value::String(value.to_string()),
                    );
                }
            }

            match api_profile.context_1m {
                Some(true) => {
                    obj.insert(
                        "model_context_window".to_string(),
                        serde_json::Value::Number(1_000_000.into()),
                    );
                    obj.insert(
                        "model_auto_compact_token_limit".to_string(),
                        serde_json::Value::Number(900_000.into()),
                    );
                }
                // Some(false) = 显式关闭 → 清理；None = 不管理 → 保留用户手填的值。
                Some(false) => {
                    obj.remove("model_context_window");
                    obj.remove("model_auto_compact_token_limit");
                }
                None => {}
            }

            obj.remove("model_effort_level");

            obj.remove("model_thinking_enabled");

            match api_profile
                .codex
                .service_tier
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                Some(tier) => {
                    obj.insert(
                        "service_tier".to_string(),
                        serde_json::Value::String(tier.to_string()),
                    );
                }
                None => {
                    obj.remove("service_tier");
                }
            }

            // 有效 catalog 非空时设置指针；空则不强制清除已有 model_catalog_json
            if !Self::effective_catalog_models(api_profile).is_empty() {
                let catalog_path = self.model_catalog_path();
                obj.insert(
                    "model_catalog_json".to_string(),
                    serde_json::Value::String(catalog_path.to_string_lossy().into_owned()),
                );
            }
        }

        config
    }

    /// Pre-write semantic check: the merged config must carry this switch target provider.
    /// On failure the switch transaction rolls back from snapshots, so a broken config
    /// is never silently written to disk.
    fn verify_merged_config(
        &self,
        merged: &serde_json::Value,
        api_profile: &ApiProfile,
    ) -> Result<()> {
        let merged_obj = merged.as_object().ok_or_else(|| {
            anyhow::anyhow!("Codex merge result is not an object; refusing to write")
        })?;
        if merged_obj.is_empty() {
            anyhow::bail!("Codex merge result is empty; refusing to write");
        }
        let (provider_id, is_bedrock) = Self::active_provider_id(api_profile);
        let active = merged_obj
            .get("model_provider")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        if active != provider_id {
            anyhow::bail!("Codex merge result provider mismatch; refusing to write");
        }
        // Bedrock without aws settings is intentionally omitted by merge; otherwise
        // the target section must exist and offer a working endpoint/credential route.
        if is_bedrock {
            return Ok(());
        }
        let entry = merged_obj
            .get("model_providers")
            .and_then(|value| value.as_object())
            .and_then(|providers| providers.get(&provider_id))
            .and_then(|value| value.as_object())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Codex merge result misses target provider section; refusing to write"
                )
            })?;
        let non_empty_str = |key: &str| {
            entry
                .get(key)
                .and_then(|value| value.as_str())
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false)
        };
        let has_command_auth = entry
            .get("auth")
            .and_then(|value| value.as_object())
            .map(|auth| !auth.is_empty())
            .unwrap_or(false);
        if !(non_empty_str("base_url")
            || non_empty_str("env_key")
            || non_empty_str("experimental_bearer_token")
            || has_command_auth)
        {
            anyhow::bail!(
                "Codex merge result target provider has no endpoint/credential; refusing to write"
            );
        }
        match entry.get("wire_api").and_then(|value| value.as_str()) {
            Some("responses") => Ok(()),
            _ => anyhow::bail!(
                "Codex merge result target provider wire_api invalid; refusing to write"
            ),
        }
    }

    fn write_config(&self, config: &serde_json::Value) -> Result<()> {
        let path = self.config_path();

        if let Some(parent) = path.parent() {
            ensure_private_dir(parent).context("Failed to create Codex config directory")?;
        }

        let toml_value = Self::json_to_toml(config)?;
        let content =
            toml::to_string_pretty(&toml_value).context("Failed to serialize Codex TOML")?;

        // 原子写入：临时文件 + rename
        atomic_write_private(&path, content.as_bytes()).context("Failed to write Codex config")?;

        Ok(())
    }

    fn backup_config(&self) -> Result<PathBuf> {
        let path = self.config_path();

        if !path.exists() {
            anyhow::bail!("Config file does not exist");
        }

        let backup_path = backup::backup_required(&self.config_dir, &path, "config")?;

        // 同时备份 auth.json（如果存在）—— API key + 登录态（tokens.refresh 等）都在这里，
        // 仅备份 config.toml 不足以在误操作后完整恢复。备份失败不中断主备份。
        let _ = backup::backup_one(&self.config_dir, &self.auth_file_path(), "auth")?;

        // 备份 model_catalog.json（切换可能整表覆盖）
        let _ = backup::backup_one(
            &self.config_dir,
            &self.model_catalog_path(),
            "model_catalog",
        )?;

        self.cleanup_old_backups(10)?;

        Ok(backup_path)
    }

    fn cleanup_old_backups(&self, keep: usize) -> Result<()> {
        // config.backup.* 与 auth.backup.* / catalog 各自独立计数，互不挤占。
        backup::cleanup_prefix(&self.config_dir, "config.backup.", keep)?;
        backup::cleanup_prefix(&self.config_dir, "auth.backup.", keep)?;
        backup::cleanup_prefix(&self.config_dir, "model_catalog.backup.", keep)
    }

    fn managed_paths(&self) -> Vec<PathBuf> {
        vec![
            self.config_file_path(),
            self.auth_file_path(),
            self.model_catalog_path(),
        ]
    }

    /// Codex 特有：API key 存在独立的 ~/.codex/auth.json 的 OPENAI_API_KEY 字段，
    /// 而非 config.toml。保留 auth.json 中的其他字段，只更新 OPENAI_API_KEY。
    /// 同时在有效 catalog 列表非空时写入 model_catalog.json。
    fn apply_api_credentials(&self, api_profile: &ApiProfile) -> Result<()> {
        // catalog 先于 auth：失败则整次 switch 的 apply 失败，可重试
        self.write_model_catalog(api_profile)?;

        if Self::is_amazon_bedrock_profile(api_profile)
            || Self::env_key(api_profile).is_some()
            || api_profile.codex.has_command_auth()
        {
            return Ok(());
        }

        let path = self.auth_file_path();

        // 读取现有 auth.json（保留其他字段），解析失败则从空对象开始。
        let mut auth = if path.exists() {
            fs::read_to_string(&path)
                .ok()
                .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
                .unwrap_or_else(|| serde_json::json!({}))
        } else {
            serde_json::json!({})
        };

        if !auth.is_object() {
            auth = serde_json::json!({});
        }
        if let Some(obj) = auth.as_object_mut() {
            obj.insert(
                "auth_mode".to_string(),
                serde_json::Value::String("apikey".to_string()),
            );
            obj.insert(
                "OPENAI_API_KEY".to_string(),
                serde_json::Value::String(api_profile.api_key.clone()),
            );
        }

        if let Some(parent) = path.parent() {
            ensure_private_dir(parent).context("Failed to create Codex config directory")?;
        }

        let content =
            serde_json::to_string_pretty(&auth).context("Failed to serialize Codex auth.json")?;
        atomic_write_private(&path, content.as_bytes())
            .context("Failed to write Codex auth.json")?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::CodexProfileFields;

    fn sample_profile() -> ApiProfile {
        ApiProfile {
            id: Some(1),
            name: "test".to_string(),
            provider: "openai".to_string(),
            api_url: "https://api.example.com/v1".to_string(),
            api_key: "sk-test-key".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn test_merge_with_non_object_shared_starts_empty() {
        let adapter = CodexAdapter::new();
        // Guard: null shared config no longer panics; merge starts from empty object.
        let merged = adapter.merge_config(&sample_profile(), &serde_json::Value::Null);
        assert_eq!(merged["model_provider"], "openai-custom");
        assert_eq!(
            merged["model_providers"]["openai-custom"]["base_url"],
            "https://api.example.com/v1"
        );
    }

    #[test]
    fn test_verify_merged_config_accepts_valid_merge() {
        let adapter = CodexAdapter::new();
        let shared = adapter.extract_shared_config(&serde_json::json!({
            "model_provider": "openai-custom",
            "sandbox_mode": "danger-full-access",
            "mcp_servers": {"a": {"command": "x"}},
            "model_providers": {
                "openai-custom": {"base_url": "https://old", "wire_api": "responses"}
            }
        }));
        let profile = sample_profile();
        let merged = adapter.merge_config(&profile, &shared);
        adapter.verify_merged_config(&merged, &profile).unwrap();
        // Shared areas must survive the switch.
        assert_eq!(merged["sandbox_mode"], "danger-full-access");
        assert!(merged.get("mcp_servers").is_some());
    }

    #[test]
    fn test_verify_merged_config_rejects_stripped_config() {
        let adapter = CodexAdapter::new();
        let profile = sample_profile();
        // Empty object.
        assert!(adapter
            .verify_merged_config(&serde_json::json!({}), &profile)
            .is_err());
        // Missing provider section.
        assert!(adapter
            .verify_merged_config(
                &serde_json::json!({"model_provider": "openai-custom", "model_providers": {}}),
                &profile
            )
            .is_err());
        // Active provider mismatch.
        assert!(adapter
            .verify_merged_config(
                &serde_json::json!({
                    "model_provider": "other",
                    "model_providers": {
                        "openai-custom": {"base_url": "https://x", "wire_api": "responses"}
                    }
                }),
                &profile
            )
            .is_err());
        // Illegal wire_api.
        assert!(adapter
            .verify_merged_config(
                &serde_json::json!({
                    "model_provider": "openai-custom",
                    "model_providers": {
                        "openai-custom": {"base_url": "https://x", "wire_api": "chat"}
                    }
                }),
                &profile
            )
            .is_err());
    }

    #[test]
    fn test_switch_transaction_preserves_shared_areas_end_to_end() {
        use crate::adapters::apply_profile_transaction;
        let dir = tempfile::tempdir().unwrap();
        // Seed a realistic full user config: providers, sandbox, MCP must survive.
        fs::write(
            dir.path().join("config.toml"),
            "model = \"muse-spark-1.3\"\nmodel_provider = \"openai-custom\"\nsandbox_mode = \"danger-full-access\"\ncli_auth_credentials_store = \"file\"\n\n[model_providers.openai-custom]\nbase_url = \"https://old.example/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n\n[mcp_servers.codegraph]\ncommand = \"codegraph\"\n",
        )
        .unwrap();
        let adapter = CodexAdapter {
            config_dir: dir.path().to_path_buf(),
        };
        let disk = adapter.read_config().unwrap();
        let shared = adapter.extract_shared_config(&disk);
        let mut profile = sample_profile();
        profile.model = Some("muse-spark-1.3".to_string());
        apply_profile_transaction(&adapter, &profile, &shared).unwrap();
        let written = fs::read_to_string(dir.path().join("config.toml")).unwrap();
        let parsed: toml::Value = toml::from_str(&written).unwrap();
        assert_eq!(parsed["model_provider"].as_str(), Some("openai-custom"));
        assert_eq!(
            parsed["model_providers"]["openai-custom"]["base_url"].as_str(),
            Some("https://api.example.com/v1")
        );
        // User areas untouched by the switch.
        assert_eq!(parsed["sandbox_mode"].as_str(), Some("danger-full-access"));
        assert!(parsed.get("mcp_servers").is_some());
        assert_eq!(parsed["model"].as_str(), Some("muse-spark-1.3"));
    }

    #[test]
    fn test_toml_json_roundtrip() {
        let toml_str = r#"
model_provider = "openai"

[model_providers.openai]
base_url = "https://old.api.com"
name = "OpenAI"

[mcp_servers.fs]
command = "npx"
"#;
        let toml_value: toml::Value = toml::from_str(toml_str).unwrap();
        let json = CodexAdapter::toml_to_json(toml_value);

        assert_eq!(json["model_provider"], "openai");
        assert_eq!(
            json["model_providers"]["openai"]["base_url"],
            "https://old.api.com"
        );
        assert_eq!(json["mcp_servers"]["fs"]["command"], "npx");

        // 往返回 TOML
        let back = CodexAdapter::json_to_toml(&json).unwrap();
        let s = toml::to_string_pretty(&back).unwrap();
        assert!(s.contains("model_provider"));
        assert!(s.contains("mcp_servers"));
    }

    #[test]
    fn test_extract_shared_removes_api() {
        let adapter = CodexAdapter::new();
        let config = serde_json::json!({
            "api_key": "sk-secret",
            "model_provider": "openai",
            "model_providers": {
                "openai": {
                    "base_url": "https://api.com",
                    "name": "OpenAI",
                    "experimental_bearer_token": "bearer-secret"
                }
            },
            "mcp_servers": {
                "fs": { "command": "npx" }
            }
        });

        let shared = adapter.extract_shared_config(&config);

        // API 字段被移除
        assert!(shared.get("api_key").is_none());
        assert!(shared["model_providers"]["openai"]
            .get("base_url")
            .is_none());
        assert!(shared["model_providers"]["openai"]
            .get("experimental_bearer_token")
            .is_none());
        // 共享字段保留
        assert_eq!(shared["model_providers"]["openai"]["name"], "OpenAI");
        assert_eq!(shared["mcp_servers"]["fs"]["command"], "npx");
    }

    #[test]
    fn test_extract_shared_removes_provider_secrets_from_inactive_providers() {
        let adapter = CodexAdapter::new();
        let config = serde_json::json!({
            "model_provider": "active",
            "model_providers": {
                "active": {
                    "base_url": "https://active.example",
                    "env_key": "ACTIVE_KEY",
                    "experimental_bearer_token": "active-bearer",
                    "name": "Active"
                },
                "inactive": {
                    "base_url": "https://inactive.example",
                    "env_key": "INACTIVE_KEY",
                    "experimental_bearer_token": "inactive-bearer",
                    "api_key": "inactive-key",
                    "name": "Inactive"
                }
            }
        });

        let shared = adapter.extract_shared_config(&config);

        let active = &shared["model_providers"]["active"];
        assert!(active.get("base_url").is_none());
        assert!(active.get("env_key").is_none());
        assert!(active.get("experimental_bearer_token").is_none());

        let inactive = &shared["model_providers"]["inactive"];
        assert_eq!(inactive["base_url"], "https://inactive.example");
        assert_eq!(inactive["env_key"], "INACTIVE_KEY");
        assert!(inactive.get("experimental_bearer_token").is_none());
        assert!(inactive.get("api_key").is_none());
        assert_eq!(inactive["name"], "Inactive");
    }

    #[test]
    fn test_merge_inserts_api() {
        let adapter = CodexAdapter::new();
        let shared = serde_json::json!({
            "mcp_servers": {
                "fs": { "command": "npx" }
            }
        });

        let merged = adapter.merge_config(&sample_profile(), &shared);

        // provider="openai" 是 Codex 保留字 → 自动改名 openai-custom
        assert_eq!(merged["model_provider"], "openai-custom");
        assert_eq!(
            merged["model_providers"]["openai-custom"]["base_url"],
            "https://api.example.com/v1"
        );
        // 全新 provider 补上协议默认值
        assert_eq!(
            merged["model_providers"]["openai-custom"]["wire_api"],
            "responses"
        );
        assert_eq!(
            merged["model_providers"]["openai-custom"]["requires_openai_auth"],
            true
        );
        // 不得创建被保留的 openai provider 块
        assert!(merged["model_providers"].get("openai").is_none());
        // API key 绝不写进 config.toml（走 auth.json）
        assert!(merged.get("api_key").is_none());
        assert!(merged["model_providers"]["openai-custom"]
            .get("env_key")
            .is_none());
        // 共享配置保留
        assert_eq!(merged["mcp_servers"]["fs"]["command"], "npx");
    }

    #[test]
    fn test_merge_uses_built_in_amazon_bedrock() {
        let adapter = CodexAdapter::new();
        // 非保留字 provider 原样使用
        let custom = ApiProfile {
            provider: "myproxy".to_string(),
            ..sample_profile()
        };
        let merged = adapter.merge_config(&custom, &serde_json::json!({}));
        assert_eq!(merged["model_provider"], "myproxy");
        assert!(merged["model_providers"]["myproxy"].is_object());

        // Amazon Bedrock is a Codex built-in provider, not a custom provider id.
        let built_in = ApiProfile {
            provider: "Amazon-Bedrock".to_string(),
            codex: CodexProfileFields {
                aws_profile: Some("production".into()),
                aws_region: Some("us-east-1".into()),
                ..Default::default()
            },
            ..sample_profile()
        };
        let merged = adapter.merge_config(
            &built_in,
            &serde_json::json!({
                "model_providers": {
                    "amazon-bedrock-custom": {"base_url": "https://stale.example"}
                }
            }),
        );
        assert_eq!(merged["model_provider"], "amazon-bedrock");
        assert!(merged["model_providers"]
            .get("amazon-bedrock-custom")
            .is_none());
        assert_eq!(
            merged["model_providers"]["amazon-bedrock"]["aws"]["profile"],
            "production"
        );
        assert_eq!(
            merged["model_providers"]["amazon-bedrock"]["aws"]["region"],
            "us-east-1"
        );
        assert!(merged.get("aws_profile").is_none());
        assert!(merged.get("aws_region").is_none());
        let toml = CodexAdapter::json_to_toml(&merged).unwrap();
        let serialized = toml::to_string(&toml).unwrap();
        assert!(serialized.contains("[model_providers.amazon-bedrock.aws]"));
        assert!(serialized.contains("profile = \"production\""));
        assert!(serialized.contains("region = \"us-east-1\""));
    }

    #[test]
    fn test_merge_applies_codex_model_parameters() {
        let adapter = CodexAdapter::new();
        let profile = ApiProfile {
            model: Some("gpt-5.5".to_string()),
            context_1m: Some(true),
            codex: CodexProfileFields {
                reasoning_effort: Some("xhigh".to_string()),
                ..Default::default()
            },
            ..sample_profile()
        };

        let merged = adapter.merge_config(&profile, &serde_json::json!({}));

        assert_eq!(merged["model"], "gpt-5.5");
        assert_eq!(merged["model_reasoning_effort"], "xhigh");
        assert_eq!(merged["model_context_window"], 1_000_000);
    }

    #[test]
    fn test_merge_clears_disabled_codex_model_parameters() {
        let adapter = CodexAdapter::new();
        let shared = serde_json::json!({
            "model": "old-model",
            "model_reasoning_effort": "high",
            "model_context_window": 1_000_000,
        });
        let profile = ApiProfile {
            model: None,
            context_1m: Some(false),
            ..sample_profile()
        };

        let merged = adapter.merge_config(&profile, &shared);

        assert!(merged.get("model").is_none());
        assert!(merged.get("model_reasoning_effort").is_none());
        assert!(merged.get("model_context_window").is_none());
    }

    #[test]
    fn test_merge_normalizes_legacy_auth_fields() {
        let adapter = CodexAdapter::new();
        // 已有 provider 用 responses，profile 的 requires_openai_auth=false 被应用，
        // 历史遗留的 wire_api="chat" 自愈为 responses（chat 已被官方删除）。
        let shared = serde_json::json!({
            "model_providers": {
                "myproxy": {
                    "name": "myproxy",
                    "wire_api": "responses",
                    "requires_openai_auth": true,
                    "base_url": "https://old.api.com"
                }
            }
        });
        let profile = ApiProfile {
            provider: "myproxy".to_string(),
            codex: CodexProfileFields {
                wire_api: Some("chat".to_string()),
                requires_openai_auth: Some(false),
                ..Default::default()
            },
            ..sample_profile()
        };

        let merged = adapter.merge_config(&profile, &shared);

        assert_eq!(
            merged["model_providers"]["myproxy"]["wire_api"],
            "responses"
        );
        assert_eq!(
            merged["model_providers"]["myproxy"]["requires_openai_auth"],
            false
        );
    }

    #[test]
    fn test_merge_uses_provider_env_key_and_preserves_bearer() {
        let adapter = CodexAdapter::new();
        let profile = ApiProfile {
            provider: "myproxy".to_string(),
            codex: CodexProfileFields {
                env_key: Some("MY_PROXY_KEY".to_string()),
                experimental_bearer_token: Some("sk-bearer-xyz".to_string()),
                ..Default::default()
            },
            ..sample_profile()
        };

        let merged = adapter.merge_config(&profile, &serde_json::json!({}));
        assert_eq!(
            merged["model_providers"]["myproxy"]["env_key"],
            "MY_PROXY_KEY"
        );
        assert_eq!(
            merged["model_providers"]["myproxy"]["requires_openai_auth"],
            false
        );
        assert_eq!(
            merged["model_providers"]["myproxy"]["experimental_bearer_token"],
            "sk-bearer-xyz"
        );
    }

    #[test]
    fn test_merge_writes_standalone_web_search_only_when_enabled() {
        let adapter = CodexAdapter::new();
        let enabled = ApiProfile {
            provider: "myproxy".into(),
            codex: CodexProfileFields {
                supports_standalone_web_search: Some(true),
                ..Default::default()
            },
            ..sample_profile()
        };
        let merged = adapter.merge_config(&enabled, &serde_json::json!({}));
        assert_eq!(
            merged["model_providers"]["myproxy"]["supports_standalone_web_search"],
            true
        );

        let disabled = ApiProfile {
            provider: "myproxy".into(),
            ..sample_profile()
        };
        let merged = adapter.merge_config(
            &disabled,
            &serde_json::json!({
                "model_providers": {
                    "myproxy": {"supports_standalone_web_search": true}
                }
            }),
        );
        assert!(merged["model_providers"]["myproxy"]
            .get("supports_standalone_web_search")
            .is_none());
    }

    #[test]
    fn test_validate_rejects_unsupported_reasoning_levels() {
        let adapter = CodexAdapter::new();
        let profile = ApiProfile {
            codex: CodexProfileFields {
                reasoning_effort: Some("ultra".into()),
                ..Default::default()
            },
            ..sample_profile()
        };
        assert!(adapter.validate_profile(&profile).is_err());

        let profile = ApiProfile {
            codex: CodexProfileFields {
                catalog_models: Some(vec![CodexCatalogModel {
                    slug: "proxy-model".into(),
                    reasoning_levels: Some(vec!["turbo".into()]),
                    ..Default::default()
                }]),
                ..Default::default()
            },
            ..sample_profile()
        };
        assert!(adapter.validate_profile(&profile).is_err());

        // catalog 八档（none~ultra）放行，顶层仍只认 5 档。
        let catalog_ok = ApiProfile {
            codex: CodexProfileFields {
                catalog_models: Some(vec![CodexCatalogModel {
                    slug: "sol-like".into(),
                    reasoning_levels: Some(vec![
                        "low".into(),
                        "medium".into(),
                        "high".into(),
                        "xhigh".into(),
                        "max".into(),
                        "ultra".into(),
                    ]),
                    ..Default::default()
                }]),
                ..Default::default()
            },
            ..sample_profile()
        };
        assert!(adapter.validate_profile(&catalog_ok).is_ok());
    }

    #[test]
    fn test_switch_preserves_unrelated_provider_exactly() {
        let adapter = CodexAdapter::new();
        let current = serde_json::json!({
            "model_provider": "provider-a",
            "model_providers": {
                "provider-a": {"base_url": "https://a.old", "env_key": "A_KEY", "wire_api": "responses"},
                "provider-b": {"base_url": "https://b", "env_key": "B_KEY", "requires_openai_auth": false, "custom": {"keep": true}}
            }
        });
        let provider_b = current["model_providers"]["provider-b"].clone();
        let shared = adapter.extract_shared_config(&current);
        let profile = ApiProfile {
            provider: "provider-a".into(),
            api_url: "https://a.new".into(),
            codex: CodexProfileFields {
                env_key: Some("A_KEY_NEW".into()),
                ..Default::default()
            },
            ..sample_profile()
        };
        let merged = adapter.merge_config(&profile, &shared);
        assert_eq!(merged["model_providers"]["provider-b"], provider_b);
    }

    #[test]
    fn test_merge_applies_top_level_codex_params() {
        let adapter = CodexAdapter::new();
        let profile = ApiProfile {
            codex: CodexProfileFields {
                service_tier: Some("fast".to_string()),
                ..Default::default()
            },
            ..sample_profile()
        };

        let merged = adapter.merge_config(&profile, &serde_json::json!({}));

        assert_eq!(merged["service_tier"], "fast");
    }

    #[test]
    fn test_merge_clears_disabled_top_level_codex_params() {
        let adapter = CodexAdapter::new();
        let shared = serde_json::json!({
            "model_thinking_enabled": true,
            "service_tier": "fast",
        });
        let profile = ApiProfile { ..sample_profile() };

        let merged = adapter.merge_config(&profile, &shared);

        assert!(merged.get("model_thinking_enabled").is_none());
        assert!(merged.get("service_tier").is_none());
    }

    #[test]
    fn test_merge_preserves_existing_provider_protocol() {
        let adapter = CodexAdapter::new();
        // 已有 custom provider，带 wire_api / requires_openai_auth
        let shared = serde_json::json!({
            "model_providers": {
                "custom": {
                    "name": "custom",
                    "wire_api": "responses",
                    "requires_openai_auth": true,
                    "base_url": "https://old.api.com"
                }
            }
        });
        let profile = ApiProfile {
            provider: "custom".to_string(),
            api_url: "https://new.api.com/v1".to_string(),
            api_key: "sk-x".to_string(),
            ..sample_profile()
        };

        let merged = adapter.merge_config(&profile, &shared);

        // base_url 被更新，协议字段被原样保留
        assert_eq!(
            merged["model_providers"]["custom"]["base_url"],
            "https://new.api.com/v1"
        );
        assert_eq!(merged["model_providers"]["custom"]["wire_api"], "responses");
        assert_eq!(
            merged["model_providers"]["custom"]["requires_openai_auth"],
            true
        );
    }

    #[test]
    fn test_merge_fills_missing_openai_auth_on_existing_provider() {
        let adapter = CodexAdapter::new();
        let shared = serde_json::json!({
            "model_providers": {
                "custom": {
                    "name": "custom",
                    "wire_api": "responses"
                }
            }
        });
        let profile = ApiProfile {
            provider: "custom".to_string(),
            ..sample_profile()
        };

        let merged = adapter.merge_config(&profile, &shared);

        assert_eq!(
            merged["model_providers"]["custom"]["requires_openai_auth"],
            true
        );
    }

    #[test]
    fn test_effective_catalog_auto_includes_default_model() {
        let profile = ApiProfile {
            model: Some("gpt-default".into()),
            codex: CodexProfileFields {
                catalog_models: Some(vec![CodexCatalogModel {
                    slug: "gpt-extra".into(),
                    display_name: Some("Extra".into()),
                    ..Default::default()
                }]),
                ..Default::default()
            },
            ..sample_profile()
        };
        let list = CodexAdapter::effective_catalog_models(&profile);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].slug, "gpt-default");
        assert_eq!(list[1].slug, "gpt-extra");
        assert_eq!(list[1].display_name.as_deref(), Some("Extra"));
    }

    #[test]
    fn test_effective_catalog_dedupes_and_skips_blank() {
        let profile = ApiProfile {
            model: Some("gpt-a".into()),
            codex: CodexProfileFields {
                catalog_models: Some(vec![
                    CodexCatalogModel {
                        slug: "  ".into(),
                        display_name: None,
                        ..Default::default()
                    },
                    CodexCatalogModel {
                        slug: "gpt-a".into(),
                        display_name: Some("A".into()),
                        ..Default::default()
                    },
                    CodexCatalogModel {
                        slug: "gpt-b".into(),
                        display_name: None,
                        ..Default::default()
                    },
                    CodexCatalogModel {
                        slug: "gpt-a".into(),
                        display_name: Some("dup".into()),
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            },
            ..sample_profile()
        };
        let list = CodexAdapter::effective_catalog_models(&profile);
        assert_eq!(
            list.iter().map(|e| e.slug.as_str()).collect::<Vec<_>>(),
            vec!["gpt-a", "gpt-b"]
        );
        assert_eq!(list[0].display_name.as_deref(), Some("A"));
    }

    #[test]
    fn test_merge_sets_model_catalog_json_when_catalog_configured() {
        let adapter = CodexAdapter {
            config_dir: PathBuf::from("/tmp/helio-codex-fake"),
        };
        let profile = ApiProfile {
            model: Some("gpt-x".into()),
            codex: CodexProfileFields {
                catalog_models: Some(vec![CodexCatalogModel {
                    slug: "gpt-x".into(),
                    display_name: None,
                    ..Default::default()
                }]),
                ..Default::default()
            },
            ..sample_profile()
        };
        let merged = adapter.merge_config(&profile, &serde_json::json!({}));
        // 使用 PathBuf 期望值，避免 Windows 反斜杠与 Unix 正斜杠字面量不一致
        let expected = adapter
            .config_dir
            .join("model_catalog.json")
            .to_string_lossy()
            .into_owned();
        assert_eq!(merged["model_catalog_json"], expected);
    }

    #[test]
    fn test_merge_empty_catalog_does_not_force_pointer() {
        let adapter = CodexAdapter {
            config_dir: PathBuf::from("/tmp/helio-codex-fake"),
        };
        let shared = serde_json::json!({
            "model_catalog_json": "/existing/catalog.json"
        });
        let profile = sample_profile(); // no model, no catalog_models
        let merged = adapter.merge_config(&profile, &shared);
        assert_eq!(merged["model_catalog_json"], "/existing/catalog.json");
    }

    #[test]
    fn test_write_model_catalog_overwrites_and_preserves_slug() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
        let config_dir = std::env::temp_dir().join(format!(
            "switch-api-codex-catalog-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&config_dir).unwrap();
        // 旧 catalog 含其它 slug，应被整表覆盖
        fs::write(
            config_dir.join("model_catalog.json"),
            r#"{"models":[{"slug":"old-only","display_name":"Old","base_instructions":"KEEP_ME"}]}"#,
        )
        .unwrap();

        let adapter = CodexAdapter {
            config_dir: config_dir.clone(),
        };
        let profile = ApiProfile {
            model: Some("GPT-5.6-Sol".into()),
            context_1m: Some(true),
            codex: CodexProfileFields {
                catalog_models: Some(vec![
                    CodexCatalogModel {
                        slug: "GPT-5.6-Sol".into(),
                        display_name: Some("Sol".into()),
                        ..Default::default()
                    },
                    CodexCatalogModel {
                        slug: "extra-model".into(),
                        display_name: None,
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            },
            ..sample_profile()
        };
        adapter.write_model_catalog(&profile).unwrap();

        let written: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(config_dir.join("model_catalog.json")).unwrap(),
        )
        .unwrap();
        let models = written["models"].as_array().unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0]["slug"], "GPT-5.6-Sol"); // 原样
        assert_eq!(models[0]["display_name"], "Sol");
        assert_eq!(models[0]["context_window"], 1_000_000);
        assert_eq!(models[0]["base_instructions"], "KEEP_ME"); // 复用旧模板
        assert_eq!(models[1]["slug"], "extra-model");
        assert_eq!(models[1]["display_name"], "extra-model");
        assert_eq!(models[1]["priority"], 1);
        // old-only 消失
        assert!(models.iter().all(|m| m["slug"] != "old-only"));

        let _ = fs::remove_dir_all(&config_dir);
    }

    #[test]
    fn test_write_model_catalog_noop_when_empty() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
        let config_dir = std::env::temp_dir().join(format!(
            "switch-api-codex-catalog-empty-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&config_dir).unwrap();
        let catalog = config_dir.join("model_catalog.json");
        fs::write(&catalog, r#"{"models":[{"slug":"keep"}]}"#).unwrap();

        let adapter = CodexAdapter {
            config_dir: config_dir.clone(),
        };
        adapter.write_model_catalog(&sample_profile()).unwrap();
        let content = fs::read_to_string(&catalog).unwrap();
        assert!(content.contains("keep"));

        let _ = fs::remove_dir_all(&config_dir);
    }

    #[test]
    fn test_write_model_catalog_standard_context_when_not_1m() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
        let config_dir = std::env::temp_dir().join(format!(
            "switch-api-codex-catalog-ctx-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&config_dir).unwrap();
        let adapter = CodexAdapter {
            config_dir: config_dir.clone(),
        };
        let profile = ApiProfile {
            model: Some("m".into()),
            context_1m: Some(false),
            ..sample_profile()
        };
        adapter.write_model_catalog(&profile).unwrap();
        let written: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(config_dir.join("model_catalog.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(written["models"][0]["context_window"], 272_000);
        assert_eq!(written["models"][0]["supports_reasoning_summaries"], false);
        assert_eq!(written["models"][0]["supports_parallel_tool_calls"], false);
        assert_eq!(written["models"][0]["supports_search_tool"], false);
        assert_eq!(
            written["models"][0]["input_modalities"],
            serde_json::json!(["text"])
        );

        let _ = fs::remove_dir_all(&config_dir);
    }

    #[test]
    fn test_apply_api_credentials_writes_auth_json() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
        let config_dir = std::env::temp_dir().join(format!(
            "switch-api-codex-auth-test-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&config_dir).unwrap();
        // 预置 auth.json，含其他字段，验证被保留
        fs::write(
            config_dir.join("auth.json"),
            r#"{"OPENAI_API_KEY":"sk-old","tokens":{"refresh":"abc"}}"#,
        )
        .unwrap();

        let adapter = CodexAdapter {
            config_dir: config_dir.clone(),
        };
        adapter.apply_api_credentials(&sample_profile()).unwrap();

        let written: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(config_dir.join("auth.json")).unwrap())
                .unwrap();
        // key 被更新
        assert_eq!(written["OPENAI_API_KEY"], "sk-test-key");
        // 其他字段保留
        assert_eq!(written["tokens"]["refresh"], "abc");

        let _ = fs::remove_dir_all(&config_dir);
    }

    #[test]
    fn test_env_key_profile_does_not_modify_auth_json() {
        let dir = tempfile::tempdir().unwrap();
        let auth_path = dir.path().join("auth.json");
        fs::write(&auth_path, r#"{"OPENAI_API_KEY":"keep"}"#).unwrap();
        let adapter = CodexAdapter {
            config_dir: dir.path().to_path_buf(),
        };
        let profile = ApiProfile {
            codex: CodexProfileFields {
                env_key: Some("MY_CODEX_KEY".into()),
                ..Default::default()
            },
            ..sample_profile()
        };
        adapter.apply_api_credentials(&profile).unwrap();
        assert_eq!(
            fs::read_to_string(auth_path).unwrap(),
            r#"{"OPENAI_API_KEY":"keep"}"#
        );
    }

    #[test]
    fn test_bedrock_profile_does_not_modify_auth_json() {
        let dir = tempfile::tempdir().unwrap();
        let auth_path = dir.path().join("auth.json");
        fs::write(&auth_path, r#"{"OPENAI_API_KEY":"keep"}"#).unwrap();
        let adapter = CodexAdapter {
            config_dir: dir.path().to_path_buf(),
        };
        let profile = ApiProfile {
            provider: "amazon-bedrock".into(),
            api_url: String::new(),
            api_key: String::new(),
            target_app: Some(crate::models::TargetApp::Codex),
            ..Default::default()
        };
        adapter.apply_api_credentials(&profile).unwrap();
        assert_eq!(
            fs::read_to_string(auth_path).unwrap(),
            r#"{"OPENAI_API_KEY":"keep"}"#
        );
    }

    #[test]
    fn test_catalog_uses_explicit_model_capabilities() {
        let catalog = CodexAdapter::build_catalog_json(
            &[CodexCatalogModel {
                slug: "capable".into(),
                context_window: Some(640_000),
                reasoning_levels: Some(vec!["minimal".into(), "xhigh".into()]),
                supports_images: Some(true),
                supports_tool_calls: Some(true),
                supports_web_search: Some(true),
                ..Default::default()
            }],
            None,
            "base",
        );
        let model = &catalog["models"][0];
        assert_eq!(model["context_window"], 640_000);
        assert_eq!(model["supports_reasoning_summaries"], true);
        assert_eq!(model["default_reasoning_level"], "minimal");
        assert_eq!(
            model["supported_reasoning_levels"],
            serde_json::json!([
                {"effort": "minimal", "description": "minimal reasoning effort"},
                {"effort": "xhigh", "description": "xhigh reasoning effort"}
            ])
        );
        assert_eq!(model["supports_parallel_tool_calls"], true);
        assert_eq!(model["supports_search_tool"], true);
        assert_eq!(
            model["input_modalities"],
            serde_json::json!(["text", "image"])
        );
    }

    #[test]
    fn test_catalog_passes_through_max_ultra_and_skips_none_default() {
        let catalog = CodexAdapter::build_catalog_json(
            &[CodexCatalogModel {
                slug: "sol-like".into(),
                reasoning_levels: Some(vec![
                    "none".into(),
                    "low".into(),
                    "max".into(),
                    "ultra".into(),
                    "bogus".into(),
                ]),
                ..Default::default()
            }],
            None,
            "base",
        );
        let model = &catalog["models"][0];
        // bogus 被过滤，none/max/ultra 透传
        let efforts: Vec<&str> = model["supported_reasoning_levels"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["effort"].as_str().unwrap())
            .collect();
        assert_eq!(efforts, vec!["none", "low", "max", "ultra"]);
        // 默认值跳过 none，取第一个真实档位
        assert_eq!(model["default_reasoning_level"], "low");
        // shell 类型跟随官方 unified_exec（shell_command 已是 legacy）
        assert_eq!(model["shell_type"], "unified_exec");
    }

    #[test]
    fn test_catalog_migrates_legacy_reasoning_support_to_all_documented_levels() {
        let catalog = CodexAdapter::build_catalog_json(
            &[CodexCatalogModel {
                slug: "legacy-capable".into(),
                supports_reasoning: Some(true),
                ..Default::default()
            }],
            None,
            "base",
        );
        let model = &catalog["models"][0];

        assert_eq!(model["supports_reasoning_summaries"], true);
        assert_eq!(model["default_reasoning_level"], "minimal");
        assert_eq!(
            model["supported_reasoning_levels"],
            serde_json::json!([
                {"effort": "minimal", "description": "minimal reasoning effort"},
                {"effort": "low", "description": "low reasoning effort"},
                {"effort": "medium", "description": "medium reasoning effort"},
                {"effort": "high", "description": "high reasoning effort"},
                {"effort": "xhigh", "description": "xhigh reasoning effort"}
            ])
        );
    }

    #[test]
    fn test_validate_rejects_removed_chat_wire() {
        let adapter = CodexAdapter::new();
        let profile = ApiProfile {
            codex: CodexProfileFields {
                wire_api: Some("chat".to_string()),
                ..Default::default()
            },
            ..sample_profile()
        };
        let err = adapter.validate_profile(&profile).unwrap_err().to_string();
        assert!(err.contains("responses"), "应指引迁移到 responses: {err}");

        let unknown = ApiProfile {
            codex: CodexProfileFields {
                wire_api: Some("grpc".to_string()),
                ..Default::default()
            },
            ..sample_profile()
        };
        assert!(adapter.validate_profile(&unknown).is_err());
    }

    #[test]
    fn test_validate_rejects_bad_tiers_and_summaries() {
        let adapter = CodexAdapter::new();
        for codex in [
            CodexProfileFields {
                service_tier: Some("ultra".into()),
                ..Default::default()
            },
            CodexProfileFields {
                reasoning_summary: Some("verbose".into()),
                ..Default::default()
            },
            CodexProfileFields {
                verbosity: Some("xhigh".into()),
                ..Default::default()
            },
        ] {
            let profile = ApiProfile {
                codex,
                ..sample_profile()
            };
            assert!(adapter.validate_profile(&profile).is_err());
        }
        let ok = ApiProfile {
            codex: CodexProfileFields {
                service_tier: Some("flex".into()),
                reasoning_summary: Some("concise".into()),
                verbosity: Some("medium".into()),
                ..Default::default()
            },
            ..sample_profile()
        };
        assert!(adapter.validate_profile(&ok).is_ok());
    }

    #[test]
    fn test_merge_bearer_only_defaults_openai_auth_false() {
        let adapter = CodexAdapter::new();
        let profile = ApiProfile {
            provider: "myproxy".to_string(),
            codex: CodexProfileFields {
                experimental_bearer_token: Some("sk-bearer".to_string()),
                ..Default::default()
            },
            ..sample_profile()
        };
        let merged = adapter.merge_config(&profile, &serde_json::json!({}));
        assert_eq!(
            merged["model_providers"]["myproxy"]["requires_openai_auth"],
            false
        );
        // auth.json 模式（无 env/bearer）仍默认 true，保证 auth.json 的 key 生效。
        let merged = adapter.merge_config(&sample_profile(), &serde_json::json!({}));
        assert_eq!(
            merged["model_providers"]["openai-custom"]["requires_openai_auth"],
            true
        );
    }

    #[test]
    fn test_merge_command_auth_writes_auth_table() {
        let adapter = CodexAdapter::new();
        let profile = ApiProfile {
            provider: "myproxy".to_string(),
            codex: CodexProfileFields {
                auth_command: Some("gcloud".to_string()),
                auth_args: Some(vec!["auth".into(), "print-access-token".into()]),
                auth_timeout_ms: Some(5000),
                auth_refresh_interval_ms: Some(300000),
                ..Default::default()
            },
            ..sample_profile()
        };
        assert!(adapter.validate_profile(&profile).is_ok());
        let merged = adapter.merge_config(&profile, &serde_json::json!({}));
        let provider = &merged["model_providers"]["myproxy"];
        assert_eq!(provider["auth"]["command"], "gcloud");
        assert_eq!(
            provider["auth"]["args"],
            serde_json::json!(["auth", "print-access-token"])
        );
        assert_eq!(provider["auth"]["timeout_ms"], 5000);
        assert_eq!(provider["auth"]["refresh_interval_ms"], 300000);
        assert!(provider.get("env_key").is_none());
        assert!(provider.get("experimental_bearer_token").is_none());
        assert!(provider.get("requires_openai_auth").is_none());
        assert_eq!(provider["wire_api"], "responses");
        // 命令模式不接管 auth.json
        assert!(merged.get("cli_auth_credentials_store").is_none());
    }

    #[test]
    fn test_validate_rejects_auth_command_conflicts() {
        let adapter = CodexAdapter::new();
        for codex in [
            CodexProfileFields {
                auth_command: Some("cmd".into()),
                env_key: Some("K".into()),
                ..Default::default()
            },
            CodexProfileFields {
                auth_command: Some("cmd".into()),
                experimental_bearer_token: Some("sk-x".into()),
                ..Default::default()
            },
            CodexProfileFields {
                auth_command: Some("cmd".into()),
                requires_openai_auth: Some(true),
                ..Default::default()
            },
        ] {
            let profile = ApiProfile {
                codex,
                ..sample_profile()
            };
            assert!(adapter.validate_profile(&profile).is_err());
        }
    }

    #[test]
    fn test_merge_writes_summary_and_verbosity_without_clearing_unset() {
        let adapter = CodexAdapter::new();
        let profile = ApiProfile {
            codex: CodexProfileFields {
                reasoning_summary: Some("concise".to_string()),
                verbosity: Some("medium".to_string()),
                ..Default::default()
            },
            ..sample_profile()
        };
        let merged = adapter.merge_config(&profile, &serde_json::json!({}));
        assert_eq!(merged["model_reasoning_summary"], "concise");
        assert_eq!(merged["model_verbosity"], "medium");

        // 未管理这两个字段的 Profile 不得清除手填值。
        let shared = serde_json::json!({
            "model_reasoning_summary": "detailed",
            "model_verbosity": "low",
        });
        let merged = adapter.merge_config(&sample_profile(), &shared);
        assert_eq!(merged["model_reasoning_summary"], "detailed");
        assert_eq!(merged["model_verbosity"], "low");
    }

    #[test]
    fn test_merge_context_none_preserves_existing_window() {
        let adapter = CodexAdapter::new();
        let shared = serde_json::json!({
            "model_context_window": 128000,
            "model_auto_compact_token_limit": 100000,
        });
        // context_1m=None（不管理）→ 保留
        let merged = adapter.merge_config(&sample_profile(), &shared);
        assert_eq!(merged["model_context_window"], 128000);
        // Some(false)（显式关闭）→ 清理
        let off = ApiProfile {
            context_1m: Some(false),
            ..sample_profile()
        };
        let merged = adapter.merge_config(&off, &shared);
        assert!(merged.get("model_context_window").is_none());
        assert!(merged.get("model_auto_compact_token_limit").is_none());
    }

    #[test]
    fn test_merge_clears_cli_auth_store_when_env_key() {
        let adapter = CodexAdapter::new();
        let shared = serde_json::json!({ "cli_auth_credentials_store": "file" });
        let profile = ApiProfile {
            provider: "myproxy".to_string(),
            codex: CodexProfileFields {
                env_key: Some("MY_KEY".to_string()),
                ..Default::default()
            },
            ..sample_profile()
        };
        let merged = adapter.merge_config(&profile, &shared);
        assert!(merged.get("cli_auth_credentials_store").is_none());
    }

    #[test]
    fn test_backup_config_also_backs_up_auth_json() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
        let config_dir = std::env::temp_dir().join(format!(
            "switch-api-codex-backup-test-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&config_dir).unwrap();
        // 预置 config.toml + auth.json（含 API key 和登录态 tokens.refresh）
        fs::write(
            config_dir.join("config.toml"),
            "model_provider = \"openai-custom\"\n",
        )
        .unwrap();
        fs::write(
            config_dir.join("auth.json"),
            r#"{"OPENAI_API_KEY":"sk-secret","tokens":{"refresh":"refresh-token-xyz"}}"#,
        )
        .unwrap();

        let adapter = CodexAdapter {
            config_dir: config_dir.clone(),
        };
        let backup_path = adapter.backup_config().unwrap();

        // config 备份生成
        assert!(backup_path.exists());
        assert!(backup_path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("config.backup."));

        // auth 备份生成，内容含登录态 refresh
        let auth_backup = fs::read_dir(&config_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.file_name().to_string_lossy().starts_with("auth.backup."))
            .expect("auth backup should exist");
        let auth_content = fs::read_to_string(auth_backup.path()).unwrap();
        let auth_json: serde_json::Value = serde_json::from_str(&auth_content).unwrap();
        assert_eq!(auth_json["OPENAI_API_KEY"], "sk-secret");
        assert_eq!(auth_json["tokens"]["refresh"], "refresh-token-xyz");

        let _ = fs::remove_dir_all(&config_dir);
    }
}
