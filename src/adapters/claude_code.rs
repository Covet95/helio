use super::{backup, ConfigAdapter};
use crate::models::ApiProfile;
use crate::utils::secure_fs::atomic_write_private;
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

    /// ~/.claude.json 路径（顶层 mcpServers = Claude Code 全局 MCP 的事实位置）
    fn claude_json_path(&self) -> PathBuf {
        self.config_dir
            .parent()
            .map(|p| p.join(".claude.json"))
            .unwrap_or_else(|| {
                dirs::home_dir()
                    .expect("Failed to get home directory")
                    .join(".claude.json")
            })
    }

    /// 读 ~/.claude.json 顶层 mcpServers。读不到/解析失败/无该键 → None（不报错）。
    fn read_claude_json_mcp_servers(&self) -> Option<serde_json::Value> {
        let path = self.claude_json_path();
        if !path.exists() {
            return None;
        }
        let content = fs::read_to_string(path).ok()?;
        let parsed: serde_json::Value = serde_json::from_str(&content).ok()?;
        parsed.get("mcpServers").cloned()
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
    fn read_mcp_servers_raw(&self) -> Result<Option<serde_json::Value>> {
        if let Some(mcp) = self.read_claude_json_mcp_servers() {
            return Ok(Some(mcp));
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

        // MCP：把 ~/.claude.json 的 mcpServers 纳入共享配置（入库，可随数据库迁移）。
        // claude.json 存在时优先，否则保留 config 里老式的 mcpServers。
        if let Some(mcp) = self.read_claude_json_mcp_servers() {
            shared["mcpServers"] = mcp;
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

        // MCP 写回 ~/.claude.json（apply_auxiliary_config），settings.json 不写 mcpServers
        if let Some(obj) = config.as_object_mut() {
            obj.remove("mcpServers");
        }

        config
    }

    fn apply_auxiliary_config(&self, shared_config: &serde_json::Value) -> Result<()> {
        let Some(mcp) = shared_config.get("mcpServers").cloned() else {
            return Ok(());
        };
        let path = self.claude_json_path();
        let mut claude_json: serde_json::Value = if path.exists() {
            fs::read_to_string(&path)
                .ok()
                .and_then(|c| serde_json::from_str(&c).ok())
                .unwrap_or_else(|| serde_json::json!({}))
        } else {
            serde_json::json!({})
        };
        if let Some(obj) = claude_json.as_object_mut() {
            obj.insert("mcpServers".to_string(), mcp);
        }
        let content = serde_json::to_string_pretty(&claude_json).context("Failed to serialize")?;
        atomic_write_private(&path, content.as_bytes()).context("Failed to write claude.json")?;
        Ok(())
    }

    fn managed_paths(&self) -> Vec<PathBuf> {
        vec![self.config_path(), self.claude_json_path()]
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

        let backup_path = backup::backup_required(&self.config_dir, &path, "settings")?;

        // 清理旧备份（保留最近 10 个）
        self.cleanup_old_backups(10)?;

        Ok(backup_path)
    }

    fn cleanup_old_backups(&self, keep: usize) -> Result<()> {
        backup::cleanup_prefix(&self.config_dir, "settings.backup.", keep)
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
        // 用 temp 目录隔离,避免读到真实 ~/.claude.json
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join(".claude");
        fs::create_dir_all(&config_dir).unwrap();
        let adapter = ClaudeCodeAdapter {
            config_dir: config_dir.clone(),
        };

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

        let _ = fs::remove_dir_all(&config_dir);
    }

    #[test]
    fn test_extract_includes_claude_json_mcp_servers() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join(".claude");
        fs::create_dir_all(&config_dir).unwrap();
        let adapter = ClaudeCodeAdapter {
            config_dir: config_dir.clone(),
        };
        fs::write(
            dir.path().join(".claude.json"),
            r#"{"mcpServers":{"bing-search":{"command":"npx"}},"projects":{"p1":"kept"}}"#,
        )
        .unwrap();

        let shared = adapter.extract_shared_config(&serde_json::json!({}));

        assert_eq!(shared["mcpServers"]["bing-search"]["command"], "npx");

        let _ = fs::remove_dir_all(&config_dir);
    }

    #[test]
    fn test_extract_without_claude_json_keeps_settings_mcp() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join(".claude");
        fs::create_dir_all(&config_dir).unwrap();
        let adapter = ClaudeCodeAdapter {
            config_dir: config_dir.clone(),
        };

        // 无 ~/.claude.json,settings.json 里有老式 mcpServers → 保留
        let shared = adapter.extract_shared_config(&serde_json::json!({
            "mcpServers": { "legacy": { "command": "npx" } }
        }));

        assert_eq!(shared["mcpServers"]["legacy"]["command"], "npx");

        let _ = fs::remove_dir_all(&config_dir);
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
            },
            "mcpServers": { "bing-search": { "command": "npx" } }
        });

        let merged = adapter.merge_config(&api_profile, &shared_config);

        // API 字段应该被添加
        assert_eq!(merged["env"]["ANTHROPIC_BASE_URL"], "https://test.api");
        assert_eq!(merged["env"]["ANTHROPIC_AUTH_TOKEN"], "sk-new-key");

        // 共享配置应该保留
        assert_eq!(merged["env"]["OTHER_VAR"], "value");
        assert_eq!(merged["permissions"]["allow"][0], "bash");

        // mcpServers 不写 settings.json（走 ~/.claude.json）
        assert!(merged.get("mcpServers").is_none());
    }

    #[test]
    fn test_apply_auxiliary_config_writes_claude_json_preserving_fields() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join(".claude");
        fs::create_dir_all(&config_dir).unwrap();
        let adapter = ClaudeCodeAdapter {
            config_dir: config_dir.clone(),
        };
        let claude_json = dir.path().join(".claude.json");
        fs::write(&claude_json, r#"{"projects":{"p1":"kept"}}"#).unwrap();

        let shared = serde_json::json!({
            "mcpServers": { "bing-search": { "command": "npx" } }
        });
        adapter.apply_auxiliary_config(&shared).unwrap();

        let written: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&claude_json).unwrap()).unwrap();
        assert_eq!(written["mcpServers"]["bing-search"]["command"], "npx");
        // 其他字段保留
        assert_eq!(written["projects"]["p1"], "kept");

        let _ = fs::remove_dir_all(&config_dir);
    }

    #[test]
    fn test_apply_auxiliary_config_creates_file_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join(".claude");
        fs::create_dir_all(&config_dir).unwrap();
        let adapter = ClaudeCodeAdapter {
            config_dir: config_dir.clone(),
        };
        let claude_json = dir.path().join(".claude.json");

        let shared = serde_json::json!({
            "mcpServers": { "cdp-bridge": { "command": "uvx" } }
        });
        adapter.apply_auxiliary_config(&shared).unwrap();

        assert!(claude_json.exists());
        let written: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&claude_json).unwrap()).unwrap();
        assert_eq!(written["mcpServers"]["cdp-bridge"]["command"], "uvx");

        let _ = fs::remove_dir_all(&config_dir);
    }

    #[test]
    fn test_apply_auxiliary_config_no_mcp_noop() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join(".claude");
        fs::create_dir_all(&config_dir).unwrap();
        let adapter = ClaudeCodeAdapter {
            config_dir: config_dir.clone(),
        };
        let claude_json = dir.path().join(".claude.json");

        adapter
            .apply_auxiliary_config(&serde_json::json!({}))
            .unwrap();

        assert!(!claude_json.exists());

        let _ = fs::remove_dir_all(&config_dir);
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
