use super::ConfigAdapter;
use crate::models::{ApiProfile, TargetApp};
use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

pub struct ClaudeCodeAdapter {
    config_dir: PathBuf,
}

impl ClaudeCodeAdapter {
    pub fn new() -> Self {
        let home = dirs::home_dir().expect("Failed to get home directory");
        let config_dir = home.join(".claude");
        Self { config_dir }
    }

    /// 获取 settings.local.json 路径
    fn local_settings_path(&self) -> PathBuf {
        self.config_dir.join("settings.local.json")
    }

    /// 获取 settings.json 路径
    fn global_settings_path(&self) -> PathBuf {
        self.config_dir.join("settings.json")
    }
}

impl ConfigAdapter for ClaudeCodeAdapter {
    fn target_app(&self) -> TargetApp {
        TargetApp::ClaudeCode
    }

    fn config_path(&self) -> PathBuf {
        // 优先使用 settings.local.json
        self.local_settings_path()
    }

    fn read_config(&self) -> Result<serde_json::Value> {
        let path = self.config_path();

        if !path.exists() {
            // 如果 local 不存在，尝试读取全局配置
            let global_path = self.global_settings_path();
            if global_path.exists() {
                let content = fs::read_to_string(&global_path)
                    .context("Failed to read global settings")?;
                return serde_json::from_str(&content)
                    .context("Failed to parse global settings");
            }

            // 都不存在则返回空配置
            return Ok(serde_json::json!({}));
        }

        let content = fs::read_to_string(&path)
            .context("Failed to read config file")?;

        serde_json::from_str(&content)
            .context("Failed to parse config JSON")
    }

    fn extract_shared_config(&self, config: &serde_json::Value) -> serde_json::Value {
        let mut shared = config.clone();

        // 移除 API 相关字段
        if let Some(env) = shared.get_mut("env").and_then(|v| v.as_object_mut()) {
            env.remove("ANTHROPIC_BASE_URL");
            env.remove("ANTHROPIC_AUTH_TOKEN");
        }

        shared
    }

    fn merge_config(&self, api_profile: &ApiProfile, shared_config: &serde_json::Value) -> serde_json::Value {
        let mut config = shared_config.clone();

        // 确保 env 对象存在
        if config.get("env").is_none() {
            config["env"] = serde_json::json!({});
        }

        // 设置 API URL 和 Key
        if let Some(env) = config.get_mut("env").and_then(|v| v.as_object_mut()) {
            env.insert(
                "ANTHROPIC_BASE_URL".to_string(),
                serde_json::Value::String(api_profile.api_url.clone()),
            );
            env.insert(
                "ANTHROPIC_AUTH_TOKEN".to_string(),
                serde_json::Value::String(api_profile.api_key.clone()),
            );
        }

        config
    }

    fn write_config(&self, config: &serde_json::Value) -> Result<()> {
        let path = self.config_path();

        // 确保配置目录存在
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .context("Failed to create config directory")?;
        }

        // 格式化 JSON（美化输出）
        let content = serde_json::to_string_pretty(config)
            .context("Failed to serialize config")?;

        // 原子写入：先写临时文件，再重命名
        let temp_path = path.with_extension("tmp");

        // 写入临时文件
        fs::write(&temp_path, &content)
            .context("Failed to write temp config file")?;

        // 同步到磁盘（确保数据持久化）
        if let Ok(file) = fs::File::open(&temp_path) {
            let _ = file.sync_all();
        }

        // 原子重命名
        fs::rename(&temp_path, &path)
            .context("Failed to rename temp config to final config")?;

        Ok(())
    }

    fn backup_config(&self) -> Result<PathBuf> {
        let path = self.config_path();

        if !path.exists() {
            anyhow::bail!("Config file does not exist");
        }

        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let backup_path = self.config_dir.join(format!("settings.backup.{}.json", timestamp));

        fs::copy(&path, &backup_path)
            .context("Failed to backup config")?;

        // 清理旧备份（保留最近 10 个）
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
                    .starts_with("settings.backup.")
            })
            .collect();

        // 按修改时间排序（最新的在前）
        backups.sort_by_key(|entry| {
            entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        });
        backups.reverse();

        // 删除多余的备份
        for entry in backups.iter().skip(keep) {
            let _ = fs::remove_file(entry.path());
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_shared_config() {
        let adapter = ClaudeCodeAdapter::new();

        let config = serde_json::json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://api.anthropic.com",
                "ANTHROPIC_AUTH_TOKEN": "sk-test",
                "OTHER_VAR": "value"
            },
            "permissions": {
                "allow": ["bash"]
            }
        });

        let shared = adapter.extract_shared_config(&config);

        // API 字段应该被移除
        assert!(shared["env"]["ANTHROPIC_BASE_URL"].is_null());
        assert!(shared["env"]["ANTHROPIC_AUTH_TOKEN"].is_null());

        // 其他字段应该保留
        assert_eq!(shared["env"]["OTHER_VAR"], "value");
        assert_eq!(shared["permissions"]["allow"][0], "bash");
    }

    #[test]
    fn test_merge_config() {
        let adapter = ClaudeCodeAdapter::new();

        let api_profile = ApiProfile {
            id: Some(1),
            name: "test".to_string(),
            provider: "anthropic".to_string(),
            api_url: "https://test.api".to_string(),
            api_key: "sk-new-key".to_string(),
            model_mapping: None,
            created_at: None,
            updated_at: None,
        };

        let shared_config = serde_json::json!({
            "env": {
                "OTHER_VAR": "value"
            },
            "permissions": {
                "allow": ["bash"]
            }
        });

        let merged = adapter.merge_config(&api_profile, &shared_config);

        // API 字段应该被添加
        assert_eq!(merged["env"]["ANTHROPIC_BASE_URL"], "https://test.api");
        assert_eq!(merged["env"]["ANTHROPIC_AUTH_TOKEN"], "sk-new-key");

        // 共享配置应该保留
        assert_eq!(merged["env"]["OTHER_VAR"], "value");
        assert_eq!(merged["permissions"]["allow"][0], "bash");
    }
}
