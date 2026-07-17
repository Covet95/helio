use super::ConfigAdapter;
use crate::models::ApiProfile;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// OpenClaw adapter — `~/.openclaw/openclaw.json` (JSON).
///
/// MVP: custom `models.providers.<id>` + `agents.defaults.model.primary`.
pub struct OpenClawAdapter {
    config_dir: PathBuf,
}

impl OpenClawAdapter {
    pub fn new() -> Self {
        let home = dirs::home_dir().expect("Failed to get home directory");
        Self {
            config_dir: home.join(".openclaw"),
        }
    }

    #[cfg(test)]
    fn with_dir(config_dir: PathBuf) -> Self {
        Self { config_dir }
    }

    fn config_file_path(&self) -> PathBuf {
        self.config_dir.join("openclaw.json")
    }

    fn agent_models_path(&self) -> PathBuf {
        self.config_dir
            .join("agents")
            .join("main")
            .join("agent")
            .join("models.json")
    }

    pub fn normalize_provider_name(provider: &str) -> String {
        let s = provider.trim().to_lowercase().replace(' ', "-");
        if s.is_empty() {
            "custom".to_string()
        } else {
            s
        }
    }

    /// Map Helio api_mode → OpenClaw `api` field.
    pub fn map_api_field(api_mode: Option<&str>) -> String {
        match api_mode.map(str::trim).filter(|s| !s.is_empty()) {
            None | Some("chat_completions") | Some("openai-completions") => {
                "openai-completions".into()
            }
            Some("anthropic_messages") => "anthropic-messages".into(),
            Some("openai-responses") | Some("codex_responses") | Some("responses") => {
                "openai-responses".into()
            }
            Some(other) => other.to_string(),
        }
    }

    fn profile_api_mode(profile: &ApiProfile) -> Option<&str> {
        // OpenClaw-owned field only — never read Hermes.
        profile.openclaw.api_mode.as_deref()
    }

    /// Resolve contextWindow for a model entry.
    /// - context_1m=true  → 1_000_000
    /// - context_1m=false → Grok 500_000 / others 200_000 (model-aware standard)
    /// - None             → keep existing if any, else 1_000_000 (safe default for custom)
    pub fn resolve_context_window(profile: &ApiProfile, existing: Option<i64>) -> i64 {
        use crate::models::CONTEXT_LENGTH_1M;
        match profile.context_1m {
            Some(true) => CONTEXT_LENGTH_1M,
            Some(false) => profile.standard_context_length(),
            None => existing.filter(|&v| v > 0).unwrap_or(CONTEXT_LENGTH_1M),
        }
    }

    /// Resolve maxTokens for a model entry.
    /// - profile.openclaw.max_tokens if set and > 0
    /// - else keep existing if any
    /// - else 128_000 (not the old hard-coded 8192)
    pub fn resolve_max_tokens(profile: &ApiProfile, existing: Option<i64>) -> i64 {
        if let Some(n) = profile.openclaw.max_tokens {
            if n > 0 {
                return n;
            }
        }
        existing.filter(|&v| v > 0).unwrap_or(128_000)
    }

    /// Upsert the selected model into providers.<id>.models[] and patch
    /// contextWindow / maxTokens / api on the matching entry.
    fn upsert_model_entry(
        models_arr: &mut Vec<serde_json::Value>,
        mid: &str,
        api: &str,
        context_window: i64,
        max_tokens: i64,
    ) {
        let mut found = false;
        for m in models_arr.iter_mut() {
            let Some(obj) = m.as_object_mut() else {
                continue;
            };
            let id = obj.get("id").and_then(|v| v.as_str()).unwrap_or("");
            if id != mid {
                continue;
            }
            obj.insert("api".into(), serde_json::Value::String(api.to_string()));
            obj.insert(
                "contextWindow".into(),
                serde_json::Value::Number(context_window.into()),
            );
            obj.insert(
                "maxTokens".into(),
                serde_json::Value::Number(max_tokens.into()),
            );
            found = true;
            break;
        }
        if !found {
            models_arr.push(serde_json::json!({
                "id": mid,
                "name": mid,
                "api": api,
                "reasoning": true,
                "input": ["text", "image"],
                "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
                "contextWindow": context_window,
                "maxTokens": max_tokens
            }));
        }
    }

