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
}

impl ConfigAdapter for CodexAdapter {
    fn target_app(&self) -> TargetApp {
        TargetApp::Codex
    }

    fn config_path(&self) -> PathBuf {
        self.config_file_path()
    }

    fn read_config(&self) -> Result<serde_json::Value> {
        // TODO: Codex 使用 TOML 格式，需要实现 TOML 解析
        // 目前返回空配置
        Ok(serde_json::json!({}))
    }

    fn extract_shared_config(&self, config: &serde_json::Value) -> serde_json::Value {
        // TODO: 实现 Codex 的共享配置提取逻辑
        config.clone()
    }

    fn merge_config(&self, _api_profile: &ApiProfile, shared_config: &serde_json::Value) -> serde_json::Value {
        // TODO: 实现 Codex 的配置合并逻辑
        shared_config.clone()
    }

    fn write_config(&self, _config: &serde_json::Value) -> Result<()> {
        // TODO: 实现 Codex 的配置写入逻辑（TOML 格式）
        anyhow::bail!("Codex adapter not fully implemented yet")
    }

    fn backup_config(&self) -> Result<PathBuf> {
        let path = self.config_path();

        if !path.exists() {
            anyhow::bail!("Config file does not exist");
        }

        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let backup_path = self.config_dir.join(format!("config.backup.{}.toml", timestamp));

        fs::copy(&path, &backup_path)
            .context("Failed to backup config")?;

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
