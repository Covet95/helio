use super::ConfigAdapter;
use crate::models::ApiProfile;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Hermes Agent adapter — `~/.hermes/config.yaml` (YAML).
///
/// MVP: custom OpenAI-compatible endpoints only.
/// Switch surface: `model.{default,provider,api_mode}` + upsert `custom_providers[]`.
pub struct HermesAdapter {
    config_dir: PathBuf,
}

impl HermesAdapter {
    pub fn new() -> Self {
        let home = dirs::home_dir().expect("Failed to get home directory");
        Self {
            config_dir: home.join(".hermes"),
        }
    }

    #[cfg(test)]
    fn with_dir(config_dir: PathBuf) -> Self {
        Self { config_dir }
    }

    fn config_file_path(&self) -> PathBuf {
        self.config_dir.join("config.yaml")
    }

    fn auth_path(&self) -> PathBuf {
        self.config_dir.join("auth.json")
    }

    /// Normalize custom provider name the way Hermes does: lower, spaces→`-`, strip `custom:`.
    pub fn normalize_provider_name(provider: &str) -> String {
        let s = provider.trim().to_lowercase();
        let s = s
            .strip_prefix("custom:")
            .unwrap_or(s.as_str())
            .trim()
            .replace(' ', "-");
        if s.is_empty() {
            "custom".to_string()
        } else {
            s
        }
    }

    pub fn custom_provider_slug(provider: &str) -> String {
        format!("custom:{}", Self::normalize_provider_name(provider))
    }

    fn api_mode(profile: &ApiProfile) -> String {
        profile
            .hermes
            .api_mode
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("chat_completions")
            .to_string()
    }

    /// Hermes-only context_length resolution (does not share OpenClaw helpers).
    pub fn resolve_context_length(profile: &ApiProfile, existing: Option<i64>) -> Option<i64> {
        match profile.context_1m {
            Some(true) => Some(1_000_000),
            Some(false) => Some(existing.filter(|&v| v > 0).unwrap_or(200_000)),
            None => None, // do not clobber
        }
    }

    /// Apply profile onto a config document (JSON-shaped Value from YAML).
    pub fn apply_profile_to_config(
        config: &serde_json::Value,
        api_profile: &ApiProfile,
    ) -> serde_json::Value {
        let mut cfg = config.clone();
        if !cfg.is_object() {
            cfg = serde_json::json!({});
        }

        let name = Self::normalize_provider_name(&api_profile.provider);
        let slug = format!("custom:{}", name);
        let mode = Self::api_mode(api_profile);

        // model section — must be a mapping
        let mut model = match cfg.get("model") {
            Some(m) if m.is_object() => m.clone(),
            _ => serde_json::json!({}),
        };
        let existing_model_ctx = model
            .get("context_length")
            .and_then(|v| v.as_i64());
        if let Some(obj) = model.as_object_mut() {
            if let Some(ref mid) = api_profile.model {
                if !mid.is_empty() {
                    obj.insert("default".into(), serde_json::Value::String(mid.clone()));
                }
            }
            obj.insert("provider".into(), serde_json::Value::String(slug));
            // Named custom providers keep endpoint on custom_providers; clear model.base_url.
            obj.insert("base_url".into(), serde_json::Value::String(String::new()));
            obj.insert("api_mode".into(), serde_json::Value::String(mode.clone()));
            if let Some(ctx) = Self::resolve_context_length(api_profile, existing_model_ctx) {
                obj.insert(
                    "context_length".into(),
                    serde_json::Value::Number(ctx.into()),
                );
            }
        }
        cfg.as_object_mut()
            .unwrap()
            .insert("model".into(), model);

        // upsert custom_providers
        let mut providers = match cfg.get("custom_providers") {
            Some(p) if p.is_array() => p.clone(),
            _ => serde_json::json!([]),
        };
        let arr = providers.as_array_mut().unwrap();
        let mut found = false;
        for entry in arr.iter_mut() {
            let Some(obj) = entry.as_object_mut() else {
                continue;
            };
            let ename = obj
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if Self::normalize_provider_name(ename) != name {
                continue;
            }
            let existing_entry_ctx = obj.get("context_length").and_then(|v| v.as_i64());
            obj.insert(
                "base_url".into(),
                serde_json::Value::String(api_profile.api_url.clone()),
            );
            obj.insert(
                "api_key".into(),
                serde_json::Value::String(api_profile.api_key.clone()),
            );
            obj.insert("api_mode".into(), serde_json::Value::String(mode.clone()));
            if let Some(ref mid) = api_profile.model {
                if !mid.is_empty() {
                    obj.insert("model".into(), serde_json::Value::String(mid.clone()));
                }
            }
            if let Some(ctx) = Self::resolve_context_length(api_profile, existing_entry_ctx) {
                obj.insert(
                    "context_length".into(),
                    serde_json::Value::Number(ctx.into()),
                );
            }
            // Keep existing extra_body / models map etc.
            found = true;
            break;
        }
        if !found {
            let mut entry = serde_json::Map::new();
            entry.insert("name".into(), serde_json::Value::String(name));
            entry.insert(
                "base_url".into(),
                serde_json::Value::String(api_profile.api_url.clone()),
            );
            entry.insert(
                "api_key".into(),
                serde_json::Value::String(api_profile.api_key.clone()),
            );
            entry.insert("api_mode".into(), serde_json::Value::String(mode));
            if let Some(ref mid) = api_profile.model {
                if !mid.is_empty() {
                    entry.insert("model".into(), serde_json::Value::String(mid.clone()));
                }
            }
            if let Some(ctx) = Self::resolve_context_length(api_profile, None) {
                entry.insert(
                    "context_length".into(),
                    serde_json::Value::Number(ctx.into()),
                );
            }
            arr.push(serde_json::Value::Object(entry));
        }
        cfg.as_object_mut()
            .unwrap()
            .insert("custom_providers".into(), providers);

        cfg
    }

