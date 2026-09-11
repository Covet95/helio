use super::{backup, ConfigAdapter};
use crate::models::{ApiProfile, CONTEXT_LENGTH_1M};
use crate::utils::secure_fs::atomic_write_private;
use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

/// Helio-managed custom Anthropic provider written into ZCode's OpenCode-shaped store.
///
/// ZCode 3.7.7 persists API providers in `{home}/.zcode/v2/config.json`.
/// `ZCODE_DATA_BASE_DIR` replaces `$HOME` (not the v2 directory itself).
/// `setting.json` is UI-only and is not touched.
pub struct ZCodeAdapter {
    config_dir: PathBuf,
}

impl ZCodeAdapter {
    pub fn new() -> Self {
        Self {
            config_dir: Self::default_config_dir(),
        }
    }

    fn default_config_dir() -> PathBuf {
        Self::resolve_config_dir(
            std::env::var("ZCODE_DATA_BASE_DIR").ok().as_deref(),
            dirs::home_dir().expect("Failed to get home directory"),
        )
    }

    /// `{base}/.zcode/v2` — `base` is `ZCODE_DATA_BASE_DIR` or `$HOME`.
    fn resolve_config_dir(base_override: Option<&str>, home: PathBuf) -> PathBuf {
        let base = base_override
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or(home);
        base.join(".zcode").join("v2")
    }

    fn config_file_path(&self) -> PathBuf {
        self.config_dir.join("config.json")
    }

    /// provider 名 → ZCode `provider` map 的 id（空 → `anthropic`，其余小写）
    pub fn normalize_provider_id(provider: &str) -> String {
        let trimmed = provider.trim();
        if trimmed.is_empty() {
            "anthropic".to_string()
        } else {
            trimmed.to_lowercase()
        }
    }

    fn provider_id(api_profile: &ApiProfile) -> String {
        Self::normalize_provider_id(&api_profile.provider)
    }

    fn provider_display_name(api_profile: &ApiProfile) -> String {
        let trimmed = api_profile.provider.trim();
        if trimmed.is_empty() {
            "Anthropic".to_string()
        } else {
            trimmed.to_string()
        }
    }

    pub fn provider_exists_in_config(config: &serde_json::Value, provider_id: &str) -> bool {
        let pid = Self::normalize_provider_id(provider_id);
        config
            .get("provider")
            .and_then(|value| value.as_object())
            .map(|providers| providers.contains_key(&pid))
            .unwrap_or(false)
    }

    pub fn remove_provider_from_config(
        config: &serde_json::Value,
        provider_id: &str,
    ) -> serde_json::Value {
        let pid = Self::normalize_provider_id(provider_id);
        let mut config = config.clone();

        if let Some(providers) = config
            .get_mut("provider")
            .and_then(|value| value.as_object_mut())
        {
            providers.remove(&pid);
        }

        let should_clear_model = config
            .get("model")
            .and_then(|value| value.as_str())
            .and_then(|model| model.split_once('/'))
            .map(|(provider, _)| provider.eq_ignore_ascii_case(&pid))
            .unwrap_or(false);
        if should_clear_model {
            if let Some(object) = config.as_object_mut() {
                object.remove("model");
            }
        }

        config
    }

    fn remove_provider(&self, provider_id: &str) -> Result<()> {
        let pid = Self::normalize_provider_id(provider_id);
        let config = self.read_config()?;
        if !Self::provider_exists_in_config(&config, &pid) {
            return Ok(());
        }
        if self.config_path().exists() {
            self.backup_config()
                .with_context(|| "Failed to back up ZCode config before removal")?;
        }
        self.write_config(&Self::remove_provider_from_config(&config, &pid))
    }

    /// Delete a ZCode profile and clean a provider only when Helio recorded that
    /// it created the provider and no other Helio profile still uses it.
    pub fn delete_profile_and_cleanup_local(db: &crate::db::Database, name: &str) -> Result<bool> {
        let profiles = db.list_profiles()?;
        let Some(profile) = profiles.into_iter().find(|profile| {
            profile.target_app == Some(crate::models::TargetApp::ZCode) && profile.name == name
        }) else {
            return Ok(false);
        };

        let provider = profile.provider;
        let provider_id = Self::normalize_provider_id(&provider);
        let provider_still_used = db.list_profiles()?.into_iter().any(|other| {
            other.id != profile.id
                && other.target_app == Some(crate::models::TargetApp::ZCode)
                && Self::normalize_provider_id(&other.provider) == provider_id
        });
        let provider_managed = db
            .provider_managed_by_helio(crate::models::TargetApp::ZCode, &provider_id)?
            .unwrap_or(false);

        if !provider_still_used && provider_managed {
            Self::new().remove_provider(&provider_id)?;
        }

        let deleted = db.delete_profile(name, crate::models::TargetApp::ZCode)?;
        if deleted && !provider_still_used {
            db.clear_provider_ownership(crate::models::TargetApp::ZCode, &provider_id)?;
        }
        Ok(deleted)
    }

