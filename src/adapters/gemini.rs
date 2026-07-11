use super::ConfigAdapter;
use crate::models::ApiProfile;
use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

pub struct GeminiAdapter {
    config_dir: PathBuf,
}

impl GeminiAdapter {
    pub fn new() -> Self {
        let home = dirs::home_dir().expect("Failed to get home directory");
        let config_dir = home.join(".gemini");
        Self { config_dir }
    }

    fn settings_path(&self) -> PathBuf {
        self.config_dir.join("settings.json")
    }

    fn env_path(&self) -> PathBuf {
        self.config_dir.join(".env")
    }

    /// 解析 .env 文件为 (有序的行) 形式，保留非 API 行原样。
    /// 返回 (lines) 其中每行是原始字符串。
    fn read_env_lines(&self) -> Vec<String> {
        let path = self.env_path();
        if !path.exists() {
            return Vec::new();
        }
        fs::read_to_string(&path)
            .map(|c| c.lines().map(|l| l.to_string()).collect())
            .unwrap_or_default()
    }

    /// 将新的 API 凭据写入 .env，保留其他变量和注释。
    fn write_env_api(&self, api_key: &str, base_url: Option<&str>) -> Result<()> {
        let existing = self.read_env_lines();
        let mut out: Vec<String> = Vec::new();
        let mut wrote_key = false;
        let mut wrote_url = false;

        for line in existing {
            let trimmed = line.trim_start();
            if trimmed.starts_with("GEMINI_API_KEY=") {
                out.push(format!("GEMINI_API_KEY=\"{}\"", api_key));
                wrote_key = true;
            } else if trimmed.starts_with("GOOGLE_GEMINI_BASE_URL=") {
                if let Some(url) = base_url {
                    out.push(format!("GOOGLE_GEMINI_BASE_URL=\"{}\"", url));
                    wrote_url = true;
                }
                // 如果新 profile 无 base_url，则丢弃旧的 base_url 行
            } else {
                // 保留其他变量和注释原样
                out.push(line);
            }
        }

        if !wrote_key {
            out.push(format!("GEMINI_API_KEY=\"{}\"", api_key));
        }
        if !wrote_url {
            if let Some(url) = base_url {
                out.push(format!("GOOGLE_GEMINI_BASE_URL=\"{}\"", url));
            }
        }

        let path = self.env_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).context("Failed to create Gemini config directory")?;
        }

        let content = out.join("\n") + "\n";
        let temp_path = path.with_extension("env.tmp");
        fs::write(&temp_path, &content).context("Failed to write temp Gemini .env")?;
        fs::rename(&temp_path, &path).context("Failed to rename temp Gemini .env")?;

        Ok(())
    }
}

impl ConfigAdapter for GeminiAdapter {
    fn config_path(&self) -> PathBuf {
        self.settings_path()
    }

    fn read_config(&self) -> Result<serde_json::Value> {
        let path = self.settings_path();

        if !path.exists() {
            return Ok(serde_json::json!({}));
        }

        let content = fs::read_to_string(&path).context("Failed to read Gemini settings")?;
        serde_json::from_str(&content).context("Failed to parse Gemini settings.json")
    }

    fn extract_shared_config(&self, config: &serde_json::Value) -> serde_json::Value {
        // Gemini 的 API key 在 .env，不在 settings.json。
        // 因此整个 settings.json 都是共享配置。
        config.clone()
    }

    fn merge_config(
        &self,
        _api_profile: &ApiProfile,
        shared_config: &serde_json::Value,
    ) -> serde_json::Value {
        // API 凭据写入 .env（在 write_config 中处理），settings.json 原样返回。
        shared_config.clone()
    }

    fn write_config(&self, config: &serde_json::Value) -> Result<()> {
        // 写 settings.json
        let path = self.settings_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).context("Failed to create Gemini config directory")?;
        }

        let content =
            serde_json::to_string_pretty(config).context("Failed to serialize Gemini settings")?;
        let temp_path = path.with_extension("json.tmp");
        fs::write(&temp_path, &content).context("Failed to write temp Gemini settings")?;
        if let Ok(file) = fs::File::open(&temp_path) {
            let _ = file.sync_all();
        }
        fs::rename(&temp_path, &path).context("Failed to rename temp Gemini settings")?;

        Ok(())
    }

    fn backup_config(&self) -> Result<PathBuf> {
        let path = self.settings_path();

        if !path.exists() {
            anyhow::bail!("Config file does not exist");
        }

        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let backup_path = self
            .config_dir
            .join(format!("settings.backup.{}.json", timestamp));

        fs::copy(&path, &backup_path).context("Failed to backup config")?;

        // 同时备份 .env（如果存在）
        let env_path = self.env_path();
        if env_path.exists() {
            let env_backup = self
                .config_dir
                .join(format!("env.backup.{}", timestamp));
            let _ = fs::copy(&env_path, &env_backup);
        }

        self.cleanup_old_backups(10)?;

        Ok(backup_path)
    }

    fn cleanup_old_backups(&self, keep: usize) -> Result<()> {
        let mut backups: Vec<_> = fs::read_dir(&self.config_dir)?
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                let name = entry.file_name().to_string_lossy().to_string();
                name.starts_with("settings.backup.")
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

    /// Gemini 特有：把 API 凭据写入 .env，保留其他变量
    fn apply_api_credentials(&self, api_profile: &ApiProfile) -> Result<()> {
        let base_url = if api_profile.api_url.is_empty()
            || api_profile.api_url == "https://generativelanguage.googleapis.com"
        {
            None
        } else {
            Some(api_profile.api_url.as_str())
        };
        self.write_env_api(&api_profile.api_key, base_url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_shared_returns_all_settings() {
        let adapter = GeminiAdapter::new();
        let config = serde_json::json!({
            "mcpServers": { "fs": { "command": "npx" } },
            "model": { "name": "gemini-pro" },
            "security": { "auth": { "selectedType": "gemini-api-key" } }
        });

        let shared = adapter.extract_shared_config(&config);

        // 整个 settings.json 都是共享配置（API 在 .env）
        assert_eq!(shared["mcpServers"]["fs"]["command"], "npx");
        assert_eq!(shared["model"]["name"], "gemini-pro");
        assert_eq!(shared["security"]["auth"]["selectedType"], "gemini-api-key");
    }

    #[test]
    fn test_merge_preserves_settings() {
        let adapter = GeminiAdapter::new();
        let profile = ApiProfile {
            id: Some(1),
            name: "g".to_string(),
            provider: "google".to_string(),
            api_url: "https://proxy.com".to_string(),
            api_key: "key123".to_string(),
            ..Default::default()
        };
        let shared = serde_json::json!({
            "mcpServers": { "fs": { "command": "npx" } }
        });

        let merged = adapter.merge_config(&profile, &shared);

        // settings.json 原样保留（API 不写入 JSON）
        assert_eq!(merged["mcpServers"]["fs"]["command"], "npx");
        assert!(merged.get("GEMINI_API_KEY").is_none());
        assert!(merged.get("api_key").is_none());
    }
}