    /// Apply profile onto openclaw.json document.
    pub fn apply_profile_to_config(
        config: &serde_json::Value,
        api_profile: &ApiProfile,
    ) -> serde_json::Value {
        let mut cfg = if config.is_object() {
            config.clone()
        } else {
            serde_json::json!({})
        };

        let provider = Self::normalize_provider_name(&api_profile.provider);
        let api = Self::map_api_field(Self::profile_api_mode(api_profile));
        let model_id = api_profile
            .model
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());

        // models.providers.<provider>
        {
            let root = cfg.as_object_mut().unwrap();
            let models = root
                .entry("models".to_string())
                .or_insert_with(|| serde_json::json!({}));
            if !models.is_object() {
                *models = serde_json::json!({});
            }
            let models_obj = models.as_object_mut().unwrap();
            // keep mode if present; default replace for custom-only installs
            if !models_obj.contains_key("mode") {
                models_obj.insert(
                    "mode".into(),
                    serde_json::Value::String("replace".into()),
                );
            }
            let providers = models_obj
                .entry("providers".to_string())
                .or_insert_with(|| serde_json::json!({}));
            if !providers.is_object() {
                *providers = serde_json::json!({});
            }
            let providers_obj = providers.as_object_mut().unwrap();

            let mut entry = match providers_obj.get(&provider) {
                Some(e) if e.is_object() => e.clone(),
                _ => serde_json::json!({}),
            };
            {
                let e = entry.as_object_mut().unwrap();
                e.insert(
                    "baseUrl".into(),
                    serde_json::Value::String(api_profile.api_url.clone()),
                );
                e.insert(
                    "apiKey".into(),
                    serde_json::Value::String(api_profile.api_key.clone()),
                );
                e.insert("api".into(), serde_json::Value::String(api.clone()));

                // Ensure models[] has the selected model and patch context/maxTokens
                if let Some(mid) = model_id {
                    let mut models_arr = match e.get("models") {
                        Some(m) if m.is_array() => m.as_array().unwrap().clone(),
                        _ => Vec::new(),
                    };
                    let existing_cw = models_arr.iter().find_map(|m| {
                        let id = m.get("id").and_then(|v| v.as_str())?;
                        if id == mid {
                            m.get("contextWindow").and_then(|v| v.as_i64())
                        } else {
                            None
                        }
                    });
                    let existing_mt = models_arr.iter().find_map(|m| {
                        let id = m.get("id").and_then(|v| v.as_str())?;
                        if id == mid {
                            m.get("maxTokens").and_then(|v| v.as_i64())
                        } else {
                            None
                        }
                    });
                    let cw = Self::resolve_context_window(api_profile, existing_cw);
                    let mt = Self::resolve_max_tokens(api_profile, existing_mt);
                    Self::upsert_model_entry(&mut models_arr, mid, &api, cw, mt);
                    e.insert("models".into(), serde_json::Value::Array(models_arr));
                } else if e.get("models").is_none() {
                    e.insert("models".into(), serde_json::json!([]));
                }
            }
            providers_obj.insert(provider.clone(), entry);
        }

