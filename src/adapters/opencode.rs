use super::ConfigAdapter;
use crate::models::ApiProfile;
use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

pub struct OpenCodeAdapter {
    config_dir: PathBuf,
}

impl OpenCodeAdapter {
    pub fn new() -> Self {
        let home = dirs::home_dir().expect("Failed to get home directory");
        let config_dir = home.join(".config").join("opencode");
        Self { config_dir }
    }

    fn config_file_path(&self) -> PathBuf {
        self.config_dir.join("opencode.json")
    }

    /// 去除 JSONC 注释（// 行注释和 /* */ 块注释），简单实现。
    /// 不处理字符串内的 // 等边界情况，对标准 opencode.json 足够。
    fn strip_jsonc_comments(input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        let mut chars = input.chars().peekable();
        let mut in_string = false;
        let mut escaped = false;

        while let Some(c) = chars.next() {
            if in_string {
                out.push(c);
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    in_string = false;
                }
                continue;
            }

            match c {
                '"' => {
                    in_string = true;
                    out.push(c);
                }
                '/' if chars.peek() == Some(&'/') => {
                    // 行注释：跳到行尾
                    chars.next();
                    for nc in chars.by_ref() {
                        if nc == '\n' {
                            out.push('\n');
                            break;
                        }
                    }
                }
                '/' if chars.peek() == Some(&'*') => {
                    // 块注释：跳到 */
                    chars.next();
                    let mut prev = '\0';
                    for nc in chars.by_ref() {
                        if prev == '*' && nc == '/' {
                            break;
                        }
                        prev = nc;
                    }
                }
                _ => out.push(c),
            }
        }

        out
    }

    /// 从 profile.provider 推导 OpenCode provider id（小写）
    fn provider_id(api_profile: &ApiProfile) -> String {
        if api_profile.provider.is_empty() {
            "custom".to_string()
        } else {
            api_profile.provider.to_lowercase()
        }
    }
}

impl ConfigAdapter for OpenCodeAdapter {
    fn config_path(&self) -> PathBuf {
        self.config_file_path()
    }

    fn read_config(&self) -> Result<serde_json::Value> {
        let path = self.config_path();

        if !path.exists() {
            return Ok(serde_json::json!({}));
        }

        let content = fs::read_to_string(&path).context("Failed to read OpenCode config")?;
        let stripped = Self::strip_jsonc_comments(&content);
        serde_json::from_str(&stripped).context("Failed to parse OpenCode config")
    }

    fn extract_shared_config(&self, config: &serde_json::Value) -> serde_json::Value {
        let mut shared = config.clone();

        // 移除每个 provider 的 options.apiKey 和 options.baseURL（API 凭据）
        if let Some(providers) = shared.get_mut("provider").and_then(|v| v.as_object_mut()) {
            for (_id, provider) in providers.iter_mut() {
                if let Some(options) = provider.get_mut("options").and_then(|v| v.as_object_mut()) {
                    options.remove("apiKey");
                    options.remove("baseURL");
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
        let provider_id = Self::provider_id(api_profile);

        if config.get("provider").is_none() {
            config["provider"] = serde_json::json!({});
        }

        if let Some(providers) = config.get_mut("provider").and_then(|v| v.as_object_mut()) {
            let entry = providers
                .entry(provider_id.clone())
                .or_insert_with(|| serde_json::json!({}));
            if let Some(p) = entry.as_object_mut() {
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
                        serde_json::Value::String(api_profile.api_url.clone()),
                    );
                }
            }
        }

        config
    }

    fn write_config(&self, config: &serde_json::Value) -> Result<()> {
        let path = self.config_path();

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).context("Failed to create OpenCode config directory")?;
        }

        let content =
            serde_json::to_string_pretty(config).context("Failed to serialize OpenCode config")?;
        let temp_path = path.with_extension("json.tmp");
        fs::write(&temp_path, &content).context("Failed to write temp OpenCode config")?;
        if let Ok(file) = fs::File::open(&temp_path) {
            let _ = file.sync_all();
        }
        fs::rename(&temp_path, &path).context("Failed to rename temp OpenCode config")?;

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
            .join(format!("opencode.backup.{}.json", timestamp));

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
                    .starts_with("opencode.backup.")
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
            provider: "anthropic".to_string(),
            api_url: "https://api.example.com/v1".to_string(),
            api_key: "sk-test-key".to_string(),
            model_mapping: None,
            ..Default::default()
        }
    }

    #[test]
    fn test_strip_jsonc_comments() {
        let input = r#"{
  // line comment
  "model": "x", /* block */
  "url": "http://a//b"
}"#;
        let out = OpenCodeAdapter::strip_jsonc_comments(input);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["model"], "x");
        // 字符串内的 // 被保留
        assert_eq!(parsed["url"], "http://a//b");
    }

    #[test]
    fn test_extract_shared_removes_api() {
        let adapter = OpenCodeAdapter::new();
        let config = serde_json::json!({
            "provider": {
                "anthropic": {
                    "name": "Anthropic",
                    "options": {
                        "apiKey": "sk-secret",
                        "baseURL": "https://api.com",
                        "timeout": 30000
                    }
                }
            },
            "mcp": { "fs": { "type": "local" } },
            "permission": { "edit": "ask" }
        });

        let shared = adapter.extract_shared_config(&config);

        // API 字段被移除
        assert!(shared["provider"]["anthropic"]["options"].get("apiKey").is_none());
        assert!(shared["provider"]["anthropic"]["options"].get("baseURL").is_none());
        // 非 API option 保留
        assert_eq!(shared["provider"]["anthropic"]["options"]["timeout"], 30000);
        // 共享配置保留
        assert_eq!(shared["provider"]["anthropic"]["name"], "Anthropic");
        assert_eq!(shared["mcp"]["fs"]["type"], "local");
        assert_eq!(shared["permission"]["edit"], "ask");
    }

    #[test]
    fn test_merge_inserts_api() {
        let adapter = OpenCodeAdapter::new();
        let shared = serde_json::json!({
            "mcp": { "fs": { "type": "local" } },
            "permission": { "edit": "ask" }
        });

        let merged = adapter.merge_config(&sample_profile(), &shared);

        // API 写入 provider.anthropic.options
        assert_eq!(merged["provider"]["anthropic"]["options"]["apiKey"], "sk-test-key");
        assert_eq!(
            merged["provider"]["anthropic"]["options"]["baseURL"],
            "https://api.example.com/v1"
        );
        // 共享配置保留
        assert_eq!(merged["mcp"]["fs"]["type"], "local");
        assert_eq!(merged["permission"]["edit"], "ask");
    }

    #[test]
    fn test_merge_preserves_other_providers() {
        let adapter = OpenCodeAdapter::new();
        let shared = serde_json::json!({
            "provider": {
                "openai": {
                    "name": "OpenAI",
                    "options": { "timeout": 5000 }
                }
            }
        });

        let merged = adapter.merge_config(&sample_profile(), &shared);

        // 切换 anthropic 不影响 openai
        assert_eq!(merged["provider"]["openai"]["name"], "OpenAI");
        assert_eq!(merged["provider"]["openai"]["options"]["timeout"], 5000);
        // 新 provider 已添加
        assert_eq!(merged["provider"]["anthropic"]["options"]["apiKey"], "sk-test-key");
    }
}
