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

    /// 写入主配置文件之外的辅助文件（如 Claude 的 ~/.claude.json 里的 MCP）。
    /// 默认无操作；实现出错时整个切换事务回滚。
    fn apply_auxiliary_config(&self, _shared_config: &serde_json::Value) -> Result<()> {
        Ok(())
    }

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
        restore_snapshots(snapshots)
    }

    /// 应用 API 凭据到工具特定的位置（默认无操作）。
    /// 大多数工具的 API 凭据通过 merge_config 写入主配置文件即可。
    /// Pi 等工具的 key 在 auth.json / models.json，需要重写此方法。
    fn apply_api_credentials(&self, _api_profile: &ApiProfile) -> Result<()> {
        Ok(())
    }
}

/// 把前镜像逐文件写回（存在 → 原子写回；不存在 → 删除）。失败不中断，收集首个错误。
/// 供事务错误回滚与崩溃恢复 journal 共用。
pub fn restore_snapshots(snapshots: &[FileSnapshot]) -> Result<()> {
    let mut first_error = None;
    for snapshot in snapshots {
        let result: Result<()> = match &snapshot.contents {
            Some(contents) => atomic_write_private(&snapshot.path, contents),
            None if snapshot.path.exists() => fs::remove_file(&snapshot.path).map_err(Into::into),
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
        .and_then(|_| adapter.apply_auxiliary_config(shared_config))
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
        backfill_mcp_entries(&mut shared_config, &previous);
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

/// 一次完整的配置切换（CLI / GUI / 托盘共用入口）：
/// 写 journal（意图 + 前镜像 + 旧 active）→ 若已是目标 profile 则先清 active →
/// DB 写 shared_config → 写配置文件（含备份）→ 记 active_profile → 删 journal。
///
/// 清 active 是为了「重复切换同一 profile」时的崩溃恢复：若不先清，半完成时
/// `active_profile` 仍等于目标，恢复逻辑会误判「已完成」而保留半状态配置。
///
/// 软失败（本进程内 Err）会**立即**按 journal 回滚，不把半状态留给下次启动。
/// 进程崩溃后由 `journal::recover_interrupted_switch` 在下次启动时完成同样恢复。
pub fn apply_profile_switch(
    db: &crate::db::Database,
    target_app: TargetApp,
    api_profile: &ApiProfile,
    shared_config: &serde_json::Value,
    create_backup: bool,
) -> Result<ProfileApplicationResult> {
    let profile_id = api_profile
        .id
        .ok_or_else(|| anyhow::anyhow!("Profile '{}' has no id", api_profile.name))?;
    let adapter = get_adapter(target_app);
    let journal = journal::begin_switch(
        db,
        adapter.as_ref(),
        target_app,
        profile_id,
        &api_profile.name,
    )?;

    let result = (|| -> Result<ProfileApplicationResult> {
        // 制造 `active != target` 窗口：仅当当前 active 已是目标时需要。
        // 不同 profile 之间切换时 active 本来就不是目标，无需动。
        let already_active = db
            .get_active_profile(target_app)?
            .map(|a| a.profile_id == profile_id)
            .unwrap_or(false);
        if already_active {
            db.clear_active_profile(target_app)?;
        }
        db.save_shared_config(target_app, shared_config.clone())?;
        let applied =
            apply_profile_configuration(target_app, api_profile, shared_config, create_backup)?;
        db.set_active_profile(target_app, profile_id)?;
        Ok(applied)
    })();

    match result {
        Ok(applied) => {
            if let Some(journal) = journal {
                if let Err(error) = journal.commit() {
                    // 切换已完成，清理失败仅导致下次启动多一次「已完成」判定。
                    tracing::warn!("{error:#}");
                }
            }
            Ok(applied)
        }
        Err(error) => {
            // 软失败立即按 journal 回滚（与启动恢复同一路径），避免半状态一直留到下次启动。
            // 无 journal 的场景（:memory: 测试库）只能返回原错误。
            if journal.is_some() {
                if let Err(recover_error) = journal::recover_interrupted_switch(db) {
                    return Err(anyhow::anyhow!(
                        "{error}; immediate journal recovery also failed: {recover_error}"
                    ));
                }
            }
            Err(error)
        }
    }
}

#[cfg(test)]
mod switch_journal_tests {
    use super::*;
    use crate::db::Database;
    use crate::models::ApiProfile;
    use std::fs;
    use std::path::PathBuf;

    // 验证：begin_switch 遇到残留 journal 时会先恢复，且恢复失败时拒绝开新事务。
    #[test]
    fn begin_switch_refuses_to_overwrite_unrecoverable_journal() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let db_path = dir.path().join("db.sqlite");
        let db = Database::open(&db_path)?;

        // 未知 version：recover 会忽略并保留 journal。
        let journal_path = PathBuf::from(format!("{}.switch-journal.json", db_path.display()));
        fs::write(
            &journal_path,
            br#"{
              "version": 999,
              "app": "claude-code",
              "profile_id": 1,
              "profile_name": "x",
              "created_at": 0,
              "previous_shared_config": null,
              "snapshots": []
            }"#,
        )?;

        struct Dummy;
        impl ConfigAdapter for Dummy {
            fn config_path(&self) -> PathBuf {
                PathBuf::from("/tmp/unused")
            }
            fn read_config(&self) -> Result<serde_json::Value> {
                Ok(serde_json::json!({}))
            }
            fn extract_shared_config(&self, c: &serde_json::Value) -> serde_json::Value {
                c.clone()
            }
            fn merge_config(&self, _: &ApiProfile, s: &serde_json::Value) -> serde_json::Value {
                s.clone()
            }
            fn write_config(&self, _: &serde_json::Value) -> Result<()> {
                Ok(())
            }
            fn backup_config(&self) -> Result<PathBuf> {
                Ok(PathBuf::from("/tmp/unused"))
            }
            fn cleanup_old_backups(&self, _: usize) -> Result<()> {
                Ok(())
            }
        }

        let err = journal::begin_switch(&db, &Dummy, TargetApp::ClaudeCode, 1, "x").unwrap_err();
        assert!(
            err.to_string().contains("恢复未完成") || err.to_string().contains("journal"),
            "unexpected: {err}"
        );
        assert!(journal_path.exists(), "不得覆盖未恢复的 journal");
        Ok(())
    }
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

/// 对 MCP 键做条目级补缺:previous(数据库)有而 live(磁盘)缺失的 MCP 条目补入 live,
/// live 已有条目以 live 为准(磁盘优先)。openclaw 的 `mcp.servers` 嵌套层同样处理。
fn backfill_mcp_entries(live: &mut serde_json::Value, previous: &serde_json::Value) {
    const MCP_KEYS: &[&str] = &["mcpServers", "mcp_servers", "mcp"];
    let (Some(previous_object), Some(live_object)) = (previous.as_object(), live.as_object_mut())
    else {
        return;
    };

    for key in MCP_KEYS {
        let Some(previous_value) = previous_object.get(*key) else {
            continue;
        };
        let Some(previous_map) = previous_value.as_object() else {
            continue;
        };

        let live_value = live_object
            .entry((*key).to_string())
            .or_insert_with(|| serde_json::json!({}));
        if let Some(live_map) = live_value.as_object_mut() {
            for (name, value) in previous_map {
                if !live_map.contains_key(name) {
                    live_map.insert(name.clone(), value.clone());
                }
            }
        }

        // openclaw: mcp.servers 深一层补缺
        if *key == "mcp" {
            if let Some(previous_servers) =
                previous_value.get("servers").and_then(|v| v.as_object())
            {
                if let Some(live_servers) = live_value
                    .get_mut("servers")
                    .and_then(|v| v.as_object_mut())
                {
                    for (name, value) in previous_servers {
                        if !live_servers.contains_key(name) {
                            live_servers.insert(name.clone(), value.clone());
                        }
                    }
                }
            }
        }
    }
}

pub mod backup;
pub mod claude_code;
pub mod codex;
pub mod hermes;
pub mod journal;
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

    struct FailingAuxAdapter {
        config: PathBuf,
        aux: PathBuf,
    }

    impl ConfigAdapter for FailingAuxAdapter {
        fn config_path(&self) -> PathBuf {
            self.config.clone()
        }
        fn read_config(&self) -> Result<serde_json::Value> {
            Ok(serde_json::json!({}))
        }
        fn extract_shared_config(&self, config: &serde_json::Value) -> serde_json::Value {
            config.clone()
        }
        fn merge_config(&self, _: &ApiProfile, shared: &serde_json::Value) -> serde_json::Value {
            shared.clone()
        }
        fn write_config(&self, config: &serde_json::Value) -> Result<()> {
            fs::write(&self.config, serde_json::to_vec(config)?)?;
            Ok(())
        }
        fn backup_config(&self) -> Result<PathBuf> {
            Ok(self.config.clone())
        }
        fn cleanup_old_backups(&self, _: usize) -> Result<()> {
            Ok(())
        }
        fn managed_paths(&self) -> Vec<PathBuf> {
            vec![self.config.clone(), self.aux.clone()]
        }
        fn apply_auxiliary_config(&self, _: &serde_json::Value) -> Result<()> {
            anyhow::bail!("injected auxiliary write failure")
        }
    }

    #[test]
    fn transaction_restores_all_managed_files_after_aux_failure() {
        let dir = tempfile::tempdir().unwrap();
        let adapter = FailingAuxAdapter {
            config: dir.path().join("config"),
            aux: dir.path().join("claude.json"),
        };
        fs::write(&adapter.config, b"old config").unwrap();
        fs::write(&adapter.aux, b"old mcp").unwrap();

        let error = apply_profile_transaction(
            &adapter,
            &ApiProfile::default(),
            &serde_json::json!({ "mcpServers": { "x": { "command": "y" } } }),
        )
        .unwrap_err();
        assert!(error.to_string().contains("injected"));
        assert_eq!(fs::read(&adapter.config).unwrap(), b"old config");
        assert_eq!(fs::read(&adapter.aux).unwrap(), b"old mcp");
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

    #[test]
    fn backfill_mcp_entries_fills_missing_codex_servers() {
        let mut live = serde_json::json!({ "mcp_servers": {} });
        let previous = serde_json::json!({
            "mcp_servers": {
                "bing-search": { "command": "npx" },
                "github": { "url": "https://x/mcp" }
            }
        });
        super::backfill_mcp_entries(&mut live, &previous);
        assert_eq!(live["mcp_servers"]["bing-search"]["command"], "npx");
        assert_eq!(live["mcp_servers"]["github"]["url"], "https://x/mcp");
    }

    #[test]
    fn backfill_mcp_entries_keeps_live_conflict() {
        let mut live = serde_json::json!({
            "mcp_servers": { "github": { "url": "https://live/mcp" } }
        });
        let previous = serde_json::json!({
            "mcp_servers": { "github": { "url": "https://db/mcp" }, "new": { "command": "x" } }
        });
        super::backfill_mcp_entries(&mut live, &previous);
        assert_eq!(live["mcp_servers"]["github"]["url"], "https://live/mcp");
        assert_eq!(live["mcp_servers"]["new"]["command"], "x");
    }

    #[test]
    fn backfill_mcp_entries_merges_openclaw_nested_servers() {
        let mut live = serde_json::json!({ "mcp": {} });
        let previous = serde_json::json!({
            "mcp": { "servers": { "cdp-bridge": { "command": "uvx" } } }
        });
        super::backfill_mcp_entries(&mut live, &previous);
        assert_eq!(live["mcp"]["servers"]["cdp-bridge"]["command"], "uvx");
    }

    #[test]
    fn backfill_mcp_entries_skips_non_object_values() {
        let mut live = serde_json::json!({});
        let previous = serde_json::json!({ "mcp_servers": "nope" });
        super::backfill_mcp_entries(&mut live, &previous);
        assert!(live.get("mcp_servers").is_none());
    }
}