        // agents.defaults.model.primary + optional contextTokens
        if let Some(mid) = model_id {
            let primary = format!("{}/{}", provider, mid);
            let root = cfg.as_object_mut().unwrap();
            let agents = root
                .entry("agents".to_string())
                .or_insert_with(|| serde_json::json!({}));
            if !agents.is_object() {
                *agents = serde_json::json!({});
            }
            let agents_obj = agents.as_object_mut().unwrap();
            let defaults = agents_obj
                .entry("defaults".to_string())
                .or_insert_with(|| serde_json::json!({}));
            if !defaults.is_object() {
                *defaults = serde_json::json!({});
            }
            let defaults_obj = defaults.as_object_mut().unwrap();
            let model = defaults_obj
                .entry("model".to_string())
                .or_insert_with(|| serde_json::json!({}));
            if !model.is_object() {
                // was a string primary historically
                *model = serde_json::json!({});
            }
            model.as_object_mut().unwrap().insert(
                "primary".into(),
                serde_json::Value::String(primary.clone()),
            );

            // ensure agents.defaults.models map has the key (empty object ok)
            let models_map = defaults_obj
                .entry("models".to_string())
                .or_insert_with(|| serde_json::json!({}));
            if models_map.is_object() {
                let mm = models_map.as_object_mut().unwrap();
                if !mm.contains_key(&primary) {
                    mm.insert(primary, serde_json::json!({}));
                }
            }

            // Sync agents.defaults.contextTokens when profile expresses context intent.
            // Only write when context_1m is Some, so we don't clobber user tuning
            // on bare API-key-only switches.
            if api_profile.context_1m.is_some() {
                let existing_ct = defaults_obj
                    .get("contextTokens")
                    .and_then(|v| v.as_i64());
                let ct = Self::resolve_context_window(api_profile, existing_ct);
                defaults_obj.insert(
                    "contextTokens".into(),
                    serde_json::Value::Number(ct.into()),
                );
            }
        }

        cfg
    }

    fn write_json_pretty(path: &Path, config: &serde_json::Value) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).context("Failed to create OpenClaw config directory")?;
        }
        let content = serde_json::to_string_pretty(config)
            .context("Failed to serialize OpenClaw config")?;
        let temp = path.with_extension("json.tmp");
        fs::write(&temp, format!("{}\n", content)).context("Failed to write temp OpenClaw config")?;
        if let Ok(file) = fs::File::open(&temp) {
            let _ = file.sync_all();
        }
        fs::rename(&temp, path).context("Failed to rename temp OpenClaw config")?;
        Ok(())
    }

    /// Sync provider entry into agents/main/agent/models.json if present.
    fn sync_agent_models_json(&self, api_profile: &ApiProfile) -> Result<()> {
        let path = self.agent_models_path();
        if !path.exists() {
            return Ok(());
        }
        let content = fs::read_to_string(&path).context("Failed to read agent models.json")?;
        let mut data: serde_json::Value =
            serde_json::from_str(&content).context("Failed to parse agent models.json")?;
        if !data.is_object() {
            data = serde_json::json!({});
        }

        let provider = Self::normalize_provider_name(&api_profile.provider);
        let api = Self::map_api_field(Self::profile_api_mode(api_profile));
        let model_id = api_profile
            .model
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let providers = data
            .as_object_mut()
            .unwrap()
            .entry("providers".to_string())
            .or_insert_with(|| serde_json::json!({}));
        if !providers.is_object() {
            *providers = serde_json::json!({});
        }
        let providers_obj = providers.as_object_mut().unwrap();
        let mut entry = match providers_obj.get(&provider) {
            Some(e) if e.is_object() => e.clone(),
            _ => serde_json::json!({ "models": [] }),
        };
        if let Some(e) = entry.as_object_mut() {
            e.insert(
                "baseUrl".into(),
                serde_json::Value::String(api_profile.api_url.clone()),
            );
            e.insert(
                "apiKey".into(),
                serde_json::Value::String(api_profile.api_key.clone()),
            );
            e.insert("api".into(), serde_json::Value::String(api.clone()));
            if let Some(mid) = model_id {
                let mut models_arr = match e.get("models") {
                    Some(m) if m.is_array() => m.as_array().unwrap().clone(),
                    _ => Vec::new(),
                };
                let existing_cw = models_arr.iter().find_map(|m| {
                    let id = m.get("id").and_then(|v| v.as_str())?;
                    if id == mid {
                        m.get("contextWindow").and_then(|v| v.as_i64())
                    } else {
                        None
                    }
                });
                let existing_mt = models_arr.iter().find_map(|m| {
                    let id = m.get("id").and_then(|v| v.as_str())?;
                    if id == mid {
                        m.get("maxTokens").and_then(|v| v.as_i64())
                    } else {
                        None
                    }
                });
                let cw = Self::resolve_context_window(api_profile, existing_cw);
                let mt = Self::resolve_max_tokens(api_profile, existing_mt);
                Self::upsert_model_entry(&mut models_arr, mid, &api, cw, mt);
                e.insert("models".into(), serde_json::Value::Array(models_arr));
            }
        }
        providers_obj.insert(provider, entry);
        Self::write_json_pretty(&path, &data)
    }

    fn cleanup_backup_prefix(&self, prefix: &str, keep: usize) -> Result<()> {
        let mut backups: Vec<_> = fs::read_dir(&self.config_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.starts_with(prefix) && name.ends_with(".json")
            })
            .collect();
        backups.sort_by_key(|e| {
            e.metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        });
        backups.reverse();
        for entry in backups.iter().skip(keep) {
            let _ = fs::remove_file(entry.path());
        }
        Ok(())
    }
}

