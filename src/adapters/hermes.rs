use super::{backup, ConfigAdapter};
use crate::models::ApiProfile;
use crate::utils::secure_fs::atomic_write_private;
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
    ///
    /// - `context_1m=true`  → 1_000_000
    /// - `context_1m=false` → Grok 500_000 / others 200_000
    ///   (ignore any previous oversized value so turning 1M off always lands
    ///   on the model-aware standard default)
    /// - `None`             → do not clobber existing config
    pub fn resolve_context_length(profile: &ApiProfile, _existing: Option<i64>) -> Option<i64> {
        use crate::models::CONTEXT_LENGTH_1M;
        match profile.context_1m {
            Some(true) => Some(CONTEXT_LENGTH_1M),
            Some(false) => Some(profile.standard_context_length()),
            None => None,
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
        let existing_model_ctx = model.get("context_length").and_then(|v| v.as_i64());
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
        cfg.as_object_mut().unwrap().insert("model".into(), model);

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
            let ename = obj.get("name").and_then(|v| v.as_str()).unwrap_or("");
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
        let content =
            serde_yaml::to_string(config).context("Failed to serialize Hermes config.yaml")?;
        atomic_write_private(path, content.as_bytes())
            .context("Failed to write Hermes config.yaml")?;
        Ok(())
    }

    /// Mirror Helio keys into credential_pool[custom:<name>] as access_token entries.
    /// Active key is written first so Hermes order-based fallback prefers it.
    /// Creates auth.json / credential_pool when missing.
    fn sync_auth_pool_keys(
        &self,
        provider: &str,
        base_url: &str,
        keys: &[(String, bool)], // (secret, is_active)
    ) -> Result<()> {
        let path = self.auth_path();
        let mut data: serde_json::Value = if path.exists() {
            let content = fs::read_to_string(&path).context("Failed to read Hermes auth.json")?;
            serde_json::from_str(&content).context("Failed to parse Hermes auth.json")?
        } else {
            serde_json::json!({})
        };
        if !data.is_object() {
            data = serde_json::json!({});
        }

        let pool_key = Self::custom_provider_slug(provider);
        let Some(root) = data.as_object_mut() else {
            return Err(anyhow::anyhow!(
                "Hermes auth.json 结构异常（非对象），拒绝覆写"
            ));
        };
        if !root.contains_key("credential_pool") {
            root.insert("credential_pool".into(), serde_json::json!({}));
        }
        // 用户旧版数据 credential_pool 可能是数组/字符串：重建为空对象而非 panic
        let pool = match root
            .get_mut("credential_pool")
            .and_then(|v| v.as_object_mut())
        {
            Some(p) => p,
            None => {
                root.insert("credential_pool".into(), serde_json::json!({}));
                root.get_mut("credential_pool")
                    .and_then(|v| v.as_object_mut())
                    .ok_or_else(|| anyhow::anyhow!("无法重建 credential_pool"))?
            }
        };

        // active first, then others; skip empty secrets; de-dupe by secret
        let mut ordered: Vec<&str> = Vec::new();
        for (secret, is_active) in keys {
            if *is_active && !secret.trim().is_empty() {
                ordered.push(secret.as_str());
            }
        }
        for (secret, is_active) in keys {
            if !*is_active && !secret.trim().is_empty() && !ordered.contains(&secret.as_str()) {
                ordered.push(secret.as_str());
            }
        }
        if ordered.is_empty() {
            return Ok(());
        }

        let arr: Vec<serde_json::Value> = ordered
            .into_iter()
            .map(|secret| {
                serde_json::json!({
                    "access_token": secret,
                    "base_url": base_url,
                    "auth_type": "api_key",
                })
            })
            .collect();
        pool.insert(pool_key, serde_json::Value::Array(arr));

        let out =
            serde_json::to_string_pretty(&data).context("Failed to serialize Hermes auth.json")?;
        atomic_write_private(&path, out.as_bytes()).context("Failed to write Hermes auth.json")?;
        Ok(())
    }
}