    /// Default model plus Claude-style role mapping models.
    pub fn resolve_model_ids(api_profile: &ApiProfile) -> Vec<String> {
        let mut model_ids = Vec::new();
        let mut push = |model: &str| {
            let model = model.trim();
            if !model.is_empty() && !model_ids.iter().any(|m| m == model) {
                model_ids.push(model.to_string());
            }
        };

        if let Some(model) = api_profile.model.as_deref() {
            push(model);
        }
        if let Some(mapping) = api_profile.claude.model_mapping.as_ref() {
            for role in ["sonnet", "opus", "fable", "haiku"] {
                if let Some(model) = mapping.get(&format!("{role}_model")) {
                    push(model);
                }
            }
        }
        model_ids
    }

    fn role_one_m(api_profile: &ApiProfile, model_id: &str) -> bool {
        let Some(mapping) = api_profile.claude.model_mapping.as_ref() else {
            return false;
        };
        ["sonnet", "opus", "fable", "haiku"].iter().any(|role| {
            mapping
                .get(&format!("{role}_model"))
                .map(|m| m.trim() == model_id)
                .unwrap_or(false)
                && mapping
                    .get(&format!("{role}_one_m"))
                    .map(|s| s == "true")
                    .unwrap_or(false)
        })
    }

    fn model_context_window(api_profile: &ApiProfile, model_id: &str) -> i64 {
        if api_profile.context_1m == Some(true)
            || model_id.trim_end().ends_with("[1M]")
            || Self::role_one_m(api_profile, model_id)
        {
            CONTEXT_LENGTH_1M
        } else if ApiProfile::model_is_grok(Some(model_id)) {
            crate::models::CONTEXT_LENGTH_GROK
        } else {
            crate::models::CONTEXT_LENGTH_STANDARD
        }
    }

    fn model_display_name(api_profile: &ApiProfile, model_id: &str) -> String {
        if let Some(mapping) = api_profile.claude.model_mapping.as_ref() {
            for role in ["sonnet", "opus", "fable", "haiku"] {
                let matches = mapping
                    .get(&format!("{role}_model"))
                    .map(|m| m.trim() == model_id)
                    .unwrap_or(false);
                if !matches {
                    continue;
                }
                if let Some(name) = mapping
                    .get(&format!("{role}_name"))
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                {
                    return name.to_string();
                }
            }
        }
        model_id.to_string()
    }
}

impl Default for ZCodeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

/// 剥离所有 provider 的 `provider.*.options.apiKey`。
fn strip_credentials(config: &mut serde_json::Value) {
    super::credentials::strip_credential_map(config, &["provider"], &["options", "apiKey"]);
}

/// 把磁盘配置中其他 provider 的 key 补回 shared（shared 已剥离）。
/// 当前 provider 的 key 随后会被 merge 用 profile 的值覆盖。
fn restore_credentials(config: &mut serde_json::Value, disk: &serde_json::Value) {
    super::credentials::backfill_credential_map(
        config,
        disk,
        &["provider"],
        &["options", "apiKey"],
        false,
    );
}

impl ConfigAdapter for ZCodeAdapter {
    fn config_path(&self) -> PathBuf {
        self.config_file_path()
    }

    fn read_config(&self) -> Result<serde_json::Value> {
        let path = self.config_path();
        if !path.exists() {
            return Ok(serde_json::json!({}));
        }
        let content = fs::read_to_string(&path).context("Failed to read ZCode config")?;
        serde_json::from_str(&content).context("Failed to parse ZCode config")
    }