impl ConfigAdapter for OpenClawAdapter {
    fn config_path(&self) -> PathBuf {
        self.config_file_path()
    }

    fn read_config(&self) -> Result<serde_json::Value> {
        let path = self.config_file_path();
        if !path.exists() {
            return Ok(serde_json::json!({}));
        }
        let content = fs::read_to_string(&path).context("Failed to read openclaw.json")?;
        serde_json::from_str(&content).context("Failed to parse openclaw.json")
    }

    fn extract_shared_config(&self, config: &serde_json::Value) -> serde_json::Value {
        config.clone()
    }

    fn merge_config(
        &self,
        api_profile: &ApiProfile,
        shared_config: &serde_json::Value,
    ) -> serde_json::Value {
        Self::apply_profile_to_config(shared_config, api_profile)
    }

    fn write_config(&self, config: &serde_json::Value) -> Result<()> {
        Self::write_json_pretty(&self.config_file_path(), config)
    }

    fn backup_config(&self) -> Result<PathBuf> {
        let path = self.config_file_path();
        if !path.exists() {
            anyhow::bail!("Config file does not exist");
        }
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let backup_path = self
            .config_dir
            .join(format!("openclaw.backup.{}.json", timestamp));
        fs::copy(&path, &backup_path).context("Failed to backup openclaw.json")?;

        let agent_models = self.agent_models_path();
        if agent_models.exists() {
            let bak = self.config_dir.join(format!(
                "models.backup.{}.json",
                timestamp
            ));
            // store under config_dir root for simple cleanup
            let _ = fs::copy(&agent_models, &bak);
        }

        self.cleanup_old_backups(10)?;
        Ok(backup_path)
    }

    fn cleanup_old_backups(&self, keep: usize) -> Result<()> {
        if !self.config_dir.exists() {
            return Ok(());
        }
        // openclaw.json backups + agent models.json backups written to config_dir root
        self.cleanup_backup_prefix("openclaw.backup.", keep)?;
        self.cleanup_backup_prefix("models.backup.", keep)?;
        Ok(())
    }

