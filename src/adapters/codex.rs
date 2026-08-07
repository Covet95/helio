use super::{backup, ConfigAdapter};
use crate::models::{ApiProfile, CodexCatalogModel};
use crate::utils::secure_fs::{atomic_write_private, ensure_private_dir};
use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

/// 非 1M 时 catalog 条目默认上下文（与常见 Codex 内置条目对齐）
const CATALOG_CONTEXT_STANDARD: i64 = 272_000;
const CATALOG_CONTEXT_1M: i64 = 1_000_000;
const LEGACY_REASONING_LEVELS: &[&str] = &["minimal", "low", "medium", "high", "xhigh"];
const CODEX_REASONING_LEVELS: &[&str] = &["minimal", "low", "medium", "high", "xhigh"];

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
        let source: Vec<String> = entry
            .reasoning_levels
            .clone()
            .or_else(|| {
                entry.supports_reasoning.and_then(|enabled| {
                    enabled.then(|| {
                        LEGACY_REASONING_LEVELS
                            .iter()
                            .map(|level| (*level).to_string())
                            .collect()
                    })
                })
            })
            .unwrap_or_default();
        let mut seen = std::collections::HashSet::new();
        source
            .into_iter()
            .map(|level| level.trim().to_ascii_lowercase())
            .filter(|level| CODEX_REASONING_LEVELS.contains(&level.as_str()))
            .filter(|level| seen.insert(level.clone()))
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
                    "default_reasoning_level": if supports_reasoning {
                        Self::normalized_reasoning_levels(e)
                            .first()
                            .cloned()
                            .unwrap_or_else(|| "medium".to_string())
                    } else {
                        "none".to_string()
                    },
                    "supported_reasoning_levels": reasoning_levels,
                    "shell_type": "shell_command",
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
        // config.toml 里唯一的 API 端点/凭据信息是各 provider 的 base_url 和
        // experimental_bearer_token（第三方中转专用鉴权）。因此都要移除，
        // 保留 wire_api / requires_openai_auth / name 等协议字段。
        if let Some(obj) = shared.as_object_mut() {
            // 兼容历史版本误写入的顶层 api_key
            obj.remove("api_key");

            let active_provider = obj
                .get("model_provider")
                .and_then(|value| value.as_str())
                .map(str::to_string);
            if let (Some(active_provider), Some(providers)) = (
                active_provider,
                obj.get_mut("model_providers")
                    .and_then(|v| v.as_object_mut()),
            ) {
                if let Some(p) = providers
                    .get_mut(&active_provider)
                    .and_then(|value| value.as_object_mut())
                {
                    p.remove("base_url");
                    p.remove("env_key");
                    p.remove("experimental_bearer_token");
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
            if Self::env_key(api_profile).is_none() && api_profile.api_key.trim().is_empty() {
                anyhow::bail!("Codex custom provider requires an API key or env_key");
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

        if let Some(entries) = api_profile.codex.catalog_models.as_ref() {
            for entry in entries {
                if let Some(levels) = entry.reasoning_levels.as_ref() {
                    for level in levels {
                        let normalized = level.trim().to_ascii_lowercase();
                        if !CODEX_REASONING_LEVELS.contains(&normalized.as_str()) {
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
        let mut config = shared_config.clone();

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
            let raw_id = if api_profile.provider.is_empty() {
                "custom".to_string()
            } else {
                api_profile.provider.to_lowercase()
            };
            let provider_id = if Self::is_reserved_provider_id(&raw_id) {
                format!("{raw_id}-custom")
            } else {
                raw_id
            };

            // 写入目标 provider 配置并规范化 Responses 与鉴权模式；其他 provider 不动。
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
                    if let Some(env_key) = Self::env_key(api_profile) {
                        p.insert(
                            "env_key".to_string(),
                            serde_json::Value::String(env_key.to_string()),
                        );
                        p.insert(
                            "requires_openai_auth".to_string(),
                            serde_json::Value::Bool(false),
                        );
                    } else {
                        p.remove("env_key");
                        p.insert(
                            "requires_openai_auth".to_string(),
                            serde_json::Value::Bool(true),
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
                    p.remove("experimental_bearer_token");
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
            if !is_bedrock && Self::env_key(api_profile).is_none() {
                obj.insert(
                    "cli_auth_credentials_store".to_string(),
                    serde_json::Value::String("file".to_string()),
                );
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
                Some(false) | None => {
                    obj.remove("model_context_window");
                    obj.remove("model_auto_compact_token_limit");
                }
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

        if Self::is_amazon_bedrock_profile(api_profile) || Self::env_key(api_profile).is_some() {
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
        // 已有 provider 用 responses，profile 指定 chat + requires_openai_auth=false
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
            true
        );
    }

    #[test]
    fn test_merge_uses_provider_env_key_and_clears_legacy_bearer() {
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
        assert!(merged["model_providers"]["myproxy"]
            .get("experimental_bearer_token")
            .is_none());
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
                    reasoning_levels: Some(vec!["ultra".into()]),
                    ..Default::default()
                }]),
                ..Default::default()
            },
            ..sample_profile()
        };
        assert!(adapter.validate_profile(&profile).is_err());
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
