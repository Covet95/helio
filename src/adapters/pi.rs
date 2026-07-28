use super::ConfigAdapter;
use crate::models::ApiProfile;
use crate::utils::secure_fs::{atomic_write_private, copy_private};
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Pi coding agent adapter — `~/.pi/agent/`.
///
/// Switch surface:
/// - `auth.json`: merge api_key for the profile provider id
/// - `models.json`: upsert custom provider when api_url is non-official
/// - `settings.json`: defaultProvider / defaultModel only; rest shared
pub struct PiAdapter {
    config_dir: PathBuf,
}

impl PiAdapter {
    pub fn new() -> Self {
        let home = dirs::home_dir().expect("Failed to get home directory");
        Self {
            config_dir: home.join(".pi").join("agent"),
        }
    }

    #[cfg(test)]
    fn with_dir(config_dir: PathBuf) -> Self {
        Self { config_dir }
    }

    fn settings_path(&self) -> PathBuf {
        self.config_dir.join("settings.json")
    }

    fn auth_path(&self) -> PathBuf {
        self.config_dir.join("auth.json")
    }

    fn models_path(&self) -> PathBuf {
        self.config_dir.join("models.json")
    }

    /// provider id used in auth.json / models.json / settings.defaultProvider
    pub fn provider_id(provider: &str) -> String {
        let s = provider.trim().to_lowercase();
        if s.is_empty() {
            "custom".to_string()
        } else {
            s
        }
    }

    /// Official hosts that do not require a models.json custom provider entry.
    pub fn is_official_base(provider_id: &str, api_url: &str) -> bool {
        let url = api_url.trim().trim_end_matches('/');
        if url.is_empty() {
            return true;
        }
        let lower = url.to_lowercase();
        // Strip scheme for host checks
        let host_path = lower
            .strip_prefix("https://")
            .or_else(|| lower.strip_prefix("http://"))
            .unwrap_or(lower.as_str());

        match provider_id {
            "anthropic" => {
                host_path == "api.anthropic.com" || host_path.starts_with("api.anthropic.com/")
            }
            "openai" => {
                host_path == "api.openai.com"
                    || host_path == "api.openai.com/v1"
                    || host_path.starts_with("api.openai.com/")
            }
            "google" => {
                host_path == "generativelanguage.googleapis.com"
                    || host_path.starts_with("generativelanguage.googleapis.com/")
            }
            "openrouter" => {
                host_path == "openrouter.ai"
                    || host_path == "openrouter.ai/api/v1"
                    || host_path.starts_with("openrouter.ai/")
            }
            "deepseek" => {
                host_path == "api.deepseek.com"
                    || host_path == "api.deepseek.com/v1"
                    || host_path.starts_with("api.deepseek.com/")
            }
            _ => false,
        }
    }

    /// Map profile protocol fields → Pi models.json `api` value.
    pub fn resolve_api(profile: &ApiProfile, provider_id: &str, api_url: &str) -> String {
        let mode = profile
            .hermes
            .api_mode
            .as_deref()
            .or(profile.openclaw.api_mode.as_deref())
            .unwrap_or("")
            .trim()
            .to_lowercase();
        let wire = profile
            .codex
            .wire_api
            .as_deref()
            .unwrap_or("")
            .trim()
            .to_lowercase();

        if mode.contains("anthropic") || mode == "anthropic_messages" || mode == "anthropic-messages"
        {
            return "anthropic-messages".into();
        }
        if mode.contains("response")
            || mode == "codex_responses"
            || mode == "openai-responses"
            || wire == "responses"
        {
            return "openai-responses".into();
        }
        if mode.contains("google") || mode == "google-generative-ai" {
            return "google-generative-ai".into();
        }
        // Official google host without explicit mode
        if provider_id == "google" && Self::is_official_base("google", api_url) {
            return "google-generative-ai".into();
        }
        "openai-completions".into()
    }

    fn read_json_object(path: &Path) -> Result<serde_json::Value> {
        if !path.exists() {
            return Ok(serde_json::json!({}));
        }
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        if content.trim().is_empty() {
            return Ok(serde_json::json!({}));
        }
        let v: serde_json::Value = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse {}", path.display()))?;
        if v.is_object() {
            Ok(v)
        } else {
            Ok(serde_json::json!({}))
        }
    }