    fn write_yaml(path: &Path, config: &serde_json::Value) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).context("Failed to create Hermes config directory")?;
        }
        let content =
            serde_yaml::to_string(config).context("Failed to serialize Hermes config.yaml")?;
        let temp_path = path.with_extension("yaml.tmp");
        fs::write(&temp_path, &content).context("Failed to write temp Hermes config.yaml")?;
        if let Ok(file) = fs::File::open(&temp_path) {
            let _ = file.sync_all();
        }
        fs::rename(&temp_path, path).context("Failed to rename temp Hermes config.yaml")?;
        Ok(())
    }

    /// Update credential_pool entries that already hold access_token so they don't shadow new keys.
    fn sync_auth_pool(&self, provider: &str, api_key: &str, base_url: &str) -> Result<()> {
        let path = self.auth_path();
        if !path.exists() {
            return Ok(());
        }
        let content = fs::read_to_string(&path).context("Failed to read Hermes auth.json")?;
        let mut data: serde_json::Value =
            serde_json::from_str(&content).context("Failed to parse Hermes auth.json")?;

        let pool_key = Self::custom_provider_slug(provider);
        let Some(pool) = data.get_mut("credential_pool") else {
            return Ok(());
        };
        let Some(arr) = pool.get_mut(&pool_key).and_then(|v| v.as_array_mut()) else {
            return Ok(());
        };

        let mut changed = false;
        for entry in arr.iter_mut() {
            let Some(obj) = entry.as_object_mut() else {
                continue;
            };
            let has_token = obj
                .get("access_token")
                .and_then(|v| v.as_str())
                .map(|s| !s.is_empty())
                .unwrap_or(false);
            if has_token {
                obj.insert(
                    "access_token".into(),
                    serde_json::Value::String(api_key.to_string()),
                );
                obj.insert(
                    "base_url".into(),
                    serde_json::Value::String(base_url.to_string()),
                );
                changed = true;
            }
        }

        if !changed {
            return Ok(());
        }

        let out =
            serde_json::to_string_pretty(&data).context("Failed to serialize Hermes auth.json")?;
        let temp = path.with_extension("json.tmp");
        fs::write(&temp, &out).context("Failed to write temp Hermes auth.json")?;
        if let Ok(file) = fs::File::open(&temp) {
            let _ = file.sync_all();
        }
        fs::rename(&temp, &path).context("Failed to rename temp Hermes auth.json")?;
        Ok(())
    }
}

impl ConfigAdapter for HermesAdapter {
    fn config_path(&self) -> PathBuf {
        self.config_file_path()
    }

