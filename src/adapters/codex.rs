use super::ConfigAdapter;
use crate::models::{ApiProfile, TargetApp};
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
    fn target_app(&self) -> TargetApp {
        TargetApp::Codex
    }

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

        // Codex 的 API 凭据位于 model_providers.<provider>.base_url 和顶层 env 中的 key 引用。
        // 我们移除顶层的 api_key（如果存在）以及 model_providers 下各 provider 的 base_url。
        if let Some(obj) = shared.as_object_mut() {
            obj.remove("api_key");

            // 移除各 model_provider 的 base_url（API 端点信息）
            if let Some(providers) = obj.get_mut("model_providers").and_then(|v| v.as_object_mut()) {
                for (_name, provider) in providers.iter_mut() {
                    if let Some(p) = provider.as_object_mut() {
                        p.remove("base_url");
                        p.remove("env_key");
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

        // 使用 profile.provider 作为 provider id（默认 "openai" 风格）
        let provider_id = if api_profile.provider.is_empty() {
            "custom".to_string()
        } else {
            api_profile.provider.to_lowercase()
        };

        // 写入 provider 配置
        if let Some(providers) = config
            .get_mut("model_providers")
            .and_then(|v| v.as_object_mut())
        {
            let entry = providers
                .entry(provider_id.clone())
                .or_insert_with(|| serde_json::json!({}));
            if let Some(p) = entry.as_object_mut() {
                p.insert(
                    "base_url".to_string(),
                    serde_json::Value::String(api_profile.api_url.clone()),
                );
                // Codex 通过 env_key 引用环境变量中的 key
                p.insert(
                    "env_key".to_string(),
                    serde_json::Value::String(format!(
                        "{}_API_KEY",
                        provider_id.to_uppercase()
                    )),
                );
            }
        }

        // 设置当前使用的 provider 和顶层 api_key
        config["model_provider"] = serde_json::Value::String(provider_id);
        config["api_key"] = serde_json::Value::String(api_profile.api_key.clone());

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
            created_at: None,
            updated_at: None,
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

        // API 字段被写入
        assert_eq!(merged["model_provider"], "openai");
        assert_eq!(merged["api_key"], "sk-test-key");
        assert_eq!(
            merged["model_providers"]["openai"]["base_url"],
            "https://api.example.com/v1"
        );
        // 共享配置保留
        assert_eq!(merged["mcp_servers"]["fs"]["command"], "npx");
    }
}