    fn write_json(path: &Path, value: &serde_json::Value) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create {}", parent.display()))?;
        }
        let content =
            serde_json::to_string_pretty(value).context("Failed to serialize pi config")?;
        atomic_write_private(path, format!("{content}\n").as_bytes())
            .with_context(|| format!("Failed to write {}", path.display()))?;
        Ok(())
    }

    /// Merge api_key credential for provider into auth.json document.
    pub fn merge_auth(
        auth: &serde_json::Value,
        provider_id: &str,
        api_key: &str,
    ) -> serde_json::Value {
        let mut out = if auth.is_object() {
            auth.clone()
        } else {
            serde_json::json!({})
        };
        let entry = serde_json::json!({
            "type": "api_key",
            "key": api_key,
        });
        out.as_object_mut()
            .unwrap()
            .insert(provider_id.to_string(), entry);
        out
    }

    /// Upsert custom provider in models.json.
    pub fn merge_models(
        models: &serde_json::Value,
        provider_id: &str,
        api_url: &str,
        api: &str,
        api_key: &str,
        model: Option<&str>,
    ) -> serde_json::Value {
        let mut root = if models.is_object() {
            models.clone()
        } else {
            serde_json::json!({})
        };
        let providers = root
            .as_object_mut()
            .unwrap()
            .entry("providers")
            .or_insert_with(|| serde_json::json!({}));
        if !providers.is_object() {
            *providers = serde_json::json!({});
        }

        let existing = providers.get(provider_id).cloned().unwrap_or(serde_json::json!({}));
        let mut prov = if existing.is_object() {
            existing
        } else {
            serde_json::json!({})
        };

        {
            let obj = prov.as_object_mut().unwrap();
            obj.insert(
                "baseUrl".into(),
                serde_json::Value::String(api_url.trim_end_matches('/').to_string()),
            );
            obj.insert("api".into(), serde_json::Value::String(api.to_string()));
            obj.insert(
                "apiKey".into(),
                serde_json::Value::String(api_key.to_string()),
            );

            let mut model_list = obj
                .get("models")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            if let Some(mid) = model.map(str::trim).filter(|s| !s.is_empty()) {
                let already = model_list.iter().any(|m| {
                    m.get("id")
                        .and_then(|v| v.as_str())
                        .map(|s| s == mid)
                        .unwrap_or(false)
                });
                if !already {
                    model_list.push(serde_json::json!({ "id": mid }));
                }
            }
            obj.insert("models".into(), serde_json::Value::Array(model_list));
        }

        providers
            .as_object_mut()
            .unwrap()
            .insert(provider_id.to_string(), prov);
        root
    }

    pub fn merge_settings_defaults(
        settings: &serde_json::Value,
        provider_id: &str,
        model: Option<&str>,
    ) -> serde_json::Value {
        let mut out = if settings.is_object() {
            settings.clone()
        } else {
            serde_json::json!({})
        };
        let obj = out.as_object_mut().unwrap();
        obj.insert(
            "defaultProvider".into(),
            serde_json::Value::String(provider_id.to_string()),
        );
        if let Some(mid) = model.map(str::trim).filter(|s| !s.is_empty()) {
            obj.insert(
                "defaultModel".into(),
                serde_json::Value::String(mid.to_string()),
            );
        }
        out
    }

    fn backup_one(path: &Path, config_dir: &Path, stamp: &str, label: &str) -> Result<()> {
        if !path.exists() {
            return Ok(());
        }
        let backup = config_dir.join(format!("{label}.backup.{stamp}.json"));
        copy_private(path, &backup)
            .with_context(|| format!("Failed to backup {}", path.display()))?;
        Ok(())
    }
}

impl Default for PiAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfigAdapter for PiAdapter {
    fn config_path(&self) -> PathBuf {
        self.settings_path()
    }

    fn read_config(&self) -> Result<serde_json::Value> {
        Self::read_json_object(&self.settings_path())
    }