    fn apply_api_credentials(&self, api_profile: &ApiProfile) -> Result<()> {
        self.sync_agent_models_json(api_profile)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::OpenClawProfileFields;

    fn sample() -> ApiProfile {
        ApiProfile {
            name: "cpa".into(),
            provider: "CPA".into(),
            api_url: "http://127.0.0.1:9999/v1".into(),
            api_key: "sk-new".into(),
            model: Some("claude-opus-4-8".into()),
            openclaw: OpenClawProfileFields {
                api_mode: Some("chat_completions".into()),
                max_tokens: None,
            },
            ..Default::default()
        }
    }

    #[test]
    fn map_api_defaults_to_openai_completions() {
        assert_eq!(
            OpenClawAdapter::map_api_field(None),
            "openai-completions"
        );
        assert_eq!(
            OpenClawAdapter::map_api_field(Some("anthropic_messages")),
            "anthropic-messages"
        );
    }

    #[test]
    fn merge_upserts_provider_and_primary_preserves_mcp() {
        let shared = serde_json::json!({
            "models": {
                "mode": "replace",
                "providers": {
                    "cpa": {
                        "baseUrl": "http://127.0.0.1:8317/v1",
                        "apiKey": "old",
                        "api": "openai-completions",
                        "models": [
                            { "id": "claude-opus-4-8", "name": "Opus" }
                        ]
                    }
                }
            },
            "agents": {
                "defaults": {
                    "model": {
                        "primary": "cpa/old-model",
                        "fallbacks": ["cpa/gpt-5.5"]
                    },
                    "models": { "cpa/old-model": {} }
                }
            },
            "mcp": { "servers": { "x": { "command": "uvx" } } },
            "channels": { "feishu": { "enabled": true } }
        });
        let merged = OpenClawAdapter::apply_profile_to_config(&shared, &sample());
        assert_eq!(
            merged["models"]["providers"]["cpa"]["baseUrl"],
            "http://127.0.0.1:9999/v1"
        );
        assert_eq!(merged["models"]["providers"]["cpa"]["apiKey"], "sk-new");
        assert_eq!(
            merged["agents"]["defaults"]["model"]["primary"],
            "cpa/claude-opus-4-8"
        );
        // fallbacks preserved
        assert_eq!(
            merged["agents"]["defaults"]["model"]["fallbacks"][0],
            "cpa/gpt-5.5"
        );
        // existing model catalog preserved
        assert_eq!(
            merged["models"]["providers"]["cpa"]["models"][0]["id"],
            "claude-opus-4-8"
        );
        assert_eq!(merged["mcp"]["servers"]["x"]["command"], "uvx");
        assert_eq!(merged["channels"]["feishu"]["enabled"], true);
    }

    #[test]
    fn merge_inserts_new_provider() {
        let shared = serde_json::json!({ "mcp": {} });
        let p = ApiProfile {
            name: "m".into(),
            provider: "muapi".into(),
            api_url: "https://ai.muapi.cn".into(),
            api_key: "sk-x".into(),
            model: Some("grok-4.5".into()),
            openclaw: OpenClawProfileFields {
                api_mode: Some("anthropic_messages".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let merged = OpenClawAdapter::apply_profile_to_config(&shared, &p);
        assert_eq!(
            merged["models"]["providers"]["muapi"]["api"],
            "anthropic-messages"
        );
        assert_eq!(
            merged["agents"]["defaults"]["model"]["primary"],
            "muapi/grok-4.5"
        );
        // default maxTokens is 128k (not 8192)
        assert_eq!(
            merged["models"]["providers"]["muapi"]["models"][0]["maxTokens"],
            128_000
        );
        assert_eq!(
            merged["models"]["providers"]["muapi"]["models"][0]["contextWindow"],
            1_000_000
        );
        assert_eq!(merged["mcp"], serde_json::json!({}));
    }

    #[test]
    fn merge_updates_existing_model_context_and_max_tokens() {
        let shared = serde_json::json!({
            "models": {
                "providers": {
                    "cpa": {
                        "baseUrl": "http://old",
                        "apiKey": "old",
                        "api": "openai-completions",
                        "models": [{
                            "id": "claude-opus-4-8",
                            "name": "Opus",
                            "api": "openai-completions",
                            "contextWindow": 200000,
                            "maxTokens": 8192
                        }]
                    }
                }
            },
            "agents": {
                "defaults": {
                    "model": { "primary": "cpa/claude-opus-4-8" },
                    "contextTokens": 200000
                }
            }
        });
        let p = ApiProfile {
            name: "c".into(),
            provider: "cpa".into(),
            api_url: "http://127.0.0.1:9999/v1".into(),
            api_key: "sk-new".into(),
            model: Some("claude-opus-4-8".into()),
            context_1m: Some(true),
            openclaw: OpenClawProfileFields {
                max_tokens: Some(65536),
                ..Default::default()
            },
            ..Default::default()
        };
        let merged = OpenClawAdapter::apply_profile_to_config(&shared, &p);
        let m = &merged["models"]["providers"]["cpa"]["models"][0];
        assert_eq!(m["contextWindow"], 1_000_000);
        assert_eq!(m["maxTokens"], 65536);
        assert_eq!(m["id"], "claude-opus-4-8");
        // context_1m Some → sync agents.defaults.contextTokens
        assert_eq!(merged["agents"]["defaults"]["contextTokens"], 1_000_000);
    }

    #[test]
    fn context_1m_none_does_not_clobber_context_tokens() {
        let shared = serde_json::json!({
            "models": { "providers": {} },
            "agents": { "defaults": { "contextTokens": 777777 } }
        });
        let p = ApiProfile {
            name: "c".into(),
            provider: "cpa".into(),
            api_url: "http://x".into(),
            api_key: "k".into(),
            model: Some("m1".into()),
            context_1m: None,
            ..Default::default()
        };
        let merged = OpenClawAdapter::apply_profile_to_config(&shared, &p);
        assert_eq!(merged["agents"]["defaults"]["contextTokens"], 777777);
    }

    #[test]
    fn resolve_helpers() {
        let p = ApiProfile {
            context_1m: Some(true),
            openclaw: OpenClawProfileFields {
                max_tokens: Some(32000),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(OpenClawAdapter::resolve_context_window(&p, Some(1)), 1_000_000);
        assert_eq!(OpenClawAdapter::resolve_max_tokens(&p, Some(8)), 32000);
        // Non-Grok + 1M off → standard 200k (ignores previous existing value)
        let p2 = ApiProfile {
            model: Some("claude-opus-4-8".into()),
            context_1m: Some(false),
            openclaw: OpenClawProfileFields::default(),
            ..Default::default()
        };
        assert_eq!(
            OpenClawAdapter::resolve_context_window(&p2, Some(128000)),
            200_000
        );
        assert_eq!(OpenClawAdapter::resolve_max_tokens(&p2, None), 128_000);
        // Grok + 1M off → 500k
        let p3 = ApiProfile {
            model: Some("grok-4.5".into()),
            context_1m: Some(false),
            openclaw: OpenClawProfileFields::default(),
            ..Default::default()
        };
        assert_eq!(
            OpenClawAdapter::resolve_context_window(&p3, Some(1_000_000)),
            500_000
        );
    }

    #[test]
    fn write_read_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let adapter = OpenClawAdapter::with_dir(dir.path().to_path_buf());
        let cfg = OpenClawAdapter::apply_profile_to_config(&serde_json::json!({}), &sample());
        adapter.write_config(&cfg).unwrap();
        let back = adapter.read_config().unwrap();
        assert_eq!(
            back["agents"]["defaults"]["model"]["primary"],
            "cpa/claude-opus-4-8"
        );
    }

    #[test]
    fn sync_agent_models_json_updates_provider() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().join("agents/main/agent");
        fs::create_dir_all(&agent_dir).unwrap();
        fs::write(
            agent_dir.join("models.json"),
            r#"{"providers":{"cpa":{"baseUrl":"http://old","apiKey":"old","api":"openai-completions","models":[{"id":"claude-opus-4-8","maxTokens":100}]}}}"#,
        )
        .unwrap();
        let adapter = OpenClawAdapter::with_dir(dir.path().to_path_buf());
        let mut p = sample();
        p.openclaw.max_tokens = Some(64000);
        p.context_1m = Some(true);
        adapter.sync_agent_models_json(&p).unwrap();
        let data: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(agent_dir.join("models.json")).unwrap())
                .unwrap();
        assert_eq!(
            data["providers"]["cpa"]["baseUrl"],
            "http://127.0.0.1:9999/v1"
        );
        assert_eq!(data["providers"]["cpa"]["apiKey"], "sk-new");
        assert_eq!(
            data["providers"]["cpa"]["models"][0]["maxTokens"],
            64000
        );
        assert_eq!(
            data["providers"]["cpa"]["models"][0]["contextWindow"],
            1_000_000
        );
    }
}