    fn read_config(&self) -> Result<serde_json::Value> {
        let path = self.config_file_path();
        if !path.exists() {
            return Ok(serde_json::json!({}));
        }
        let content = fs::read_to_string(&path).context("Failed to read Hermes config.yaml")?;
        let value: serde_json::Value =
            serde_yaml::from_str(&content).context("Failed to parse Hermes config.yaml")?;
        Ok(value)
    }

    fn extract_shared_config(&self, config: &serde_json::Value) -> serde_json::Value {
        // Full document is shared; merge overwrites only API surface fields.
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
        Self::write_yaml(&self.config_file_path(), config)
    }

    fn backup_config(&self) -> Result<PathBuf> {
        let path = self.config_file_path();
        if !path.exists() {
            anyhow::bail!("Config file does not exist");
        }
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let backup_path = self
            .config_dir
            .join(format!("config.backup.{}.yaml", timestamp));
        fs::copy(&path, &backup_path).context("Failed to backup Hermes config.yaml")?;

        let auth = self.auth_path();
        if auth.exists() {
            let auth_backup = self
                .config_dir
                .join(format!("auth.backup.{}.json", timestamp));
            let _ = fs::copy(&auth, &auth_backup);
        }

        self.cleanup_old_backups(10)?;
        Ok(backup_path)
    }