    fn extract_shared_config(&self, config: &serde_json::Value) -> serde_json::Value {
        // API lives in auth.json / models.json; settings are shared preferences.
        config.clone()
    }

    fn merge_config(
        &self,
        api_profile: &ApiProfile,
        shared_config: &serde_json::Value,
    ) -> serde_json::Value {
        let pid = Self::provider_id(&api_profile.provider);
        Self::merge_settings_defaults(shared_config, &pid, api_profile.model.as_deref())
    }

    fn write_config(&self, config: &serde_json::Value) -> Result<()> {
        Self::write_json(&self.settings_path(), config)
    }

    fn backup_config(&self) -> Result<PathBuf> {
        let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();
        fs::create_dir_all(&self.config_dir).ok();
        Self::backup_one(&self.settings_path(), &self.config_dir, &stamp, "settings")?;
        Self::backup_one(&self.auth_path(), &self.config_dir, &stamp, "auth")?;
        Self::backup_one(&self.models_path(), &self.config_dir, &stamp, "models")?;
        self.cleanup_old_backups(10)?;
        // Return primary backup path (settings if existed, else auth)
        let settings_backup = self
            .config_dir
            .join(format!("settings.backup.{stamp}.json"));
        if settings_backup.exists() {
            Ok(settings_backup)
        } else {
            Ok(self.config_dir.join(format!("auth.backup.{stamp}.json")))
        }
    }

