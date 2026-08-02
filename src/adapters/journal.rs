//! 配置切换事务的崩溃一致性 journal。
//!
//! 一次切换 = 重写多个配置文件（每文件原子、跨文件非原子）+ 更新 DB 的
//! `shared_configs` / `active_profiles`。进程在这两步之间崩溃会留下「配置已切、
//! DB 未记」或反之的半状态，且事后无法区分。做法：
//!
//! 1. 切换前把所有受管文件的前镜像 + 旧的 shared_config + 旧 active + 意图写入
//!    `{db_path}.switch-journal.json`（原子写入、0600，可能含明文 key）；
//! 2. 若当前 active 已是目标 profile，先 `clear_active_profile`，制造
//!    `active != 目标` 窗口（重复切换同一 profile 时否则无法区分半完成）；
//! 3. 写 shared_config / 配置文件 / set_active_profile，最后删除 journal；
//! 4. 下次启动（或软失败立即恢复）若发现 journal：DB 的 active_profile 已指向
//!    目标 profile → 切换实际已完成，仅清理 journal；否则按前镜像回滚文件、
//!    shared_config 与 active_profile。
//!
//! 不变量：journal 存在的窗口内，`active_profile == 目标` 当且仅当全部配置已被
//! 切换（含 `set_active_profile` 已执行）。因此「清理」与「回滚」都安全。

use crate::adapters::{restore_snapshots, ConfigAdapter, FileSnapshot};
use crate::db::Database;
use crate::models::TargetApp;
use crate::utils::secure_fs::atomic_write_private;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const JOURNAL_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct SnapshotEntry {
    path: String,
    /// hex 编码的文件前镜像；None = 原文件不存在。
    contents: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Journal {
    version: u32,
    app: String,
    profile_id: i64,
    profile_name: String,
    created_at: i64,
    /// None = 切换前 DB 里没有 shared_config（回滚时删行）。
    previous_shared_config: Option<Value>,
    /// 切换前的 active profile id。None = 当时无 active（回滚时 clear）。
    /// `#[serde(default)]`：兼容旧 journal（缺字段按 None，不强制回滚 active）。
    #[serde(default)]
    previous_active_profile_id: Option<i64>,
    snapshots: Vec<SnapshotEntry>,
}

fn journal_path(db_path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.switch-journal.json", db_path.display()))
}

/// 切换前记录意图与所有前镜像。`:memory:` 库不落盘，返回 `None`。
pub fn begin_switch(
    db: &Database,
    adapter: &dyn ConfigAdapter,
    target_app: TargetApp,
    profile_id: i64,
    profile_name: &str,
) -> Result<Option<SwitchJournal>> {
    let Some(db_path) = db.db_path() else {
        return Ok(None);
    };
    // rusqlite 对内存库返回空路径(个别情况 ":memory:")，不落盘。
    if db_path.as_os_str().is_empty() || db_path.as_os_str() == ":memory:" {
        return Ok(None);
    }
    // 若已有残留 journal（上次软失败未清 / 崩溃后未重启就再次切换），
    // 先按不变量恢复，再开新事务，避免覆盖掉仍可用于回滚的前镜像。
    // 恢复后 journal 仍在 = 回滚未完成，禁止开新事务盖掉旧前镜像。
    let existing = journal_path(&db_path);
    if existing.exists() {
        recover_interrupted_switch(db)?;
        if existing.exists() {
            anyhow::bail!(
                "上次切换的恢复未完成（journal 仍在 {}），请检查配置文件权限后重试",
                existing.display()
            );
        }
    }

    let snapshots = adapter
        .snapshot_files()
        .context("Failed to snapshot managed config files before switch")?;
    let previous_shared_config = db.get_shared_config(target_app)?.map(|sc| sc.config);
    let previous_active_profile_id = db.get_active_profile(target_app)?.map(|a| a.profile_id);
    let journal = Journal {
        version: JOURNAL_VERSION,
        app: target_app.as_str().to_string(),
        profile_id,
        profile_name: profile_name.to_string(),
        created_at: chrono::Utc::now().timestamp(),
        previous_shared_config,
        previous_active_profile_id,
        snapshots: snapshots
            .iter()
            .map(|s| SnapshotEntry {
                path: s.path.to_string_lossy().to_string(),
                contents: s.contents.as_ref().map(|b| hex_encode(b)),
            })
            .collect(),
    };
    let path = journal_path(&db_path);
    let bytes =
        serde_json::to_vec_pretty(&journal).context("Failed to serialize switch journal")?;
    atomic_write_private(&path, &bytes)
        .with_context(|| format!("Failed to write switch journal {}", path.display()))?;
    Ok(Some(SwitchJournal { path }))
}

/// 切换完成后删除 journal。失败只影响下次启动多一次「已完成」判定，不视为切换失败。
#[derive(Debug)]
pub struct SwitchJournal {
    path: PathBuf,
}

impl SwitchJournal {
    pub fn commit(self) -> Result<()> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(anyhow!(
                "Failed to remove switch journal {}: {e}",
                self.path.display()
            )),
        }
    }
}

