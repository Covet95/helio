use crate::models::{ApiProfile, TargetApp};
use anyhow::Result;
use std::fs;
use std::path::PathBuf;

use crate::utils::secure_fs::atomic_write_private;

#[derive(Debug)]
pub struct FileSnapshot {
    path: PathBuf,
    contents: Option<Vec<u8>>,
}

#[derive(Debug)]
pub struct ProfileApplicationResult {
    pub backup_path: Option<PathBuf>,
    pub config_path: PathBuf,
}

/// 配置适配器 trait
pub trait ConfigAdapter {
    /// 配置文件路径
    fn config_path(&self) -> PathBuf;

    /// 读取当前配置
    fn read_config(&self) -> Result<serde_json::Value>;

    /// 读取 MCP servers 的原始 JSON（mcpServers / mcp_servers / mcp）。
    #[cfg(feature = "tauri-gui")]
    fn read_mcp_servers_raw(&self) -> Result<Option<serde_json::Value>> {
        let config = self.read_config()?;
        Ok(config
            .get("mcpServers")
            .or_else(|| config.get("mcp_servers"))
            .or_else(|| config.get("mcp"))
            .cloned())
    }

    /// 提取共享配置（排除 API 信息）
    fn extract_shared_config(&self, config: &serde_json::Value) -> serde_json::Value;

    /// 合并 API Profile 和共享配置
    fn merge_config(
        &self,
        api_profile: &ApiProfile,
        shared_config: &serde_json::Value,
    ) -> serde_json::Value;

    /// 原子写入配置
    fn write_config(&self, config: &serde_json::Value) -> Result<()>;

    /// 备份配置
    fn backup_config(&self) -> Result<PathBuf>;

    /// 清理旧备份
    fn cleanup_old_backups(&self, keep: usize) -> Result<()>;

    fn managed_paths(&self) -> Vec<PathBuf> {
        vec![self.config_path()]
    }

    fn snapshot_files(&self) -> Result<Vec<FileSnapshot>> {
        self.managed_paths()
            .into_iter()
            .map(|path| {
                let contents = if path.exists() {
                    Some(fs::read(&path)?)
                } else {
                    None
                };
                Ok(FileSnapshot { path, contents })
            })
            .collect()
    }