    fn extract_shared_config(&self, config: &serde_json::Value) -> serde_json::Value {
        // 凭据不落 shared_configs（key 只存 api_profiles 表）。
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
        if let Ok(disk) = self.read_config() {
            restore_credentials(&mut config, &disk);
        }

        let provider_id = Self::provider_id(api_profile);
        if config.get("provider").is_none() {
            config["provider"] = serde_json::json!({});
        }

        if let Some(providers) = config.get_mut("provider").and_then(|v| v.as_object_mut()) {
            let is_new = !providers.contains_key(&provider_id);
            let entry = providers
                .entry(provider_id.clone())
                .or_insert_with(|| serde_json::json!({}));
            if let Some(p) = entry.as_object_mut() {
                if is_new {
                    p.entry("name".to_string()).or_insert_with(|| {
                        serde_json::Value::String(Self::provider_display_name(api_profile))
                    });
                }
                p.insert(
                    "kind".to_string(),
                    serde_json::Value::String("anthropic".to_string()),
                );
                p.insert(
                    "source".to_string(),
                    serde_json::Value::String("custom".to_string()),
                );
                p.insert("enabled".to_string(), serde_json::Value::Bool(true));

                let options = p
                    .entry("options".to_string())
                    .or_insert_with(|| serde_json::json!({}));
                if let Some(opt) = options.as_object_mut() {
                    opt.insert(
                        "apiKey".to_string(),
                        serde_json::Value::String(api_profile.api_key.clone()),
                    );
                    opt.insert(
                        "baseURL".to_string(),
                        serde_json::Value::String(api_profile.api_url.trim().to_string()),
                    );
                    opt.insert("apiKeyRequired".to_string(), serde_json::Value::Bool(true));
                }

                let model_ids = Self::resolve_model_ids(api_profile);
                if !model_ids.is_empty() {
                    let models = p
                        .entry("models".to_string())
                        .or_insert_with(|| serde_json::json!({}));
                    if let Some(models_obj) = models.as_object_mut() {
                        for model_id in &model_ids {
                            let model = models_obj
                                .entry(model_id.clone())
                                .or_insert_with(|| serde_json::json!({}));
                            if let Some(model_obj) = model.as_object_mut() {
                                model_obj.insert(
                                    "name".to_string(),
                                    serde_json::Value::String(Self::model_display_name(
                                        api_profile,
                                        model_id,
                                    )),
                                );
                                let context = Self::model_context_window(api_profile, model_id);
                                let limit = model_obj
                                    .entry("limit".to_string())
                                    .or_insert_with(|| serde_json::json!({}));
                                if let Some(limit_obj) = limit.as_object_mut() {
                                    limit_obj.insert(
                                        "context".to_string(),
                                        serde_json::Value::Number(context.into()),
                                    );
                                    limit_obj.entry("output".to_string()).or_insert_with(|| {
                                        serde_json::Value::Number(32_000.into())
                                    });
                                }
                                model_obj
                                    .entry("modalities".to_string())
                                    .or_insert_with(|| {
                                        serde_json::json!({
                                            "input": ["text", "image"],
                                            "output": ["text"]
                                        })
                                    });
                            }
                        }
                    }
                }
            }
        }

        let default_model = api_profile
            .model
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .map(|m| m.to_string())
            .or_else(|| Self::resolve_model_ids(api_profile).into_iter().next());

        match default_model {
            Some(model) => {
                config["model"] = serde_json::Value::String(format!("{provider_id}/{model}"));
            }
            None => {
                if let Some(obj) = config.as_object_mut() {
                    obj.remove("model");
                }
            }
        }

        config
    }

    fn write_config(&self, config: &serde_json::Value) -> Result<()> {
        let path = self.config_path();
        let content =
            serde_json::to_string_pretty(config).context("Failed to serialize ZCode config")?;
        atomic_write_private(&path, content.as_bytes()).context("Failed to write ZCode config")?;
        Ok(())
    }

    fn backup_config(&self) -> Result<PathBuf> {
        let path = self.config_path();
        if !path.exists() {
            anyhow::bail!("Config file does not exist");
        }
        let backup_path = backup::backup_required(&self.config_dir, &path, "zcode")?;
        self.cleanup_old_backups(10)?;
        Ok(backup_path)
    }

