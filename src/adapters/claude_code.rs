use super::ConfigAdapter;
use crate::models::ApiProfile;
use crate::utils::secure_fs::{atomic_write_private, copy_private};
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

    /// 获取 settings.json 路径（Claude Code 的用户级/全局配置文件）
    fn global_settings_path(&self) -> PathBuf {
        self.config_dir.join("settings.json")
    }
}

impl Default for ClaudeCodeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfigAdapter for ClaudeCodeAdapter {
    fn config_path(&self) -> PathBuf {
        self.global_settings_path()
    }

    fn read_config(&self) -> Result<serde_json::Value> {
        let global_path = self.global_settings_path();
        if global_path.exists() {
            let content =
                fs::read_to_string(&global_path).context("Failed to read global settings")?;
            return serde_json::from_str(&content).context("Failed to parse global settings");
        }

        Ok(serde_json::json!({}))
    }

    /// Claude Code 的 MCP servers 存在 `~/.claude.json`（顶层 mcpServers = 全局），
    /// 不在 settings.json 里。优先读 .claude.json，找不到再回退 settings（兼容老式配置）。
    #[cfg(feature = "tauri-gui")]
    fn read_mcp_servers_raw(&self) -> Result<Option<serde_json::Value>> {
        if let Some(home) = dirs::home_dir() {
            let claude_json = home.join(".claude.json");
            if claude_json.exists() {
                if let Ok(content) = fs::read_to_string(&claude_json) {
                    if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&content) {
                        if let Some(mcp) = cfg.get("mcpServers").cloned() {
                            return Ok(Some(mcp));
                        }
                    }
                }
            }
        }