    fn restore_files(&self, snapshots: &[FileSnapshot]) -> Result<()> {
        let mut first_error = None;
        for snapshot in snapshots {
            let result: Result<()> = match &snapshot.contents {
                Some(contents) => atomic_write_private(&snapshot.path, contents),
                None if snapshot.path.exists() => {
                    fs::remove_file(&snapshot.path).map_err(Into::into)
                }
                None => Ok(()),
            };
            if let Err(error) = result {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// 应用 API 凭据到工具特定的位置（默认无操作）。
    /// 大多数工具的 API 凭据通过 merge_config 写入主配置文件即可。
    /// Pi 等工具的 key 在 auth.json / models.json，需要重写此方法。
    fn apply_api_credentials(&self, _api_profile: &ApiProfile) -> Result<()> {
        Ok(())
    }
}

pub fn apply_profile_transaction(
    adapter: &dyn ConfigAdapter,
    api_profile: &ApiProfile,
    shared_config: &serde_json::Value,
) -> Result<()> {
    let snapshots = adapter.snapshot_files()?;
    let merged = adapter.merge_config(api_profile, shared_config);
    if let Err(error) = adapter
        .write_config(&merged)
        .and_then(|_| adapter.apply_api_credentials(api_profile))
    {
        if let Err(restore_error) = adapter.restore_files(&snapshots) {
            anyhow::bail!("{error}; rollback failed: {restore_error}");
        }
        return Err(error);
    }
    Ok(())
}

pub fn resolve_shared_config(
    target_app: TargetApp,
    persisted_shared_config: Option<serde_json::Value>,
) -> Result<serde_json::Value> {
    let adapter = get_adapter(target_app);
    let mut shared_config = if adapter.config_path().exists() {
        let current_config = adapter.read_config()?;
        adapter.extract_shared_config(&current_config)
    } else {
        persisted_shared_config
            .clone()
            .unwrap_or_else(|| serde_json::json!({}))
    };

    if let Some(previous) = persisted_shared_config {
        backfill_missing_top_level(&mut shared_config, &previous);
    }
    Ok(shared_config)
}

pub fn apply_profile_configuration(
    target_app: TargetApp,
    api_profile: &ApiProfile,
    shared_config: &serde_json::Value,
    create_backup: bool,
) -> Result<ProfileApplicationResult> {
    let adapter = get_adapter(target_app);
    let backup_path = if create_backup && adapter.config_path().exists() {
        Some(adapter.backup_config()?)
    } else {
        None
    };
    apply_profile_transaction(adapter.as_ref(), api_profile, shared_config)?;
    Ok(ProfileApplicationResult {
        backup_path,
        config_path: adapter.config_path(),
    })
}

pub fn backfill_missing_top_level(live: &mut serde_json::Value, previous: &serde_json::Value) {
    if let (Some(live_object), Some(previous_object)) = (live.as_object_mut(), previous.as_object())
    {
        for (key, value) in previous_object {
            if !live_object.contains_key(key) {
                live_object.insert(key.clone(), value.clone());
            }
        }
    }
}

pub mod backup;
pub mod claude_code;
pub mod codex;
pub mod hermes;
pub mod openclaw;
pub mod opencode;
pub mod pi;

/// 获取适配器
pub fn get_adapter(target_app: TargetApp) -> Box<dyn ConfigAdapter> {
    match target_app {
        TargetApp::ClaudeCode => Box::new(claude_code::ClaudeCodeAdapter::new()),
        TargetApp::Codex => Box::new(codex::CodexAdapter::new()),
        TargetApp::Pi => Box::new(pi::PiAdapter::new()),
        TargetApp::OpenCode => Box::new(opencode::OpenCodeAdapter::new()),
        TargetApp::Hermes => Box::new(hermes::HermesAdapter::new()),
        TargetApp::OpenClaw => Box::new(openclaw::OpenClawAdapter::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::{apply_profile_transaction, ConfigAdapter};
    use crate::models::ApiProfile;
    use anyhow::Result;
    use std::fs;
    use std::path::PathBuf;

    struct FailingAdapter {
        config: PathBuf,
        credentials: PathBuf,
    }

    impl ConfigAdapter for FailingAdapter {
        fn config_path(&self) -> PathBuf {
            self.config.clone()
        }
        fn read_config(&self) -> Result<serde_json::Value> {
            Ok(serde_json::json!({}))
        }
        fn extract_shared_config(&self, config: &serde_json::Value) -> serde_json::Value {
            config.clone()
        }
        fn merge_config(&self, _: &ApiProfile, _: &serde_json::Value) -> serde_json::Value {
            serde_json::json!({"changed": true})
        }
        fn write_config(&self, _: &serde_json::Value) -> Result<()> {
            fs::write(&self.config, b"changed")?;
            Ok(())
        }
        fn backup_config(&self) -> Result<PathBuf> {
            Ok(self.config.clone())
        }
        fn cleanup_old_backups(&self, _: usize) -> Result<()> {
            Ok(())
        }
        fn managed_paths(&self) -> Vec<PathBuf> {
            vec![self.config.clone(), self.credentials.clone()]
        }
        fn apply_api_credentials(&self, _: &ApiProfile) -> Result<()> {
            fs::write(&self.credentials, b"new secret")?;
            anyhow::bail!("injected credential write failure")
        }
    }

    #[test]
    fn transaction_restores_existing_files_after_failure() {
        let dir = tempfile::tempdir().unwrap();
        let adapter = FailingAdapter {
            config: dir.path().join("config"),
            credentials: dir.path().join("auth"),
        };
        fs::write(&adapter.config, b"old config").unwrap();
        fs::write(&adapter.credentials, b"old secret").unwrap();

        let error =
            apply_profile_transaction(&adapter, &ApiProfile::default(), &serde_json::json!({}))
                .unwrap_err();
        assert!(error.to_string().contains("injected"));
        assert_eq!(fs::read(&adapter.config).unwrap(), b"old config");
        assert_eq!(fs::read(&adapter.credentials).unwrap(), b"old secret");
    }

    #[test]
    fn transaction_removes_new_files_after_failure() {
        let dir = tempfile::tempdir().unwrap();
        let adapter = FailingAdapter {
            config: dir.path().join("config"),
            credentials: dir.path().join("auth"),
        };

        assert!(apply_profile_transaction(
            &adapter,
            &ApiProfile::default(),
            &serde_json::json!({})
        )
        .is_err());
        assert!(!adapter.config.exists());
        assert!(!adapter.credentials.exists());
    }
}