    fn cleanup_old_backups(&self, keep: usize) -> Result<()> {
        backup::cleanup_prefix(&self.config_dir, "zcode.backup.", keep)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ClaudeProfileFields;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn test_adapter() -> ZCodeAdapter {
        let unique = TEST_DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
        let config_dir = std::env::temp_dir().join(format!(
            "switch-api-zcode-adapter-test-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&config_dir).unwrap();
        ZCodeAdapter { config_dir }
    }

    fn sample_profile() -> ApiProfile {
        let mut mapping = HashMap::new();
        mapping.insert("sonnet_model".into(), "claude-sonnet-4-5".into());
        mapping.insert("sonnet_name".into(), "Sonnet".into());
        mapping.insert("sonnet_one_m".into(), "true".into());
        mapping.insert("opus_model".into(), "claude-opus-4-6".into());
        ApiProfile {
            id: Some(1),
            name: "claude-proxy".to_string(),
            provider: "anthropic".to_string(),
            api_url: "https://api.deepseek.com/anthropic".to_string(),
            api_key: "sk-test-key".to_string(),
            model: Some("claude-sonnet-4-5".to_string()),
            context_1m: Some(true),
            claude: ClaudeProfileFields {
                model_mapping: Some(mapping),
            },
            ..Default::default()
        }
    }

    #[test]
    fn config_dir_joins_zcode_v2_under_home_or_override() {
        let home = PathBuf::from("/Users/me");
        assert_eq!(
            ZCodeAdapter::resolve_config_dir(None, home.clone()),
            home.join(".zcode").join("v2")
        );
        assert_eq!(
            ZCodeAdapter::resolve_config_dir(Some("/tmp/alt"), home.clone()),
            PathBuf::from("/tmp/alt/.zcode/v2")
        );
        assert_eq!(
            ZCodeAdapter::resolve_config_dir(Some("  "), home),
            PathBuf::from("/Users/me/.zcode/v2")
        );
    }

    #[test]
    fn normalize_empty_provider_defaults_to_anthropic() {
        assert_eq!(ZCodeAdapter::normalize_provider_id(""), "anthropic");
        assert_eq!(ZCodeAdapter::normalize_provider_id("  "), "anthropic");
        assert_eq!(ZCodeAdapter::normalize_provider_id("DeepSeek"), "deepseek");
    }

    #[test]
    fn merge_writes_anthropic_custom_provider_without_forcing_v1() {
        let adapter = test_adapter();
        let merged = adapter.merge_config(&sample_profile(), &serde_json::json!({}));
        let provider = &merged["provider"]["anthropic"];
        assert_eq!(provider["kind"], "anthropic");
        assert_eq!(provider["source"], "custom");
        assert_eq!(provider["enabled"], true);
        assert_eq!(provider["options"]["apiKey"], "sk-test-key");
        assert_eq!(
            provider["options"]["baseURL"],
            "https://api.deepseek.com/anthropic"
        );
        assert_eq!(provider["options"]["apiKeyRequired"], true);
        assert_eq!(merged["model"], "anthropic/claude-sonnet-4-5");
        assert_eq!(
            provider["models"]["claude-sonnet-4-5"]["limit"]["context"],
            CONTEXT_LENGTH_1M
        );
        assert_eq!(
            provider["models"]["claude-opus-4-6"]["name"],
            "claude-opus-4-6"
        );
        assert_eq!(provider["models"]["claude-sonnet-4-5"]["name"], "Sonnet");
        let _ = fs::remove_dir_all(&adapter.config_dir);
    }

    #[test]
    fn extract_strips_api_keys() {
        let adapter = test_adapter();
        let shared = adapter.extract_shared_config(&serde_json::json!({
            "theme": "dark",
            "provider": {
                "anthropic": { "options": { "apiKey": "sk-keep-out", "baseURL": "https://x" } },
                "bigmodel": { "options": { "apiKey": "sk-other" } }
            }
        }));
        assert_eq!(shared["theme"], "dark");
        assert!(shared["provider"]["anthropic"]["options"]
            .get("apiKey")
            .is_none());
        assert!(shared["provider"]["bigmodel"]["options"]
            .get("apiKey")
            .is_none());
        let _ = fs::remove_dir_all(&adapter.config_dir);
    }

    #[test]
    fn remove_provider_from_config_clears_only_target_and_default_model() {
        let config = serde_json::json!({
            "model": "anthropic/claude-sonnet-4-5",
            "provider": {
                "anthropic": { "options": { "apiKey": "sk-a" } },
                "manual": { "options": { "apiKey": "sk-m" } }
            }
        });
        let removed = ZCodeAdapter::remove_provider_from_config(&config, "ANTHROPIC");
        assert!(removed["provider"].get("anthropic").is_none());
        assert!(removed["provider"].get("manual").is_some());
        assert!(removed.get("model").is_none());
    }

    #[test]
    fn merge_restores_other_provider_keys_from_disk() {
        let adapter = test_adapter();
        adapter
            .write_config(&serde_json::json!({
                "provider": {
                    "bigmodel": {
                        "name": "BigModel",
                        "options": { "apiKey": "sk-builtin", "baseURL": "https://open.bigmodel.cn" }
                    }
                }
            }))
            .unwrap();

        let shared = serde_json::json!({
            "provider": {
                "bigmodel": {
                    "name": "BigModel",
                    "options": { "baseURL": "https://open.bigmodel.cn" }
                }
            }
        });
        let merged = adapter.merge_config(&sample_profile(), &shared);
        assert_eq!(
            merged["provider"]["bigmodel"]["options"]["apiKey"],
            "sk-builtin"
        );
        assert_eq!(
            merged["provider"]["anthropic"]["options"]["apiKey"],
            "sk-test-key"
        );
        let _ = fs::remove_dir_all(&adapter.config_dir);
    }

    #[test]
    fn restore_credentials_keeps_ids_apart_by_case() {
        // 这条断言锁定 zcode 的「按 id 精确匹配」契约：`OpenAI` 不得拿到磁盘上
        // `openai` 的 key。与 opencode 的大小写不敏感行为互为对照——两者现在
        // 共用 adapters::credentials，仅靠一个布尔参数区分，禁止单方面改动。
        let disk = serde_json::json!({
            "provider": {
                "openai": { "options": { "apiKey": "sk-lower" } }
            }
        });
        let mut shared = serde_json::json!({
            "provider": {
                "OpenAI": { "options": { "baseURL": "https://x" } }
            }
        });
        restore_credentials(&mut shared, &disk);
        assert!(
            shared["provider"]["OpenAI"]["options"]
                .get("apiKey")
                .is_none(),
            "zcode 按 id 精确匹配，大小写不同不应互相补 key"
        );
    }

    #[test]
    fn merge_preserves_unrelated_keys_and_existing_models() {
        let adapter = test_adapter();
        let shared = serde_json::json!({
            "locale": "zh-CN",
            "provider": {
                "anthropic": {
                    "models": {
                        "manual-model": { "name": "manual", "custom": true }
                    }
                }
            }
        });
        let merged = adapter.merge_config(&sample_profile(), &shared);
        assert_eq!(merged["locale"], "zh-CN");
        assert_eq!(
            merged["provider"]["anthropic"]["models"]["manual-model"]["custom"],
            true
        );
        assert!(merged["provider"]["anthropic"]["models"]
            .get("claude-sonnet-4-5")
            .is_some());
        let _ = fs::remove_dir_all(&adapter.config_dir);
    }

    #[test]
    fn resolve_model_ids_includes_default_and_mapping() {
        let ids = ZCodeAdapter::resolve_model_ids(&sample_profile());
        assert_eq!(
            ids,
            vec![
                "claude-sonnet-4-5".to_string(),
                "claude-opus-4-6".to_string()
            ]
        );
    }

    #[test]
    fn write_read_roundtrip_keeps_key_on_disk_and_strips_from_shared() {
        let adapter = test_adapter();
        let merged =
            adapter.merge_config(&sample_profile(), &serde_json::json!({ "locale": "zh-CN" }));
        adapter.write_config(&merged).unwrap();

        let disk = adapter.read_config().unwrap();
        assert_eq!(
            disk["provider"]["anthropic"]["options"]["apiKey"],
            "sk-test-key"
        );
        assert_eq!(
            disk["provider"]["anthropic"]["options"]["baseURL"],
            "https://api.deepseek.com/anthropic"
        );
        assert_eq!(disk["model"], "anthropic/claude-sonnet-4-5");
        assert_eq!(disk["locale"], "zh-CN");

        let shared = adapter.extract_shared_config(&disk);
        assert!(shared["provider"]["anthropic"]["options"]
            .get("apiKey")
            .is_none());
        assert_eq!(
            shared["provider"]["anthropic"]["options"]["baseURL"],
            "https://api.deepseek.com/anthropic"
        );
        assert_eq!(shared["locale"], "zh-CN");

        let backup = adapter.backup_config().unwrap();
        let name = backup.file_name().unwrap().to_string_lossy();
        assert!(
            name.starts_with("zcode.backup."),
            "unexpected backup name: {name}"
        );

        let _ = fs::remove_dir_all(&adapter.config_dir);
    }
}