/// 启动时调用：发现残留 journal 则按不变量完成或回滚。
///
/// 任何解析/回滚失败都只告警、不阻止启动（配置文件通常未损坏，
/// 残留 journal 仅意味着切换窗口内崩溃过）。
pub fn recover_interrupted_switch(db: &Database) -> Result<()> {
    let Some(db_path) = db.db_path() else {
        return Ok(());
    };
    if db_path.as_os_str().is_empty() || db_path.as_os_str() == ":memory:" {
        return Ok(());
    }
    let path = journal_path(&db_path);
    if !path.exists() {
        return Ok(());
    }

    let journal: Journal = match serde_json::from_slice(&fs::read(&path)?) {
        Ok(journal) => journal,
        Err(error) => {
            tracing::warn!(
                "忽略无法解析的 switch journal {} ({error})，请人工检查配置状态",
                path.display()
            );
            return Ok(());
        }
    };
    if journal.version != JOURNAL_VERSION {
        tracing::warn!(
            "忽略未知版本({})的 switch journal {}",
            journal.version,
            path.display()
        );
        return Ok(());
    }
    let Some(app) = TargetApp::parse(&journal.app) else {
        tracing::warn!(
            "忽略未知 target_app `{}` 的 switch journal {}",
            journal.app,
            path.display()
        );
        return Ok(());
    };

    // 不变量判定：active_profile 已指向目标 → 切换实际完成，仅清理。
    let completed = db
        .get_active_profile_full(app)?
        .map(|p| p.id == Some(journal.profile_id))
        .unwrap_or(false);
    if completed {
        tracing::info!(
            "上次切换({} → {})实际已完成，清理事务 journal",
            journal.profile_name,
            app
        );
        remove_journal(&path);
        return Ok(());
    }

    tracing::warn!(
        "检测到未完成的切换({} → {})，按前镜像回滚配置文件与共享配置",
        journal.profile_name,
        app
    );
    let snapshots = journal
        .snapshots
        .iter()
        .map(|s| {
            let contents = match &s.contents {
                Some(hex) => Some(
                    hex_decode(hex)
                        .with_context(|| format!("Failed to decode snapshot of {}", s.path))?,
                ),
                None => None,
            };
            Ok(FileSnapshot {
                path: PathBuf::from(&s.path),
                contents,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    if let Err(error) = restore_snapshots(&snapshots) {
        tracing::warn!(
            "回滚配置文件失败({error:#})，保留 journal {} 待下次重试",
            path.display()
        );
        return Ok(());
    }
    if let Err(error) = match &journal.previous_shared_config {
        Some(config) => db.save_shared_config(app, config.clone()),
        None => db.delete_shared_config(app),
    } {
        tracing::warn!(
            "回滚共享配置失败({error:#})，保留 journal {} 待下次重试",
            path.display()
        );
        return Ok(());
    }
    // 恢复切换前的 active：重复切换同一 profile 时事务中会 clear_active，
    // 若不写回，回滚后 UI/状态会显示「无活动档案」。
    if let Err(error) = restore_previous_active(db, app, journal.previous_active_profile_id) {
        tracing::warn!(
            "回滚 active_profile 失败({error:#})，保留 journal {} 待下次重试",
            path.display()
        );
        return Ok(());
    }
    remove_journal(&path);
    tracing::info!("已回滚未完成的切换({} → {})", journal.profile_name, app);
    Ok(())
}

fn restore_previous_active(
    db: &Database,
    app: TargetApp,
    previous_active_profile_id: Option<i64>,
) -> Result<()> {
    match previous_active_profile_id {
        Some(id) => {
            // 档案可能已在 journal 窗口外被删除；不存在则 clear，避免 FK 失败卡死恢复。
            if db.get_profile_by_id(id)?.is_some() {
                db.set_active_profile(app, id)?;
            } else {
                db.clear_active_profile(app)?;
            }
        }
        None => db.clear_active_profile(app)?,
    }
    Ok(())
}

fn remove_journal(path: &Path) {
    if let Err(error) = fs::remove_file(path) {
        if error.kind() != io::ErrorKind::NotFound {
            tracing::warn!("清理 switch journal 失败: {error:#}");
        }
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn hex_decode(input: &str) -> Result<Vec<u8>> {
    if !input.len().is_multiple_of(2) {
        anyhow::bail!("odd hex length");
    }
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks(2) {
        let hi = (pair[0] as char)
            .to_digit(16)
            .ok_or_else(|| anyhow!("invalid hex digit"))?;
        let lo = (pair[1] as char)
            .to_digit(16)
            .ok_or_else(|| anyhow!("invalid hex digit"))?;
        out.push(((hi << 4) | lo) as u8);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::{apply_profile_transaction, ConfigAdapter};
    use crate::models::ApiProfile;
    use std::fs;

    struct FakeAdapter {
        config: PathBuf,
        credentials: PathBuf,
    }

    impl ConfigAdapter for FakeAdapter {
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
            vec![self.config.clone(), self.credentials.clone()]
        }
    }

    fn setup(db_path: &Path) -> Result<(Database, FakeAdapter, ApiProfile)> {
        let db = Database::open(db_path)?;
        let dir = db_path.parent().unwrap();
        let adapter = FakeAdapter {
            config: dir.join("config.json"),
            credentials: dir.join("auth.json"),
        };
        fs::write(&adapter.config, b"old config")?;
        fs::write(&adapter.credentials, b"old secret")?;
        let profile = ApiProfile {
            id: Some(7),
            name: "work".to_string(),
            ..ApiProfile::default()
        };
        Ok((db, adapter, profile))
    }

    #[test]
    fn incomplete_switch_is_rolled_back() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let db_path = dir.path().join("db.sqlite");
        let (db, adapter, profile) = setup(&db_path)?;

        // 切换前 DB 里已有旧 shared_config。
        db.save_shared_config(TargetApp::ClaudeCode, serde_json::json!({"old": true}))?;

        let journal = begin_switch(&db, &adapter, TargetApp::ClaudeCode, 7, "work")?
            .expect("file-backed db must journal");
        assert!(journal_path(&db_path).exists());

        // 模拟切换已写了一半：文件已改、shared_config 已改、active 未设置，然后"崩溃"。
        apply_profile_transaction(&adapter, &profile, &serde_json::json!({"new": true}))?;
        db.save_shared_config(TargetApp::ClaudeCode, serde_json::json!({"new": true}))?;
        drop(journal);

        recover_interrupted_switch(&db)?;
        assert!(
            !journal_path(&db_path).exists(),
            "recovery must remove journal"
        );
        assert_eq!(fs::read(&adapter.config)?, b"old config");
        assert_eq!(fs::read(&adapter.credentials)?, b"old secret");
        assert_eq!(
            db.get_shared_config(TargetApp::ClaudeCode)?
                .map(|sc| sc.config),
            Some(serde_json::json!({"old": true}))
        );
        Ok(())
    }

    #[test]
    fn completed_switch_only_cleans_journal() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let db_path = dir.path().join("db.sqlite");
        let (db, adapter, profile) = setup(&db_path)?;
        let profile_id = db.add_profile(&profile)?;

        let journal = begin_switch(&db, &adapter, TargetApp::ClaudeCode, profile_id, "work")?;
        // 模拟切换完整走完（含 active_profile），但 journal 没删。
        apply_profile_transaction(&adapter, &profile, &serde_json::json!({"new": true}))?;
        db.set_active_profile(TargetApp::ClaudeCode, profile_id)?;
        drop(journal);

        recover_interrupted_switch(&db)?;
        assert!(!journal_path(&db_path).exists(), "journal must be cleaned");
        // 不变量：已完成则不动任何东西。
        assert_eq!(fs::read(&adapter.config)?, br#"{"new":true}"#);
        assert_eq!(
            db.get_active_profile(TargetApp::ClaudeCode)?
                .map(|a| a.profile_id),
            Some(profile_id)
        );
        Ok(())
    }

    /// 重复切换同一 profile：若崩溃在写盘之后、set_active 之前，
    /// 必须回滚到 journal 前镜像，而不是因 active 仍指向目标而误判「已完成」。
    #[test]
    fn same_profile_reswitch_incomplete_is_rolled_back() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let db_path = dir.path().join("db.sqlite");
        let (db, adapter, profile) = setup(&db_path)?;
        let profile_id = db.add_profile(&profile)?;

        // 已经是该 profile 的 active。
        db.set_active_profile(TargetApp::ClaudeCode, profile_id)?;
        db.save_shared_config(TargetApp::ClaudeCode, serde_json::json!({"old": true}))?;
        fs::write(&adapter.config, b"old config")?;
        fs::write(&adapter.credentials, b"old secret")?;

        let journal = begin_switch(&db, &adapter, TargetApp::ClaudeCode, profile_id, "work")?
            .expect("file-backed db must journal");
        // 与 apply_profile_switch 一致：重复切换时先清 active，再写盘。
        db.clear_active_profile(TargetApp::ClaudeCode)?;
        apply_profile_transaction(&adapter, &profile, &serde_json::json!({"new": true}))?;
        db.save_shared_config(TargetApp::ClaudeCode, serde_json::json!({"new": true}))?;
        // 崩溃：未 set_active_profile，未 commit journal。
        drop(journal);

        recover_interrupted_switch(&db)?;
        assert!(
            !journal_path(&db_path).exists(),
            "recovery must remove journal"
        );
        assert_eq!(fs::read(&adapter.config)?, b"old config");
        assert_eq!(fs::read(&adapter.credentials)?, b"old secret");
        assert_eq!(
            db.get_shared_config(TargetApp::ClaudeCode)?
                .map(|sc| sc.config),
            Some(serde_json::json!({"old": true}))
        );
        // 回滚应恢复切换前的 active（本例即同一 profile）。
        assert_eq!(
            db.get_active_profile(TargetApp::ClaudeCode)?
                .map(|a| a.profile_id),
            Some(profile_id)
        );
        Ok(())
    }

    /// 软失败路径：apply 中途出错后立即 recover，不应把半状态留到下次启动。
    #[test]
    fn recover_restores_previous_active_from_other_profile() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let db_path = dir.path().join("db.sqlite");
        let (db, adapter, profile) = setup(&db_path)?;
        let old_id = db.add_profile(&ApiProfile {
            name: "old".into(),
            ..ApiProfile::default()
        })?;
        let new_id = db.add_profile(&profile)?;
        db.set_active_profile(TargetApp::ClaudeCode, old_id)?;
        db.save_shared_config(TargetApp::ClaudeCode, serde_json::json!({"old": true}))?;

        let journal = begin_switch(&db, &adapter, TargetApp::ClaudeCode, new_id, "work")?
            .expect("file-backed db must journal");
        // 模拟半完成：文件与 shared 已改，active 仍是 old（A→B 不会 clear）。
        apply_profile_transaction(&adapter, &profile, &serde_json::json!({"new": true}))?;
        db.save_shared_config(TargetApp::ClaudeCode, serde_json::json!({"new": true}))?;
        drop(journal);

        recover_interrupted_switch(&db)?;
        assert_eq!(fs::read(&adapter.config)?, b"old config");
        assert_eq!(
            db.get_active_profile(TargetApp::ClaudeCode)?
                .map(|a| a.profile_id),
            Some(old_id),
            "回滚应保留切换前的 active profile"
        );
        Ok(())
    }

    #[test]
    fn commit_removes_journal() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let db_path = dir.path().join("db.sqlite");
        let (db, adapter, _profile) = setup(&db_path)?;

        let journal = begin_switch(&db, &adapter, TargetApp::ClaudeCode, 7, "work")?;
        let path = journal_path(&db_path);
        assert!(path.exists());
        journal.expect("file-backed db must journal").commit()?;
        assert!(!path.exists());
        Ok(())
    }

    #[test]
    fn memory_db_skips_journal() -> Result<()> {
        let db = Database::open(":memory:")?;
        let adapter = FakeAdapter {
            config: PathBuf::from("/nonexistent/config.json"),
            credentials: PathBuf::from("/nonexistent/auth.json"),
        };
        let journal = begin_switch(&db, &adapter, TargetApp::ClaudeCode, 1, "x")?;
        assert!(journal.is_none());
        Ok(())
    }

    #[test]
    fn missing_file_is_restored_as_missing() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let db_path = dir.path().join("db.sqlite");
        let (db, adapter, _profile) = setup(&db_path)?;

        // 切换前 credentials 不存在。
        fs::remove_file(&adapter.credentials)?;

        let journal = begin_switch(&db, &adapter, TargetApp::ClaudeCode, 7, "work")?;
        // 切换"写"出了 credentials。
        fs::write(&adapter.credentials, b"created by switch")?;
        drop(journal);

        recover_interrupted_switch(&db)?;
        assert!(
            !adapter.credentials.exists(),
            "rollback must recreate absence"
        );
        Ok(())
    }
}