    fn cleanup_old_backups(&self, keep: usize) -> Result<()> {
        if !self.config_dir.exists() {
            return Ok(());
        }
        let mut backups: Vec<_> = fs::read_dir(&self.config_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.starts_with("settings.backup.")
                    || name.starts_with("auth.backup.")
                    || name.starts_with("models.backup.")
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

    fn managed_paths(&self) -> Vec<PathBuf> {
        vec![
            self.settings_path(),
            self.auth_path(),
            self.models_path(),
        ]
    }

    fn apply_api_credentials(&self, api_profile: &ApiProfile) -> Result<()> {
        let pid = Self::provider_id(&api_profile.provider);
        let key = api_profile.api_key.as_str();

        // auth.json merge
        let auth = Self::read_json_object(&self.auth_path())?;
        let auth = Self::merge_auth(&auth, &pid, key);
        Self::write_json(&self.auth_path(), &auth)?;

        // models.json only for custom endpoints
        if !Self::is_official_base(&pid, &api_profile.api_url) {
            let api = Self::resolve_api(api_profile, &pid, &api_profile.api_url);
            let models = Self::read_json_object(&self.models_path())?;
            let models = Self::merge_models(
                &models,
                &pid,
                &api_profile.api_url,
                &api,
                key,
                api_profile.model.as_deref(),
            );
            Self::write_json(&self.models_path(), &models)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ApiProfile;

    #[test]
    fn provider_id_normalizes() {
        assert_eq!(PiAdapter::provider_id(" Anthropic "), "anthropic");
        assert_eq!(PiAdapter::provider_id(""), "custom");
    }

    #[test]
    fn official_bases() {
        assert!(PiAdapter::is_official_base("anthropic", ""));
        assert!(PiAdapter::is_official_base(
            "anthropic",
            "https://api.anthropic.com"
        ));
        assert!(PiAdapter::is_official_base(
            "openai",
            "https://api.openai.com/v1"
        ));
        assert!(PiAdapter::is_official_base(
            "google",
            "https://generativelanguage.googleapis.com"
        ));
        assert!(!PiAdapter::is_official_base(
            "anthropic",
            "https://api.deepseek.com/anthropic"
        ));
        assert!(!PiAdapter::is_official_base(
            "custom",
            "http://127.0.0.1:8317/v1"
        ));
    }

    #[test]
    fn merge_auth_preserves_peers() {
        let existing = serde_json::json!({
            "openai": { "type": "oauth", "access": "tok" },
            "anthropic": { "type": "api_key", "key": "old" }
        });
        let merged = PiAdapter::merge_auth(&existing, "anthropic", "new-key");
        assert_eq!(merged["anthropic"]["key"], "new-key");
        assert_eq!(merged["anthropic"]["type"], "api_key");
        assert_eq!(merged["openai"]["type"], "oauth");
        assert_eq!(merged["openai"]["access"], "tok");
    }

    #[test]
    fn merge_models_upserts_and_preserves() {
        let existing = serde_json::json!({
            "providers": {
                "ollama": {
                    "baseUrl": "http://localhost:11434/v1",
                    "api": "openai-completions",
                    "models": [{ "id": "llama" }]
                }
            }
        });
        let merged = PiAdapter::merge_models(
            &existing,
            "corp",
            "https://proxy.example/v1",
            "openai-completions",
            "sk",
            Some("foo"),
        );
        assert!(merged["providers"]["ollama"]["models"][0]["id"] == "llama");
        assert_eq!(
            merged["providers"]["corp"]["baseUrl"],
            "https://proxy.example/v1"
        );
        assert_eq!(merged["providers"]["corp"]["models"][0]["id"], "foo");
    }

    #[test]
    fn merge_settings_keeps_theme() {
        let shared = serde_json::json!({ "theme": "dark", "compaction": { "enabled": true } });
        let merged = PiAdapter::merge_settings_defaults(&shared, "anthropic", Some("claude-x"));
        assert_eq!(merged["theme"], "dark");
        assert_eq!(merged["defaultProvider"], "anthropic");
        assert_eq!(merged["defaultModel"], "claude-x");
        assert_eq!(merged["compaction"]["enabled"], true);
    }

    #[test]
    fn apply_writes_auth_and_settings() {
        let dir = std::env::temp_dir().join(format!(
            "helio-pi-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        // seed peer auth
        fs::write(
            dir.join("auth.json"),
            r#"{"openai":{"type":"oauth","access":"t"}}"#,
        )
        .unwrap();
        fs::write(dir.join("settings.json"), r#"{"theme":"light"}"#).unwrap();

        let adapter = PiAdapter::with_dir(dir.clone());
        let profile = ApiProfile {
            name: "a".into(),
            provider: "anthropic".into(),
            api_url: "https://api.anthropic.com".into(),
            api_key: "sk-ant".into(),
            model: Some("claude-sonnet-4-5".into()),
            ..Default::default()
        };
        let shared = adapter.read_config().unwrap();
        let merged = adapter.merge_config(&profile, &shared);
        adapter.write_config(&merged).unwrap();
        adapter.apply_api_credentials(&profile).unwrap();

        let auth: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(dir.join("auth.json")).unwrap()).unwrap();
        assert_eq!(auth["anthropic"]["key"], "sk-ant");
        assert_eq!(auth["openai"]["type"], "oauth");
        let settings: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(dir.join("settings.json")).unwrap()).unwrap();
        assert_eq!(settings["theme"], "light");
        assert_eq!(settings["defaultProvider"], "anthropic");
        // official → no models.json required
        assert!(!dir.join("models.json").exists() || {
            let m: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(dir.join("models.json")).unwrap_or_default())
                    .unwrap_or(serde_json::json!({}));
            m.get("providers")
                .and_then(|p| p.get("anthropic"))
                .is_none()
        });

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_custom_writes_models() {
        let dir = std::env::temp_dir().join(format!(
            "helio-pi-custom-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("models.json"),
            r#"{"providers":{"ollama":{"baseUrl":"http://localhost:11434/v1","api":"openai-completions","models":[{"id":"x"}]}}}"#,
        )
        .unwrap();

        let adapter = PiAdapter::with_dir(dir.clone());
        let profile = ApiProfile {
            name: "p".into(),
            provider: "corp".into(),
            api_url: "https://proxy.corp/v1".into(),
            api_key: "k".into(),
            model: Some("m1".into()),
            ..Default::default()
        };
        adapter.apply_api_credentials(&profile).unwrap();
        let models: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(dir.join("models.json")).unwrap()).unwrap();
        assert_eq!(models["providers"]["corp"]["baseUrl"], "https://proxy.corp/v1");
        assert_eq!(models["providers"]["corp"]["models"][0]["id"], "m1");
        assert_eq!(
            models["providers"]["ollama"]["models"][0]["id"],
            "x"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