impl Default for HermesAdapter {
    fn default() -> Self {
        Self::new()
    }
}

/// 剥离 custom_providers[].api_key。
fn strip_credentials(config: &mut serde_json::Value) {
    if let Some(arr) = config
        .get_mut("custom_providers")
        .and_then(|v| v.as_array_mut())
    {
        for entry in arr.iter_mut() {
            if let Some(obj) = entry.as_object_mut() {
                obj.remove("api_key");
            }
        }
    }
}

/// 把磁盘配置中其他 provider 的 api_key 补回 shared（shared 已剥离）。
/// 当前 provider 的 key 随后会被 merge 用 profile 的值覆盖。
fn restore_credentials(config: &mut serde_json::Value, disk: &serde_json::Value) {
    let Some(shared_arr) = config
        .get_mut("custom_providers")
        .and_then(|v| v.as_array_mut())
    else {
        return;
    };
    let Some(disk_arr) = disk.get("custom_providers").and_then(|v| v.as_array()) else {
        return;
    };
    for shared_entry in shared_arr.iter_mut() {
        let Some(sobj) = shared_entry.as_object_mut() else {
            continue;
        };
        if sobj.contains_key("api_key") {
            continue;
        }
        let Some(name) = sobj.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        if name.trim().is_empty() {
            continue;
        }
        for d in disk_arr {
            let Some(dobj) = d.as_object() else {
                continue;
            };
            let dname = dobj.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if HermesAdapter::normalize_provider_name(dname)
                == HermesAdapter::normalize_provider_name(name)
            {
                if let Some(k) = dobj.get("api_key") {
                    sobj.insert("api_key".into(), k.clone());
                }
                break;
            }
        }
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
        // 凭据不落 shared_configs（key 只存 api_profiles 表）。
        // auth.json 的 credential_pool 是独立文件、从不进 shared；这里剥离
        // config.yaml custom_providers 里的 api_key。merge 时从磁盘补回。
        let mut shared = config.clone();
        strip_credentials(&mut shared);
        shared
    }

    fn merge_config(
        &self,
        api_profile: &ApiProfile,
        shared_config: &serde_json::Value,
    ) -> serde_json::Value {
        let mut config = shared_config.clone();
        // shared 已剥离凭据：从磁盘补回其他 provider 的 key（多 provider 共存）
        if let Ok(disk) = self.read_config() {
            restore_credentials(&mut config, &disk);
        }
        Self::apply_profile_to_config(&config, api_profile)
    }

    fn write_config(&self, config: &serde_json::Value) -> Result<()> {
        Self::write_yaml(&self.config_file_path(), config)
    }

    fn backup_config(&self) -> Result<PathBuf> {
        let path = self.config_file_path();
        if !path.exists() {
            anyhow::bail!("Config file does not exist");
        }
        let backup_path = backup::backup_required(&self.config_dir, &path, "config")?;

        // auth.json 备份失败不中断主备份
        let _ = backup::backup_one(&self.config_dir, &self.auth_path(), "auth")?;

        self.cleanup_old_backups(10)?;
        Ok(backup_path)
    }

    fn cleanup_old_backups(&self, keep: usize) -> Result<()> {
        backup::cleanup_prefix(&self.config_dir, "config.backup.", keep)?;
        backup::cleanup_prefix(&self.config_dir, "auth.backup.", keep)
    }

    fn managed_paths(&self) -> Vec<PathBuf> {
        vec![self.config_file_path(), self.auth_path()]
    }

    fn apply_api_credentials(&self, api_profile: &ApiProfile) -> Result<()> {
        let mut keys: Vec<(String, bool)> = Vec::new();
        if let Some(list) = api_profile.api_keys.as_ref() {
            for e in list {
                if !e.key.trim().is_empty() {
                    keys.push((e.key.clone(), e.is_active));
                }
            }
        }
        if keys.is_empty() && !api_profile.api_key.trim().is_empty() {
            keys.push((api_profile.api_key.clone(), true));
        }
        self.sync_auth_pool_keys(&api_profile.provider, &api_profile.api_url, &keys)
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
        assert_eq!(HermesAdapter::normalize_provider_name("cpa"), "cpa");
        assert_eq!(HermesAdapter::custom_provider_slug("CPA"), "custom:cpa");
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
    fn context_1m_false_grok_writes_500k() {
        let shared = serde_json::json!({
            "model": { "context_length": 1_000_000 },
            "custom_providers": [{
                "name": "custom",
                "base_url": "http://old",
                "api_key": "old",
                "context_length": 1_000_000
            }]
        });
        let profile = ApiProfile {
            name: "g".into(),
            provider: "custom".into(),
            api_url: "http://new".into(),
            api_key: "k".into(),
            model: Some("grok-4.5".into()),
            context_1m: Some(false),
            hermes: HermesProfileFields {
                api_mode: Some("anthropic_messages".into()),
            },
            ..Default::default()
        };
        let merged = HermesAdapter::apply_profile_to_config(&shared, &profile);
        assert_eq!(merged["model"]["context_length"], 500_000);
        assert_eq!(merged["custom_providers"][0]["context_length"], 500_000);
    }

    #[test]
    fn context_1m_false_non_grok_writes_200k() {
        let profile = ApiProfile {
            name: "c".into(),
            provider: "cpa".into(),
            api_url: "http://new".into(),
            api_key: "k".into(),
            model: Some("claude-opus-4-8".into()),
            context_1m: Some(false),
            ..Default::default()
        };
        assert_eq!(
            HermesAdapter::resolve_context_length(&profile, Some(999_999)),
            Some(200_000)
        );
    }

    #[test]
    fn model_is_grok_detection() {
        assert!(ApiProfile::model_is_grok(Some("grok-4.5")));
        assert!(ApiProfile::model_is_grok(Some("xai/grok-4")));
        assert!(ApiProfile::model_is_grok(Some("Grok-3")));
        assert!(!ApiProfile::model_is_grok(Some("claude-opus-4-8")));
        assert!(!ApiProfile::model_is_grok(None));
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
    fn sync_auth_pool_mirrors_keys_and_preserves_other_providers() {
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
                    "auth_type": "api_key"
                  }
                ],
                "custom:freemodel": [
                  {
                    "id": "def",
                    "secret_fingerprint": "deadbeef",
                    "base_url": "https://api.freemodel.dev/v1",
                    "auth_type": "api_key"
                  }
                ]
              }
            }"#,
        )
        .unwrap();

        adapter
            .sync_auth_pool_keys(
                "gpt",
                "https://new.example/v1",
                &[
                    ("active-token".into(), true),
                    ("backup-token".into(), false),
                ],
            )
            .unwrap();
        let data: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&auth).unwrap()).unwrap();
        let gpt = data["credential_pool"]["custom:gpt"].as_array().unwrap();
        assert_eq!(gpt.len(), 2);
        assert_eq!(gpt[0]["access_token"], "active-token");
        assert_eq!(gpt[0]["base_url"], "https://new.example/v1");
        assert_eq!(gpt[1]["access_token"], "backup-token");
        // other provider untouched
        assert!(data["credential_pool"]["custom:freemodel"][0]
            .get("access_token")
            .is_none());
    }

    #[test]
    fn sync_auth_pool_creates_file_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let adapter = HermesAdapter::with_dir(dir.path().to_path_buf());
        adapter
            .sync_auth_pool_keys(
                "cpa",
                "http://127.0.0.1:8317/v1",
                &[("sk-only".into(), true)],
            )
            .unwrap();
        let auth = dir.path().join("auth.json");
        assert!(auth.exists());
        let data: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&auth).unwrap()).unwrap();
        assert_eq!(
            data["credential_pool"]["custom:cpa"][0]["access_token"],
            "sk-only"
        );
    }
}