    fn cleanup_old_backups(&self, keep: usize) -> Result<()> {
        if !self.config_dir.exists() {
            return Ok(());
        }
        let mut backups: Vec<_> = fs::read_dir(&self.config_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.starts_with("config.backup.") && name.ends_with(".yaml")
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

    fn apply_api_credentials(&self, api_profile: &ApiProfile) -> Result<()> {
        self.sync_auth_pool(
            &api_profile.provider,
            &api_profile.api_key,
            &api_profile.api_url,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::HermesProfileFields;

    fn sample_profile() -> ApiProfile {
        ApiProfile {
            name: "fm".into(),
            provider: "Free Model".into(),
            api_url: "https://api.freemodel.dev/v1".into(),
            api_key: "sk-new".into(),
            model: Some("gpt-5.5".into()),
            hermes: HermesProfileFields {
                api_mode: Some("chat_completions".into()),
            },
            ..Default::default()
        }
    }

    #[test]
    fn normalize_strips_custom_prefix_and_spaces() {
        assert_eq!(
            HermesAdapter::normalize_provider_name("custom:Free Model"),
            "free-model"
        );
        assert_eq!(
            HermesAdapter::normalize_provider_name("cpa"),
            "cpa"
        );
        assert_eq!(
            HermesAdapter::custom_provider_slug("CPA"),
            "custom:cpa"
        );
    }

    #[test]
    fn merge_upserts_custom_provider_and_preserves_mcp() {
        let shared = serde_json::json!({
            "model": {
                "default": "old",
                "provider": "custom:cpa"
            },
            "custom_providers": [
                {
                    "name": "cpa",
                    "base_url": "http://127.0.0.1:8317/v1",
                    "api_key": "old-key",
                    "api_mode": "chat_completions",
                    "extra_body": { "reasoning": { "enabled": true } }
                }
            ],
            "mcp_servers": {
                "cdp-bridge": { "enabled": true, "command": "uvx" }
            },
            "agent": { "max_turns": 60 }
        });
        let profile = ApiProfile {
            name: "c".into(),
            provider: "cpa".into(),
            api_url: "http://127.0.0.1:9999/v1".into(),
            api_key: "new-key".into(),
            model: Some("claude-opus-4-8".into()),
            ..Default::default()
        };
        let merged = HermesAdapter::apply_profile_to_config(&shared, &profile);

        assert_eq!(merged["mcp_servers"]["cdp-bridge"]["command"], "uvx");
        assert_eq!(merged["agent"]["max_turns"], 60);
        assert_eq!(merged["model"]["default"], "claude-opus-4-8");
        assert_eq!(merged["model"]["provider"], "custom:cpa");
        assert_eq!(merged["custom_providers"][0]["api_key"], "new-key");
        assert_eq!(
            merged["custom_providers"][0]["base_url"],
            "http://127.0.0.1:9999/v1"
        );
        // preserved non-API fields on entry
        assert_eq!(
            merged["custom_providers"][0]["extra_body"]["reasoning"]["enabled"],
            true
        );
    }

    #[test]
    fn merge_appends_missing_provider() {
        let shared = serde_json::json!({
            "custom_providers": [],
            "skills": { "creation_nudge_interval": 15 }
        });
        let merged = HermesAdapter::apply_profile_to_config(&shared, &sample_profile());
        assert_eq!(merged["model"]["provider"], "custom:free-model");
        assert_eq!(merged["custom_providers"].as_array().unwrap().len(), 1);
        assert_eq!(merged["custom_providers"][0]["name"], "free-model");
        assert_eq!(merged["skills"]["creation_nudge_interval"], 15);
    }

    #[test]
    fn context_1m_writes_model_and_provider_context_length() {
        let shared = serde_json::json!({
            "model": {
                "default": "old",
                "provider": "custom:cpa",
                "context_length": 200000
            },
            "custom_providers": [{
                "name": "cpa",
                "base_url": "http://old",
                "api_key": "old",
                "api_mode": "chat_completions",
                "context_length": 200000
            }]
        });
        let profile = ApiProfile {
            name: "c".into(),
            provider: "cpa".into(),
            api_url: "http://new".into(),
            api_key: "k".into(),
            model: Some("m".into()),
            context_1m: Some(true),
            hermes: HermesProfileFields {
                api_mode: Some("chat_completions".into()),
            },
            ..Default::default()
        };
        let merged = HermesAdapter::apply_profile_to_config(&shared, &profile);
        assert_eq!(merged["model"]["context_length"], 1_000_000);
        assert_eq!(merged["custom_providers"][0]["context_length"], 1_000_000);
    }

    #[test]
    fn context_1m_none_does_not_clobber_context_length() {
        let shared = serde_json::json!({
            "model": { "context_length": 777777, "provider": "custom:x" },
            "custom_providers": []
        });
        let profile = ApiProfile {
            name: "c".into(),
            provider: "x".into(),
            api_url: "http://x".into(),
            api_key: "k".into(),
            model: Some("m".into()),
            context_1m: None,
            ..Default::default()
        };
        let merged = HermesAdapter::apply_profile_to_config(&shared, &profile);
        assert_eq!(merged["model"]["context_length"], 777777);
    }

    #[test]
    fn write_read_roundtrip_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let adapter = HermesAdapter::with_dir(dir.path().to_path_buf());
        let cfg = HermesAdapter::apply_profile_to_config(
            &serde_json::json!({ "mcp_servers": { "x": { "command": "y" } } }),
            &sample_profile(),
        );
        adapter.write_config(&cfg).unwrap();
        let back = adapter.read_config().unwrap();
        assert_eq!(back["model"]["provider"], "custom:free-model");
        assert_eq!(back["mcp_servers"]["x"]["command"], "y");
        assert_eq!(back["custom_providers"][0]["api_key"], "sk-new");
    }

    #[test]
    fn sync_auth_pool_updates_access_token() {
        let dir = tempfile::tempdir().unwrap();
        let adapter = HermesAdapter::with_dir(dir.path().to_path_buf());
        let auth = dir.path().join("auth.json");
        fs::write(
            &auth,
            r#"{
              "version": 1,
              "credential_pool": {
                "custom:gpt": [
                  {
                    "id": "abc",
                    "access_token": "old-token",
                    "base_url": "https://old.example/v1",
                    "auth_type": "api_key",
                    "priority": 0,
                    "source": "manual",
                    "label": "gpt"
                  }
                ],
                "custom:freemodel": [
                  {
                    "id": "def",
                    "secret_fingerprint": "deadbeef",
                    "base_url": "https://api.freemodel.dev/v1",
                    "auth_type": "api_key",
                    "priority": 0,
                    "source": "manual",
                    "label": "freemodel"
                  }
                ]
              }
            }"#,
        )
        .unwrap();

        adapter
            .sync_auth_pool("gpt", "new-token", "https://new.example/v1")
            .unwrap();
        let data: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&auth).unwrap()).unwrap();
        assert_eq!(
            data["credential_pool"]["custom:gpt"][0]["access_token"],
            "new-token"
        );
        assert_eq!(
            data["credential_pool"]["custom:gpt"][0]["base_url"],
            "https://new.example/v1"
        );
        // fingerprint-only entry untouched
        assert!(data["credential_pool"]["custom:freemodel"][0]
            .get("access_token")
            .is_none());
    }
}
