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
    pub fn new() -> Result<Self> {
        // 不用 `expect`：主目录解析不出来时切换会直接 panic，而这是可恢复的
        // 环境异常——返回 Err 让命令层报错即可。
        //
        // 触发条件比想象中窄：macOS/多数 Linux 上 `dirs::home_dir()` 在 `$HOME`
        // 未设时会回退到 getpwuid（实测去掉 HOME 仍返回 /Users/<user>）。
        // 但该回退同样可能失败——容器里没有 passwd 条目、或服务账户无 home。
        // 那种环境下 panic 会让整个切换崩在半途，而不是干净地报错。
        let home =
            dirs::home_dir().ok_or_else(|| anyhow::anyhow!("无法定位用户主目录（HOME 未设置）"))?;
        let config_dir = home.join(".claude");
        Ok(Self { config_dir })
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

    /// 把 MCP 写入 `~/.claude.json`。
    ///
    /// **只改 `mcpServers` 一个键**，其余内容（用户自己的键、Claude Code 的
    /// 运行时状态如 `numStartups`/项目历史、JSONC 注释）一律保留。
    ///
    /// 早期实现是「解析失败就从 `{}` 起步 + 全量序列化写回」：`~/.claude.json`
    /// 是 JSONC（带注释），`serde_json` 严格解析必然失败，于是整个文件被替换成
    /// `{"mcpServers": ...}`——用户的键和运行时状态全丢。实测确认。
    ///
    /// 现在走 `doc` 层的合并路径：读 live 文本 → 只改目标键 → 写回。
    fn apply_auxiliary_config(&self, shared_config: &serde_json::Value) -> Result<()> {
        let Some(mcp) = shared_config.get("mcpServers").cloned() else {
            return Ok(());
        };
        let path = self.claude_json_path();

        let live_text = if path.exists() {
            fs::read_to_string(&path).context("Failed to read claude.json")?
        } else {
            String::new()
        };

        let next = serde_json::json!({ "mcpServers": mcp });
        let content = crate::doc::merge_document(
            crate::doc::DocFormat::Json,
            &live_text,
            // 无 previous：`mcpServers` 是唯一受管键，首次写入时只叠加不摘除，
            // 不会动用户的任何内容。
            None,
            &next,
        )
        .context("Failed to merge claude.json")?;

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
            anyhow::bail!("配置文件不存在");
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

    /// 测试用构造：测试环境必有 HOME，取不到就直接失败。
    fn adapter() -> ClaudeCodeAdapter {
        ClaudeCodeAdapter::new().expect("测试环境应能取到 HOME")
    }
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
        let adapter = adapter();

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
