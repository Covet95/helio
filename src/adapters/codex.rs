use super::ConfigAdapter;
use crate::models::ApiProfile;
use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

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

    /// Codex 内置（保留）的 provider id —— 不允许在 model_providers 中覆盖。
    /// 参见 Codex 报错：`model_providers contains reserved built-in provider IDs`。
    fn is_reserved_provider_id(id: &str) -> bool {
        matches!(id, "openai" | "oss")
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
        // config.toml 里唯一的 API 端点信息是各 provider 的 base_url。
        // 因此只移除 base_url，保留 wire_api / requires_openai_auth / name 等协议字段。
        if let Some(obj) = shared.as_object_mut() {
            // 兼容历史版本误写入的顶层 api_key
            obj.remove("api_key");

            if let Some(providers) = obj.get_mut("model_providers").and_then(|v| v.as_object_mut()) {
                for (_name, provider) in providers.iter_mut() {
                    if let Some(p) = provider.as_object_mut() {
                        p.remove("base_url");
                    }
                }
            }
        }

        shared
    }

    fn merge_config(
        &self,
        api_profile: &ApiProfile,
        shared_config: &serde_json::Value,
    ) -> serde_json::Value {
        let mut config = shared_config.clone();

        if config.get("model_providers").is_none() {
            config["model_providers"] = serde_json::json!({});
        }

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

        // 写入 provider 配置：只改 base_url，保留该 provider 已有的 wire_api /
        // requires_openai_auth / name 等协议字段；仅在 provider 全新时补默认值。
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
                p.entry("requires_openai_auth".to_string())
                    .or_insert_with(|| serde_json::Value::Bool(true));
                if is_new {
                    // 全新 provider：补上 Codex 必需的协议默认值。
                    p.entry("name".to_string())
                        .or_insert_with(|| serde_json::Value::String(provider_id.clone()));
                    p.entry("wire_api".to_string())
                        .or_insert_with(|| serde_json::Value::String("responses".to_string()));
                }
                // 历史版本误写入的 env_key 不再使用（key 走 auth.json）
                p.remove("env_key");
            }
        }

        // 设置当前使用的 provider。API key 不写 config.toml —— 走 auth.json
        // （见 apply_api_credentials），且清掉历史版本误写入的顶层 api_key。
        config["model_provider"] = serde_json::Value::String(provider_id);
        if let Some(obj) = config.as_object_mut() {
            obj.remove("api_key");

            match api_profile.model.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
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
                }
                Some(false) | None => {
                    obj.remove("model_context_window");
                }
            }
        }

        config
    }

    fn write_config(&self, config: &serde_json::Value) -> Result<()> {
        let path = self.config_path();

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).context("Failed to create Codex config directory")?;
        }

        let toml_value = Self::json_to_toml(config)?;
        let content =
            toml::to_string_pretty(&toml_value).context("Failed to serialize Codex TOML")?;

        // 原子写入：临时文件 + rename
        let temp_path = path.with_extension("toml.tmp");
        fs::write(&temp_path, &content).context("Failed to write temp Codex config")?;

        if let Ok(file) = fs::File::open(&temp_path) {
            let _ = file.sync_all();
        }

        fs::rename(&temp_path, &path).context("Failed to rename temp Codex config")?;

        Ok(())
    }

    fn backup_config(&self) -> Result<PathBuf> {
        let path = self.config_path();

        if !path.exists() {
            anyhow::bail!("Config file does not exist");
        }

        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let backup_path = self
            .config_dir
            .join(format!("config.backup.{}.toml", timestamp));

        fs::copy(&path, &backup_path).context("Failed to backup config")?;

        self.cleanup_old_backups(10)?;

        Ok(backup_path)
    }

    fn cleanup_old_backups(&self, keep: usize) -> Result<()> {
        let mut backups: Vec<_> = fs::read_dir(&self.config_dir)?
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("config.backup.")
            })
            .collect();

        backups.sort_by_key(|entry| {
            entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        });
        backups.reverse();

        for entry in backups.iter().skip(keep) {
            let _ = fs::remove_file(entry.path());
        }

        Ok(())
    }

    /// Codex 特有：API key 存在独立的 ~/.codex/auth.json 的 OPENAI_API_KEY 字段，
    /// 而非 config.toml。保留 auth.json 中的其他字段，只更新 OPENAI_API_KEY。
    fn apply_api_credentials(&self, api_profile: &ApiProfile) -> Result<()> {
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
                "OPENAI_API_KEY".to_string(),
                serde_json::Value::String(api_profile.api_key.clone()),
            );
        }

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).context("Failed to create Codex config directory")?;
        }

        let content =
            serde_json::to_string_pretty(&auth).context("Failed to serialize Codex auth.json")?;
        let temp_path = path.with_extension("json.tmp");
        fs::write(&temp_path, &content).context("Failed to write temp Codex auth.json")?;
        if let Ok(file) = fs::File::open(&temp_path) {
            let _ = file.sync_all();
        }
        fs::rename(&temp_path, &path).context("Failed to rename temp Codex auth.json")?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_profile() -> ApiProfile {
        ApiProfile {
            id: Some(1),
            name: "test".to_string(),
            provider: "openai".to_string(),
            api_url: "https://api.example.com/v1".to_string(),
            api_key: "sk-test-key".to_string(),
            model_mapping: None,
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
        assert_eq!(json["model_providers"]["openai"]["base_url"], "https://old.api.com");
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
                    "name": "OpenAI"
                }
            },
            "mcp_servers": {
                "fs": { "command": "npx" }
            }
        });

        let shared = adapter.extract_shared_config(&config);

        // API 字段被移除
        assert!(shared.get("api_key").is_none());
        assert!(shared["model_providers"]["openai"].get("base_url").is_none());
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
        assert_eq!(merged["model_providers"]["openai-custom"]["wire_api"], "responses");
        assert_eq!(
            merged["model_providers"]["openai-custom"]["requires_openai_auth"],
            true
        );
        // 不得创建被保留的 openai provider 块
        assert!(merged["model_providers"].get("openai").is_none());
        // API key 绝不写进 config.toml（走 auth.json）
        assert!(merged.get("api_key").is_none());
        assert!(merged["model_providers"]["openai-custom"].get("env_key").is_none());
        // 共享配置保留
        assert_eq!(merged["mcp_servers"]["fs"]["command"], "npx");
    }

    #[test]
    fn test_merge_avoids_reserved_provider_id() {
        let adapter = CodexAdapter::new();
        // 非保留字 provider 原样使用
        let custom = ApiProfile {
            provider: "myproxy".to_string(),
            ..sample_profile()
        };
        let merged = adapter.merge_config(&custom, &serde_json::json!({}));
        assert_eq!(merged["model_provider"], "myproxy");
        assert!(merged["model_providers"]["myproxy"].is_object());

        // 保留字 oss 同样被改名
        let oss = ApiProfile {
            provider: "OSS".to_string(),
            ..sample_profile()
        };
        let merged = adapter.merge_config(&oss, &serde_json::json!({}));
        assert_eq!(merged["model_provider"], "oss-custom");
        assert!(merged["model_providers"].get("oss").is_none());
    }

    #[test]
    fn test_merge_applies_codex_model_parameters() {
        let adapter = CodexAdapter::new();
        let profile = ApiProfile {
            model: Some("gpt-5.5".to_string()),
            reasoning_effort: Some("xhigh".to_string()),
            context_1m: Some(true),
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
            reasoning_effort: None,
            context_1m: Some(false),
            ..sample_profile()
        };

        let merged = adapter.merge_config(&profile, &shared);

        assert!(merged.get("model").is_none());
        assert!(merged.get("model_reasoning_effort").is_none());
        assert!(merged.get("model_context_window").is_none());
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
        adapter
            .apply_api_credentials(&sample_profile())
            .unwrap();

        let written: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(config_dir.join("auth.json")).unwrap(),
        )
        .unwrap();
        // key 被更新
        assert_eq!(written["OPENAI_API_KEY"], "sk-test-key");
        // 其他字段保留
        assert_eq!(written["tokens"]["refresh"], "abc");

        let _ = fs::remove_dir_all(&config_dir);
    }
}