        let config = self.read_config()?;
        Ok(config
            .get("mcpServers")
            .or_else(|| config.get("mcp_servers"))
            .or_else(|| config.get("mcp"))
            .cloned())
    }

    fn extract_shared_config(&self, config: &serde_json::Value) -> serde_json::Value {
        let mut shared = config.clone();

        // 移除 API / 模型映射相关 env（与 merge_config 写入对称）
        if let Some(env) = shared.get_mut("env").and_then(|v| v.as_object_mut()) {
            env.remove("ANTHROPIC_BASE_URL");
            env.remove("ANTHROPIC_AUTH_TOKEN");
            env.remove("ANTHROPIC_API_KEY");
            env.remove("ANTHROPIC_MODEL");
            for role in ["SONNET", "OPUS", "FABLE", "HAIKU"] {
                env.remove(&format!("ANTHROPIC_DEFAULT_{role}_MODEL"));
                env.remove(&format!("ANTHROPIC_DEFAULT_{role}_MODEL_NAME"));
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

        // 确保 env 对象存在
        if config.get("env").is_none() {
            config["env"] = serde_json::json!({});
        }

        // 设置 API URL / Key / 模型
        if let Some(env) = config.get_mut("env").and_then(|v| v.as_object_mut()) {
            env.insert(
                "ANTHROPIC_BASE_URL".to_string(),
                serde_json::Value::String(api_profile.api_url.clone()),
            );
            env.insert(
                "ANTHROPIC_AUTH_TOKEN".to_string(),
                serde_json::Value::String(api_profile.api_key.clone()),
            );
            // 模型：设了就写入，没设就移除（回退到全局默认）
            match &api_profile.model {
                Some(m) if !m.trim().is_empty() => {
                    env.insert(
                        "ANTHROPIC_MODEL".to_string(),
                        serde_json::Value::String(m.clone()),
                    );
                }
                _ => {
                    env.remove("ANTHROPIC_MODEL");
                }
            }

            // 角色映射（Sonnet/Opus/Fable/Haiku）—— 有则写，无则清（避免旧角色残留覆盖切换后的实际模型）
            let mm = api_profile.claude.model_mapping.as_ref();
            for role in ["sonnet", "opus", "fable", "haiku"] {
                let model = mm
                    .and_then(|m| m.get(&format!("{role}_model")))
                    .map(|s| s.as_str())
                    .filter(|s| !s.is_empty());
                let name = mm
                    .and_then(|m| m.get(&format!("{role}_name")))
                    .map(|s| s.as_str())
                    .filter(|s| !s.is_empty());
                let one_m = mm
                    .and_then(|m| m.get(&format!("{role}_one_m")))
                    .map(|s| s == "true")
                    .unwrap_or(false);
                let upper = role.to_uppercase();
                match model {
                    Some(m) => {
                        // [1M] 后缀 = 声明支持 1M 上下文（写在 _MODEL，不写在 _NAME）
                        let model_val = if one_m {
                            format!("{m}[1M]")
                        } else {
                            m.to_string()
                        };
                        env.insert(
                            format!("ANTHROPIC_DEFAULT_{upper}_MODEL"),
                            serde_json::Value::String(model_val),
                        );
                        match name {
                            Some(n) => {
                                env.insert(
                                    format!("ANTHROPIC_DEFAULT_{upper}_MODEL_NAME"),
                                    serde_json::Value::String(n.to_string()),
                                );
                            }
                            None => {
                                env.remove(&format!("ANTHROPIC_DEFAULT_{upper}_MODEL_NAME"));
                            }
                        }
                    }
                    None => {
                        env.remove(&format!("ANTHROPIC_DEFAULT_{upper}_MODEL"));
                        env.remove(&format!("ANTHROPIC_DEFAULT_{upper}_MODEL_NAME"));
                    }
                }
            }
        }

        config
    }

    fn write_config(&self, config: &serde_json::Value) -> Result<()> {
        let path = self.config_path();

        let content = serde_json::to_string_pretty(config).context("Failed to serialize config")?;
        atomic_write_private(&path, content.as_bytes()).context("Failed to write config")?;

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
            .join(format!("settings.backup.{}.json", timestamp));

        copy_private(&path, &backup_path).context("Failed to backup config")?;

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
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn test_adapter() -> ClaudeCodeAdapter {
        let unique = TEST_DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
        let config_dir = std::env::temp_dir().join(format!(
            "switch-api-claude-adapter-test-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&config_dir).unwrap();
        ClaudeCodeAdapter { config_dir }
    }

    #[test]
    fn test_extract_shared_config() {
        let adapter = ClaudeCodeAdapter::new();

        let config = serde_json::json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://api.anthropic.com",
                "ANTHROPIC_AUTH_TOKEN": "sk-test",
                "ANTHROPIC_MODEL": "claude-opus-4-8",
                "ANTHROPIC_DEFAULT_SONNET_MODEL": "x[1M]",
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
        assert!(shared["env"]["ANTHROPIC_MODEL"].is_null());
        assert!(shared["env"]["ANTHROPIC_DEFAULT_SONNET_MODEL"].is_null());

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
            ..Default::default()
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

    #[test]
    fn test_config_path_returns_settings_json() {
        let adapter = test_adapter();

        assert_eq!(
            adapter.config_path(),
            adapter.config_dir.join("settings.json")
        );

        let _ = fs::remove_dir_all(&adapter.config_dir);
    }

    #[test]
    fn test_read_config_reads_settings_json() {
        let adapter = test_adapter();
        fs::write(
            adapter.global_settings_path(),
            r#"{"env":{"ANTHROPIC_BASE_URL":"https://global.example"}}"#,
        )
        .unwrap();

        let config = adapter.read_config().unwrap();

        assert_eq!(
            config["env"]["ANTHROPIC_BASE_URL"],
            "https://global.example"
        );

        let _ = fs::remove_dir_all(&adapter.config_dir);
    }

    #[test]
    fn test_read_config_ignores_settings_local_json() {
        // 全局配置只认 settings.json；settings.local.json 不再被读取
        let adapter = test_adapter();
        fs::write(
            adapter.config_dir.join("settings.local.json"),
            r#"{"env":{"ANTHROPIC_BASE_URL":"https://local.example"}}"#,
        )
        .unwrap();

        let config = adapter.read_config().unwrap();

        // settings.json 不存在 → 返回空对象，不回退读 local
        assert!(config["env"]["ANTHROPIC_BASE_URL"].is_null());

        let _ = fs::remove_dir_all(&adapter.config_dir);
    }

    #[test]
    fn test_write_config_writes_settings_json() {
        let adapter = test_adapter();
        let config = serde_json::json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://global.example"
            }
        });

        adapter.write_config(&config).unwrap();

        assert!(adapter.global_settings_path().exists());
        assert!(!adapter.config_dir.join("settings.local.json").exists());
        let written: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(adapter.global_settings_path()).unwrap())
                .unwrap();
        assert_eq!(
            written["env"]["ANTHROPIC_BASE_URL"],
            "https://global.example"
        );

        let _ = fs::remove_dir_all(&adapter.config_dir);
    }
}
