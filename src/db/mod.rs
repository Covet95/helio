use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

mod profiles;
mod schema;
mod snapshot;
mod state;

use crate::utils::secure_fs::{copy_private, ensure_private_dir, ensure_private_file};

/// 自动数据库备份的保留个数（`db.backup.*` 与 `*.premigrate.*` 各自独立计数）。
const DB_BACKUP_KEEP: usize = 10;

/// 数据库文件所在目录。`Path::parent()` 对裸文件名（如 `--db-path live.sqlite`）
/// 返回 `Some("")`，直接拿去建目录/改权限会失败，这里归一成 `.`。
fn parent_dir(path: &Path) -> PathBuf {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// 重建 `api_profiles` 整表前**必须**先做的备份。
///
/// 这张表装着全部明文 API key。`DROP TABLE` + `RENAME` 的重建流程虽有事务
/// 保护，但「DROP 成功、COMMIT 前崩溃」会留下无兜底的半状态——所以备份不是
/// 可选项：**备份失败必须中止迁移**，宁可迁移不做，也不能让旧数据失去退路。
///
/// `:memory:` 库没有路径，跳过（本就无持久化数据可丢）。
///
/// 备份文件名形如 `db.sqlite.premigrate.<时间戳>.sqlite`，含明文 key，
/// 因此与常规备份一样按 [`DB_BACKUP_KEEP`] 轮转，避免无限累积。
fn backup_before_table_rebuild(conn: &rusqlite::Connection) -> Result<()> {
    let Some(db_path) = conn.path() else {
        return Ok(());
    };
    if db_path == ":memory:" || db_path.is_empty() {
        return Ok(());
    }

    let ts = chrono::Local::now().format("%Y%m%d_%H%M%S_%f");
    let backup = format!("{db_path}.premigrate.{ts}.sqlite");
    copy_private(Path::new(db_path), Path::new(&backup))
        .with_context(|| format!("Failed to back up database before migration: {backup}"))?;

    let path = Path::new(db_path);
    if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
        crate::adapters::backup::cleanup_prefix(
            &parent_dir(path),
            &format!("{file_name}.premigrate."),
            DB_BACKUP_KEEP,
        )
        .with_context(|| "Failed to rotate pre-migration backups")?;
    }

    Ok(())
}

/// 「导入替换失败，且补偿回滚也失败」的统一构造。
///
/// 必须返回 `RollbackFailed` 这个**标记类型**，而不是 `anyhow::anyhow!` 拼出的
/// 纯文本：命令层是按**类型**判定「部分生效」的（见
/// `crate::error::classify_anyhow_chain`）。拼成字符串后链上只剩一段文案，
/// 分类只能退回 `Internal`，前端就不会提示用户去核对工具的实际状态——
/// 而这里恰恰是「磁盘上可能留着半套配置」的情况，最需要那句提示。
///
/// 抽成具名函数是为了可测：要真实触发这条分支，得构造「替换失败 + 回滚也失败」
/// 的文件系统状态（例如把 live 库设成不可删除），测试里无法稳定复现，
/// 所以把这段纯逻辑单独拿出来验证。
fn rollback_failed(error: anyhow::Error, restore_error: anyhow::Error) -> anyhow::Error {
    anyhow::Error::new(crate::error::RollbackFailed::new(format!(
        "{error}；回滚失败：{restore_error}"
    )))
}

pub struct Database {
    conn: Connection,
}

/// 迁移期间的 FK 开关 guard：`PRAGMA foreign_keys` 在事务内是 no-op，
/// 按 SQLite 官方重建表流程必须在 BEGIN 前 OFF、COMMIT 后 ON。
/// Drop 时先回滚残留事务再恢复 FK——避免迁移中途失败后连接残留
/// 「未提交事务 + FK 关闭」的僵尸状态（后续写入进入僵尸事务、静默丢失）。
struct ForeignKeysGuard<'a> {
    conn: &'a Connection,
}

impl<'a> ForeignKeysGuard<'a> {
    fn off(conn: &'a Connection) -> Result<Self> {
        conn.execute_batch("PRAGMA foreign_keys=OFF;")?;
        Ok(Self { conn })
    }
}

impl Drop for ForeignKeysGuard<'_> {
    fn drop(&mut self) {
        // 迁移事务中途失败时残留的未提交事务必须先回滚，
        // 否则事务内的 PRAGMA foreign_keys=ON 是 no-op（恢复失败）。
        // 无活动事务时 ROLLBACK 返回 Err，忽略。
        let _ = self.conn.execute_batch("ROLLBACK;");
        let _ = self.conn.execute_batch("PRAGMA foreign_keys=ON;");
    }
}

impl Database {
    /// 打开或创建数据库
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        if path != Path::new(":memory:") {
            ensure_private_dir(&parent_dir(path))?;
        }
        let conn = Connection::open(path)?;
        if path != Path::new(":memory:") {
            ensure_private_file(path)?;
            // 多个应用进程可能同时打开同一库：WAL + busy_timeout 避免 SQLITE_BUSY。
            // busy_timeout 让并发写等待而非直接报错。
            conn.execute_batch("PRAGMA journal_mode=WAL;")?;
            conn.busy_timeout(std::time::Duration::from_secs(5))?;
            // SQLite 默认 FK 关闭；迁移流程依赖临时 OFF/ON，日常连接统一开启，
            // 使 active_profiles 的 ON DELETE CASCADE 真正生效。
            conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        }
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    /// 数据库文件路径（`:memory:` 为 None）。用于定位事务 journal 等旁车文件。
    pub fn db_path(&self) -> Option<PathBuf> {
        self.conn.path().map(PathBuf::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        ApiProfile, ClaudeProfileFields, CodexProfileFields, OpenClawProfileFields,
        OpenCodeManagedModelState, OpenCodeProfileFields, TargetApp,
    };
    use rusqlite::params;
    use std::collections::HashMap;
    use std::fs;

    #[test]
    fn a_failed_rollback_carries_the_marker_type_not_just_text() {
        // 关键：命令层按**类型**判定 PartialFailure。这里若退回 anyhow::anyhow!
        // 拼字符串，AppError::from 只能给出 Internal——用户就看不到
        // 「请核对工具当前配置」那句提示，而磁盘上其实留着半套配置。
        let err = rollback_failed(
            anyhow::anyhow!("替换数据库失败"),
            anyhow::anyhow!("磁盘只读"),
        );

        assert!(
            err.downcast_ref::<crate::error::RollbackFailed>().is_some(),
            "必须是 RollbackFailed 标记类型，而不是一段纯文本"
        );
        assert!(
            err.to_string().contains("替换数据库失败") && err.to_string().contains("磁盘只读"),
            "两个错误都要留在文案里，实际为：{err}"
        );

        let app = crate::error::AppError::from(err);
        assert_eq!(
            app.kind(),
            crate::error::ErrorKind::PartialFailure,
            "跨到命令层必须变成 PartialFailure"
        );
    }

    #[test]
    fn test_open_rejects_garbage_file_accepts_valid_db() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static CTR: AtomicU64 = AtomicU64::new(0);
        let n = CTR.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("helio-db-open-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        // 垃圾文件（非 SQLite）：Database::open 应失败（init_schema 执行 SQL 时读到非法文件头）
        let garbage = dir.join("garbage.db");
        std::fs::write(&garbage, b"this is not a sqlite database, just text\n").unwrap();
        assert!(
            Database::open(&garbage).is_err(),
            "导入前验证：垃圾文件必须被 Database::open 拒绝"
        );

        // 合法库：先建一个真实库，再 open 应成功
        let valid = dir.join("valid.db");
        Database::open(&valid).unwrap(); // 建库 + init_schema
        assert!(
            Database::open(&valid).is_ok(),
            "合法 Helio 库应能被 open 验证通过"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_invalid_import_leaves_live_database_unchanged() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let live_path = dir.path().join("live.sqlite");
        let db = Database::open(&live_path)?;
        db.add_profile(&ApiProfile {
            name: "live".into(),
            provider: "openai".into(),
            api_url: "https://live.example".into(),
            api_key: "live-key".into(),
            target_app: Some(TargetApp::Codex),
            ..Default::default()
        })?;
        drop(db);

        let invalid_import = dir.path().join("invalid.sqlite");
        fs::write(&invalid_import, b"not a sqlite database")?;

        assert!(Database::replace_file_from_import(&invalid_import, &live_path).is_err());

        let live = Database::open(&live_path)?;
        assert_eq!(
            live.get_profile_by_name_and_target("live", TargetApp::Codex)?
                .api_key,
            "live-key"
        );
        Ok(())
    }

    #[test]
    fn test_valid_import_replaces_live_database_and_keeps_backup() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let live_path = dir.path().join("live.sqlite");
        let import_path = dir.path().join("import.sqlite");

        let live = Database::open(&live_path)?;
        live.add_profile(&ApiProfile {
            name: "old".into(),
            provider: "openai".into(),
            api_url: "https://old.example".into(),
            api_key: "old-key".into(),
            target_app: Some(TargetApp::Codex),
            ..Default::default()
        })?;
        drop(live);

        let imported = Database::open(&import_path)?;
        imported.add_profile(&ApiProfile {
            name: "new".into(),
            provider: "openai".into(),
            api_url: "https://new.example".into(),
            api_key: "new-key".into(),
            target_app: Some(TargetApp::Codex),
            ..Default::default()
        })?;
        drop(imported);

        let backup_path = Database::replace_file_from_import(&import_path, &live_path)?
            .expect("replacing an existing database should create a backup");

        let replaced = Database::open(&live_path)?;
        assert!(replaced
            .get_profile_by_name_and_target("old", TargetApp::Codex)
            .is_err());
        assert_eq!(
            replaced
                .get_profile_by_name_and_target("new", TargetApp::Codex)?
                .api_key,
            "new-key"
        );
        drop(replaced);

        let backup = Database::open(&backup_path)?;
        assert_eq!(
            backup
                .get_profile_by_name_and_target("old", TargetApp::Codex)?
                .api_key,
            "old-key"
        );
        Ok(())
    }

    /// 建一个带 profile 的库，并让最后一条写入停留在未 checkpoint 的 WAL 中。
    /// 返回持有读快照的连接——必须由调用方保活，否则 WAL 会被 checkpoint 掉。
    fn live_db_with_uncheckpointed_wal(path: &Path, wal_only_name: &str) -> Result<Connection> {
        let db = Database::open(path)?;
        db.add_profile(&ApiProfile {
            name: "checkpointed".into(),
            provider: "openai".into(),
            api_url: "https://checkpointed.example".into(),
            api_key: "checkpointed-key".into(),
            target_app: Some(TargetApp::Codex),
            ..Default::default()
        })?;
        drop(db);

        let writer = Database::open(path)?;
        // 读者持有快照 → checkpoint 无法推进，写入滞留在 -wal。
        let reader = Connection::open(path)?;
        reader.execute_batch("BEGIN;")?;
        reader.query_row("SELECT COUNT(*) FROM api_profiles", [], |r| {
            r.get::<_, i64>(0)
        })?;

        writer.add_profile(&ApiProfile {
            name: wal_only_name.into(),
            provider: "openai".into(),
            api_url: "https://wal.example".into(),
            api_key: "wal-key".into(),
            target_app: Some(TargetApp::Codex),
            ..Default::default()
        })?;
        drop(writer);

        let mut wal = path.as_os_str().to_os_string();
        wal.push("-wal");
        assert!(
            fs::metadata(PathBuf::from(wal))?.len() > 0,
            "fixture 前提失效：-wal 应非空"
        );
        Ok(reader)
    }

    #[test]
    fn test_snapshot_includes_uncheckpointed_wal_data() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let live_path = dir.path().join("live.sqlite");
        let reader = live_db_with_uncheckpointed_wal(&live_path, "wal-only")?;

        let snapshot_path = dir.path().join("export.sqlite");
        Database::snapshot_to(&live_path, &snapshot_path)?;
        drop(reader);

        // 旧实现只拷主文件，wal-only 会丢失。
        let exported = Database::open(&snapshot_path)?;
        assert_eq!(
            exported
                .get_profile_by_name_and_target("wal-only", TargetApp::Codex)?
                .api_key,
            "wal-key",
            "导出快照必须包含尚在 WAL 中的已提交数据"
        );
        assert!(exported
            .get_profile_by_name_and_target("checkpointed", TargetApp::Codex)
            .is_ok());
        Ok(())
    }

    #[test]
    fn test_snapshot_overwrites_existing_destination() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let live_path = dir.path().join("live.sqlite");
        let db = Database::open(&live_path)?;
        db.add_profile(&ApiProfile {
            name: "p".into(),
            provider: "openai".into(),
            api_url: "https://p.example".into(),
            api_key: "p-key".into(),
            target_app: Some(TargetApp::Codex),
            ..Default::default()
        })?;
        drop(db);

        // VACUUM INTO 本身要求目标不存在，snapshot_to 需自行处理覆盖。
        let dest = dir.path().join("export.sqlite");
        fs::write(&dest, b"stale content")?;
        Database::snapshot_to(&live_path, &dest)?;

        let exported = Database::open(&dest)?;
        assert!(exported
            .get_profile_by_name_and_target("p", TargetApp::Codex)
            .is_ok());
        Ok(())
    }

    /// 覆盖导出失败时不得删除旧备份：source 不存在应在动 dest 之前失败。
    #[test]
    fn test_snapshot_failure_preserves_existing_destination() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let dest = dir.path().join("export.sqlite");
        fs::write(&dest, b"keep-me")?;
        let missing = dir.path().join("nope.sqlite");
        assert!(Database::snapshot_to(&missing, &dest).is_err());
        assert_eq!(fs::read(&dest)?, b"keep-me", "导出失败不得抹掉既有备份文件");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn test_snapshot_is_owner_only() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir()?;
        let live_path = dir.path().join("live.sqlite");
        drop(Database::open(&live_path)?);

        // 目标目录模拟用户目录（0755），导出不应收紧它。
        let user_dir = dir.path().join("Desktop");
        fs::create_dir(&user_dir)?;
        fs::set_permissions(&user_dir, fs::Permissions::from_mode(0o755))?;

        let dest = user_dir.join("helio-backup.db");
        Database::snapshot_to(&live_path, &dest)?;

        // VACUUM INTO 产出 0644，必须被收紧。
        assert_eq!(fs::metadata(&dest)?.permissions().mode() & 0o777, 0o600);
        assert_eq!(
            fs::metadata(&user_dir)?.permissions().mode() & 0o777,
            0o755,
            "导出不应改动用户目标目录权限"
        );
        Ok(())
    }

    #[test]
    fn test_snapshot_rejects_missing_source() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.sqlite");
        assert!(Database::snapshot_to(&missing, &dir.path().join("out.db")).is_err());
    }

    #[test]
    fn test_validate_rejects_unrelated_sqlite_database() -> Result<()> {
        let dir = tempfile::tempdir()?;
        // 合法 SQLite 但不是 Helio 库（实测：浏览器书签库曾被当备份导入并清空全部档案）。
        let foreign = dir.path().join("bookmarks.db");
        let conn = Connection::open(&foreign)?;
        conn.execute_batch("CREATE TABLE bookmarks(url TEXT); INSERT INTO bookmarks VALUES('x');")?;
        drop(conn);

        let error = Database::validate_import_candidate(&foreign).unwrap_err();
        assert!(
            error.to_string().contains("api_profiles"),
            "错误应说明缺少 api_profiles，实际: {error}"
        );
        Ok(())
    }

    #[test]
    fn test_validate_rejects_empty_and_garbage_files() -> Result<()> {
        let dir = tempfile::tempdir()?;
        // 0 字节文件会被 SQLite 当作合法空库。
        let empty = dir.path().join("empty.db");
        fs::write(&empty, b"")?;
        assert!(Database::validate_import_candidate(&empty).is_err());

        let garbage = dir.path().join("garbage.db");
        fs::write(&garbage, b"not a sqlite database at all")?;
        assert!(Database::validate_import_candidate(&garbage).is_err());

        let missing = dir.path().join("nope.db");
        assert!(Database::validate_import_candidate(&missing).is_err());
        Ok(())
    }

    #[test]
    fn test_validate_accepts_helio_database_without_modifying_it() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("helio.sqlite");
        let db = Database::open(&path)?;
        db.add_profile(&ApiProfile {
            name: "p".into(),
            provider: "openai".into(),
            api_url: "https://p.example".into(),
            api_key: "p-key".into(),
            target_app: Some(TargetApp::Codex),
            ..Default::default()
        })?;
        drop(db);

        let before = fs::read(&path)?;
        Database::validate_import_candidate(&path)?;
        assert_eq!(
            fs::read(&path)?,
            before,
            "校验必须只读，不得写入或迁移候选文件"
        );
        Ok(())
    }

    #[test]
    fn test_validate_accepts_older_helio_schema() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let legacy = dir.path().join("legacy.sqlite");
        // 模拟旧版本导出的库：只有 api_profiles 的关键列，没有后来新增的列，
        // 也没有 schema_migrations。用户的历史备份必须仍能导入。
        write_legacy_helio_db(&legacy)?;
        Database::validate_import_candidate(&legacy)?;
        Ok(())
    }

    /// 旧版本 Helio 库：`name` 全局 UNIQUE、缺少后续新增的列，但保留初版就有的
    /// `model_mapping`（`migrate_composite_unique` 重建表时会 SELECT 它）。
    fn write_legacy_helio_db(path: &Path) -> Result<()> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            r#"
            CREATE TABLE api_profiles (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                provider TEXT NOT NULL,
                api_url TEXT NOT NULL,
                api_key TEXT NOT NULL,
                model_mapping TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            INSERT INTO api_profiles (name,provider,api_url,api_key,created_at,updated_at)
            VALUES ('legacy','openai','https://legacy.example','legacy-key',1,1);
            "#,
        )?;
        Ok(())
    }

    #[test]
    fn test_import_of_older_schema_migrates_live_database() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let live_path = dir.path().join("live.sqlite");
        drop(Database::open(&live_path)?);

        let legacy = dir.path().join("legacy.sqlite");
        write_legacy_helio_db(&legacy)?;

        Database::replace_file_from_import(&legacy, &live_path)?;

        // 替换后的库应已是当前 schema，且旧数据仍在。
        let migrated = Database::open(&live_path)?;
        let profile = migrated
            .list_profiles()?
            .into_iter()
            .find(|p| p.name == "legacy")
            .expect("旧库中的档案应保留");
        assert_eq!(profile.api_key, "legacy-key");
        Ok(())
    }

    #[test]
    fn test_import_aborts_when_candidate_cannot_be_migrated() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let live_path = dir.path().join("live.sqlite");
        let live = Database::open(&live_path)?;
        live.add_profile(&ApiProfile {
            name: "live".into(),
            provider: "openai".into(),
            api_url: "https://live.example".into(),
            api_key: "live-key".into(),
            target_app: Some(TargetApp::Codex),
            ..Default::default()
        })?;
        drop(live);

        // 有 api_profiles 及关键列，能通过静态校验，但缺 model_mapping → 迁移必然失败。
        let broken = dir.path().join("broken.sqlite");
        let conn = Connection::open(&broken)?;
        conn.execute_batch(
            r#"
            CREATE TABLE api_profiles (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                provider TEXT NOT NULL,
                api_url TEXT NOT NULL,
                api_key TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            "#,
        )?;
        drop(conn);

        assert!(Database::replace_file_from_import(&broken, &live_path).is_err());

        // 迁移在私有 staging 副本上失败 → live 库必须原封不动。
        let live = Database::open(&live_path)?;
        assert_eq!(
            live.get_profile_by_name_and_target("live", TargetApp::Codex)?
                .api_key,
            "live-key"
        );
        // staging 残留必须清理干净。
        let leftovers = fs::read_dir(dir.path())?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(".db.import."))
            .count();
        assert_eq!(leftovers, 0, "失败的导入不应留下 staging 文件");
        Ok(())
    }

    #[test]
    fn test_import_over_stale_wal_returns_imported_data() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let live_path = dir.path().join("live.sqlite");
        let reader = live_db_with_uncheckpointed_wal(&live_path, "live-wal-only")?;

        let import_path = dir.path().join("import.sqlite");
        let imported = Database::open(&import_path)?;
        imported.add_profile(&ApiProfile {
            name: "imported".into(),
            provider: "openai".into(),
            api_url: "https://imported.example".into(),
            api_key: "imported-key".into(),
            target_app: Some(TargetApp::Codex),
            ..Default::default()
        })?;
        drop(imported);

        // Windows 不允许替换仍被打开的库文件（os error 32），所以先放掉读者。
        //
        // 这**不影响本用例的意图**：它验证的是「陈旧的 -wal 文件不会把新库
        // 恢复成旧内容」——只要 -wal 文件还在磁盘上、且非空，目的就达到了。
        // 读者连接只是用来阻止 checkpoint 把 WAL 推进主文件（见 fixture），
        // 在替换前关掉它，-wal 依然留在原地。
        drop(reader);

        let backup_path = Database::replace_file_from_import(&import_path, &live_path)?
            .expect("替换已存在的库应产生备份");

        // 旧实现把陈旧 -wal 留在原地，新库被它"恢复"成替换前的内容。
        let replaced = Database::open(&live_path)?;
        assert_eq!(
            replaced
                .get_profile_by_name_and_target("imported", TargetApp::Codex)?
                .api_key,
            "imported-key",
            "导入后应读到导入的数据"
        );
        assert!(
            replaced
                .get_profile_by_name_and_target("live-wal-only", TargetApp::Codex)
                .is_err(),
            "导入后不应残留被替换库的数据"
        );
        drop(replaced);

        // 备份必须含 live 库 WAL 中的数据（旧实现 rename 主文件会丢）。
        let backup = Database::open(&backup_path)?;
        assert_eq!(
            backup
                .get_profile_by_name_and_target("live-wal-only", TargetApp::Codex)?
                .api_key,
            "wal-key",
            "备份应包含尚在 WAL 中的已提交数据"
        );
        assert!(backup
            .get_profile_by_name_and_target("checkpointed", TargetApp::Codex)
            .is_ok());
        Ok(())
    }

    #[test]
    fn test_restore_replaced_file_clears_sidecars() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let live_path = dir.path().join("live.sqlite");
        let backup_path = dir.path().join("db.backup.sqlite");

        // 备份库含 old 档案。
        let backup = Database::open(&backup_path)?;
        backup.add_profile(&ApiProfile {
            name: "old".into(),
            provider: "openai".into(),
            api_url: "https://old.example".into(),
            api_key: "old-key".into(),
            target_app: Some(TargetApp::Codex),
            ..Default::default()
        })?;
        drop(backup);

        // live 位置留下失败导入的产物 + 陈旧 sidecar。
        let reader = live_db_with_uncheckpointed_wal(&live_path, "failed-import")?;
        drop(reader);

        Database::restore_replaced_file(&live_path, &backup_path)?;

        for suffix in ["-wal", "-shm"] {
            let mut name = live_path.as_os_str().to_os_string();
            name.push(suffix);
            assert!(
                !PathBuf::from(name).exists(),
                "恢复后不应残留 {suffix} 文件"
            );
        }
        let restored = Database::open(&live_path)?;
        assert_eq!(
            restored
                .get_profile_by_name_and_target("old", TargetApp::Codex)?
                .api_key,
            "old-key"
        );
        Ok(())
    }

    #[test]
    fn test_import_backups_are_rotated() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let live_path = dir.path().join("live.sqlite");
        let import_path = dir.path().join("import.sqlite");
        drop(Database::open(&live_path)?);
        drop(Database::open(&import_path)?);

        for _ in 0..12 {
            Database::replace_file_from_import(&import_path, &live_path)?;
        }

        let backups = fs::read_dir(dir.path())?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("db.backup."))
            .count();
        assert_eq!(backups, DB_BACKUP_KEEP, "自动备份应轮转，保留 10 个");
        Ok(())
    }

    /// 回归：**每一个**重建整表的迁移都必须先备份。
    ///
    /// `migrate_drop_model_effort_level` / `migrate_drop_model_thinking_enabled`
    /// 与 `migrate_composite_unique` 一样会 `DROP TABLE api_profiles`（这张表
    /// 装着全部明文 API key），早期只有后者做了备份。同文件另一处迁移的注释
    /// 写着「迁移会重建整表，备份失败必须中止迁移，否则旧数据无兜底」——
    /// 这条理由对三者同等适用。
    ///
    /// 两个迁移是**依次执行**的，所以「总数 ≥1」会被其中一个掩盖。这里逐个
    /// 单独触发，断言各自都产出备份。
    #[test]
    fn every_table_rebuild_migration_creates_a_backup() -> Result<()> {
        for (label, extra_column) in [
            ("model_effort_level", "model_effort_level TEXT"),
            ("model_thinking_enabled", "model_thinking_enabled INTEGER"),
        ] {
            let dir = tempfile::tempdir()?;
            let db_path = dir.path().join("live.sqlite");

            {
                let conn = Connection::open(&db_path)?;
                conn.execute_batch(&format!(
                    r#"
                    CREATE TABLE api_profiles (
                        id INTEGER PRIMARY KEY AUTOINCREMENT,
                        name TEXT NOT NULL, provider TEXT NOT NULL,
                        api_url TEXT NOT NULL, api_key TEXT NOT NULL,
                        model_mapping TEXT, model TEXT, reasoning_effort TEXT,
                        context_1m INTEGER, target_app TEXT, models TEXT,
                        wire_api TEXT, env_key TEXT, requires_openai_auth INTEGER,
                        {extra_column},
                        service_tier TEXT, experimental_bearer_token TEXT,
                        created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
                        UNIQUE(name, target_app)
                    );
                    INSERT INTO api_profiles
                        (name, provider, api_url, api_key, created_at, updated_at)
                    VALUES ('legacy','openai','https://legacy.example','legacy-key',1,1);
                    "#
                ))?;
            }

            // 打开即触发该迁移。
            drop(Database::open(&db_path)?);

            let backups = fs::read_dir(dir.path())?
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with("live.sqlite.premigrate.")
                })
                .count();

            assert!(
                backups > 0,
                "含 {label} 列的库触发重建整表迁移时，必须先产出 premigrate 备份"
            );

            // 数据本身也要还在。
            let db = Database::open(&db_path)?;
            let profile = db
                .list_profiles()?
                .into_iter()
                .find(|p| p.name == "legacy")
                .expect("迁移后旧数据应保留");
            assert_eq!(profile.api_key, "legacy-key", "迁移不应丢数据（{label}）");
        }

        Ok(())
    }

    #[test]
    fn test_premigrate_backups_are_rotated() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let db_path = dir.path().join("live.sqlite");

        // 每轮重新写一个旧 schema 的库，让 migrate_composite_unique 再备份一次。
        for _ in 0..12 {
            if db_path.exists() {
                fs::remove_file(&db_path)?;
            }
            Database::remove_sidecar_files(&db_path)?;
            write_legacy_helio_db(&db_path)?;
            drop(Database::open(&db_path)?);
        }

        let backups = fs::read_dir(dir.path())?
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("live.sqlite.premigrate.")
            })
            .count();
        assert_eq!(backups, DB_BACKUP_KEEP, "迁移前备份应轮转，保留 10 个");
        Ok(())
    }

    #[test]
    fn test_fresh_schema_has_no_effort_level() -> Result<()> {
        let db = Database::open(":memory:")?;
        let mut stmt = db.conn.prepare("PRAGMA table_info(api_profiles)")?;
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get(1))?
            .collect::<Result<Vec<_>, _>>()?;
        assert!(cols.contains(&"experimental_bearer_token".into()));
        assert!(!cols.contains(&"model_effort_level".into()));
        assert!(!cols.contains(&"model_thinking_enabled".into()));
        let migration_count: i64 = db.conn.query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE id = ?1",
            params!["2026-07-19-profile-schema-ledger"],
            |row| row.get(0),
        )?;
        assert_eq!(migration_count, 1);
        Ok(())
    }

    #[test]
    fn test_wire_api_chat_migrated_to_responses() -> Result<()> {
        let db = Database::open(":memory:")?;
        // 模拟迁移前的历史脏数据
        db.conn.execute(
            "INSERT INTO api_profiles (name, provider, api_url, api_key, target_app, wire_api, created_at, updated_at) VALUES ('old', 'p', 'https://x', 'sk', 'codex', 'chat', 1, 1)",
            [],
        )?;
        db.conn.execute(
            "DELETE FROM schema_migrations WHERE id = '2026-09-07-normalize-codex-wire-api'",
            [],
        )?;
        db.migrate_normalize_codex_wire_api()?;
        let wire: String = db.conn.query_row(
            "SELECT wire_api FROM api_profiles WHERE name = 'old'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(wire, "responses");
        // 幂等
        db.migrate_normalize_codex_wire_api()?;
        Ok(())
    }

    #[test]
    fn test_nested_codex_fields_roundtrip() -> Result<()> {
        let db = Database::open(":memory:")?;
        let id = db.add_profile(&ApiProfile {
            name: "w".into(),
            provider: "openai".into(),
            api_url: "https://x".into(),
            api_key: "sk".into(),
            target_app: Some(TargetApp::Codex),
            codex: CodexProfileFields {
                reasoning_effort: Some("xhigh".into()),
                reasoning_summary: Some("concise".into()),
                verbosity: Some("medium".into()),
                wire_api: Some("responses".into()),
                env_key: Some("MY_CODEX_KEY".into()),
                experimental_bearer_token: Some("sk-b".into()),
                supports_standalone_web_search: Some(true),
                aws_profile: Some("production".into()),
                aws_region: Some("us-east-1".into()),
                auth_command: Some("gcloud".into()),
                auth_args: Some(vec!["auth".into(), "print-access-token".into()]),
                auth_timeout_ms: Some(5000),
                auth_refresh_interval_ms: Some(300000),
                auth_cwd: Some("/tmp".into()),
                catalog_models: Some(vec![crate::models::CodexCatalogModel {
                    slug: "gpt-5.6-sol".into(),
                    display_name: Some("GPT-5.6 Sol".into()),
                    context_window: Some(400_000),
                    reasoning_levels: Some(vec!["minimal".into(), "xhigh".into()]),
                    supports_images: Some(true),
                    ..Default::default()
                }]),
                ..Default::default()
            },
            ..Default::default()
        })?;
        let got = db.get_profile_by_id(id)?.unwrap();
        assert_eq!(got.codex.reasoning_effort.as_deref(), Some("xhigh"));
        assert_eq!(got.codex.reasoning_summary.as_deref(), Some("concise"));
        assert_eq!(got.codex.verbosity.as_deref(), Some("medium"));
        assert_eq!(got.codex.env_key.as_deref(), Some("MY_CODEX_KEY"));
        assert_eq!(got.codex.experimental_bearer_token.as_deref(), Some("sk-b"));
        assert_eq!(got.codex.supports_standalone_web_search, Some(true));
        assert_eq!(got.codex.aws_profile.as_deref(), Some("production"));
        assert_eq!(got.codex.aws_region.as_deref(), Some("us-east-1"));
        assert_eq!(got.codex.auth_command.as_deref(), Some("gcloud"));
        assert_eq!(
            got.codex.auth_args,
            Some(vec!["auth".into(), "print-access-token".into()])
        );
        assert_eq!(got.codex.auth_timeout_ms, Some(5000));
        assert_eq!(got.codex.auth_refresh_interval_ms, Some(300000));
        assert_eq!(got.codex.auth_cwd.as_deref(), Some("/tmp"));
        let cm = got.codex.catalog_models.as_ref().unwrap();
        assert_eq!(cm.len(), 1);
        assert_eq!(cm[0].slug, "gpt-5.6-sol");
        assert_eq!(cm[0].display_name.as_deref(), Some("GPT-5.6 Sol"));
        assert_eq!(cm[0].context_window, Some(400_000));
        assert_eq!(
            cm[0].reasoning_levels,
            Some(vec!["minimal".into(), "xhigh".into()])
        );
        assert_eq!(cm[0].supports_images, Some(true));
        Ok(())
    }

    #[test]
    fn test_opencode_fields_and_model_state_roundtrip() -> Result<()> {
        let db = Database::open(":memory:")?;
        let id = db.add_profile(&ApiProfile {
            name: "opencode".into(),
            provider: "cpa".into(),
            api_url: "https://example.test/v1".into(),
            api_key: "key".into(),
            model: Some("gpt-5".into()),
            target_app: Some(TargetApp::OpenCode),
            opencode: OpenCodeProfileFields {
                models: Some(vec!["gpt-5".into(), "gpt-5-mini".into()]),
                opencode_api_mode: Some("responses".into()),
                model_configs: Some(std::collections::HashMap::from([(
                    "gpt-5".into(),
                    serde_json::json!({
                        "options": {
                            "reasoningEffort": "high"
                        },
                        "variants": {
                            "max": {
                                "reasoningEffort": "xhigh"
                            }
                        }
                    }),
                )])),
            },
            ..Default::default()
        })?;
        let got = db.get_profile_by_id(id)?.unwrap();
        assert_eq!(got.opencode.opencode_api_mode.as_deref(), Some("responses"));
        assert_eq!(
            got.opencode.model_configs.as_ref().unwrap()["gpt-5"]["variants"]["max"]
                ["reasoningEffort"],
            "xhigh"
        );

        let state = OpenCodeManagedModelState::from([(
            "cpa".into(),
            vec!["gpt-5".into(), "gpt-5-mini".into()],
        )]);
        db.replace_opencode_managed_models(&state)?;
        assert_eq!(db.get_opencode_managed_models()?, state);
        db.clear_opencode_managed_provider("cpa")?;
        assert!(db.get_opencode_managed_models()?.is_empty());
        Ok(())
    }

    #[test]
    fn test_provider_ownership_is_conservative_and_clearable() -> Result<()> {
        let db = Database::open(":memory:")?;
        db.record_provider_ownership_if_missing(TargetApp::ZCode, " DeepSeek ", true)?;
        assert_eq!(
            db.provider_managed_by_helio(TargetApp::ZCode, "deepseek")?,
            Some(true)
        );

        // Existing ownership is never overwritten by a later observation.
        db.record_provider_ownership_if_missing(TargetApp::ZCode, "deepseek", false)?;
        assert_eq!(
            db.provider_managed_by_helio(TargetApp::ZCode, "deepseek")?,
            Some(true)
        );

        db.clear_provider_ownership(TargetApp::ZCode, "DEEPSEEK")?;
        assert_eq!(
            db.provider_managed_by_helio(TargetApp::ZCode, "deepseek")?,
            None
        );
        Ok(())
    }

    #[test]
    fn test_legacy_effort_level_column_is_dropped() -> Result<()> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static C: AtomicU64 = AtomicU64::new(0);
        let n = C.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("helio-drop-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("l.sqlite");
        {
            let c = rusqlite::Connection::open(&path)?;
            c.execute_batch(r#"
                CREATE TABLE api_profiles (
                    id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL, provider TEXT NOT NULL,
                    api_url TEXT NOT NULL, api_key TEXT NOT NULL, model_mapping TEXT, model TEXT,
                    reasoning_effort TEXT, context_1m INTEGER, target_app TEXT, models TEXT,
                    wire_api TEXT, requires_openai_auth INTEGER, model_effort_level TEXT,
                    model_thinking_enabled INTEGER, service_tier TEXT, experimental_bearer_token TEXT,
                    created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, UNIQUE(name, target_app)
                );
                INSERT INTO api_profiles (name,provider,api_url,api_key,target_app,model_effort_level,reasoning_effort,created_at,updated_at)
                VALUES ('legacy','openai','u','k','codex','high','xhigh',0,0);
            "#)?;
        }
        let db = Database::open(&path)?;
        let got = db.get_profile_by_name_and_target("legacy", TargetApp::Codex)?;
        assert_eq!(got.codex.reasoning_effort.as_deref(), Some("xhigh"));
        let mut stmt = db.conn.prepare("PRAGMA table_info(api_profiles)")?;
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get(1))?
            .collect::<Result<Vec<_>, _>>()?;
        assert!(!cols.contains(&"model_effort_level".into()));
        assert!(!cols.contains(&"model_thinking_enabled".into()));
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn test_thinking_migration_preserves_active_profile_and_new_codex_fields() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("thinking.sqlite");
        {
            let conn = rusqlite::Connection::open(&path)?;
            conn.execute_batch(
                r#"
                PRAGMA foreign_keys=ON;
                CREATE TABLE api_profiles (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL, provider TEXT NOT NULL, api_url TEXT NOT NULL, api_key TEXT NOT NULL,
                    model_mapping TEXT, model TEXT, reasoning_effort TEXT, context_1m INTEGER,
                    target_app TEXT, models TEXT, wire_api TEXT, env_key TEXT, requires_openai_auth INTEGER,
                    model_thinking_enabled INTEGER, service_tier TEXT, experimental_bearer_token TEXT,
                    supports_standalone_web_search INTEGER, aws_profile TEXT, aws_region TEXT,
                    api_mode TEXT, max_tokens INTEGER, api_keys_json TEXT, catalog_models TEXT,
                    created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, UNIQUE(name, target_app)
                );
                CREATE TABLE active_profiles (
                    target_app TEXT PRIMARY KEY,
                    profile_id INTEGER NOT NULL,
                    FOREIGN KEY (profile_id) REFERENCES api_profiles(id) ON DELETE CASCADE
                );
                INSERT INTO api_profiles (
                    id, name, provider, api_url, api_key, target_app, model_thinking_enabled,
                    supports_standalone_web_search, aws_profile, aws_region, created_at, updated_at
                ) VALUES (
                    7, 'bedrock', 'amazon-bedrock', '', '', 'codex', 1, 1,
                    'production', 'us-east-1', 0, 0
                );
                INSERT INTO active_profiles (target_app, profile_id) VALUES ('codex', 7);
                "#,
            )?;
        }

        let db = Database::open(&path)?;
        let active = db.get_active_profile_full(TargetApp::Codex)?.unwrap();
        assert_eq!(active.id, Some(7));
        assert_eq!(active.codex.supports_standalone_web_search, Some(true));
        assert_eq!(active.codex.aws_profile.as_deref(), Some("production"));
        assert_eq!(active.codex.aws_region.as_deref(), Some("us-east-1"));
        let mut stmt = db.conn.prepare("PRAGMA table_info(api_profiles)")?;
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get(1))?
            .collect::<Result<Vec<_>, _>>()?;
        assert!(!cols.contains(&"model_thinking_enabled".into()));
        Ok(())
    }

    #[test]
    fn test_effort_level_rebuild_preserves_current_codex_columns() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("legacy.sqlite");
        {
            let conn = rusqlite::Connection::open(&path)?;
            conn.execute_batch(
                r#"
                CREATE TABLE api_profiles (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL,
                    provider TEXT NOT NULL,
                    api_url TEXT NOT NULL,
                    api_key TEXT NOT NULL,
                    model_mapping TEXT,
                    model TEXT,
                    reasoning_effort TEXT,
                    context_1m INTEGER,
                    target_app TEXT,
                    models TEXT,
                    wire_api TEXT,
                    env_key TEXT,
                    requires_openai_auth INTEGER,
                    model_effort_level TEXT,
                    model_thinking_enabled INTEGER,
                    service_tier TEXT,
                    experimental_bearer_token TEXT,
                    api_mode TEXT,
                    max_tokens INTEGER,
                    api_keys_json TEXT,
                    catalog_models TEXT,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL,
                    UNIQUE(name, target_app)
                );
                INSERT INTO api_profiles (
                    name, provider, api_url, api_key, target_app, env_key, api_keys_json,
                    catalog_models, model_effort_level, created_at, updated_at
                ) VALUES (
                    'legacy', 'openai', 'https://example.test', 'fallback-key', 'codex',
                    'CODEX_API_KEY',
                    '[{"id":"primary","label":"Primary","key":"live-key","is_active":true}]',
                    '[{"slug":"gpt-test","supports_reasoning":true}]',
                    'high', 0, 0
                );
                "#,
            )?;
        }

        let db = Database::open(&path)?;
        let profile = db.get_profile_by_name_and_target("legacy", TargetApp::Codex)?;
        assert_eq!(profile.codex.env_key.as_deref(), Some("CODEX_API_KEY"));
        assert_eq!(profile.active_key(), "live-key");
        assert_eq!(profile.codex.catalog_models.as_ref().map(Vec::len), Some(1));
        assert_eq!(
            profile.codex.catalog_models.as_ref().unwrap()[0].slug,
            "gpt-test"
        );
        Ok(())
    }

    #[test]
    fn test_database_operations() -> Result<()> {
        let db = Database::open(":memory:")?;

        // 测试添加 Profile
        let profile = ApiProfile {
            name: "test-profile".to_string(),
            provider: "anthropic".to_string(),
            api_url: "https://api.anthropic.com".to_string(),
            api_key: "sk-test-key".to_string(),
            claude: ClaudeProfileFields {
                model_mapping: Some(HashMap::from([(
                    "opus".to_string(),
                    "claude-opus-4".to_string(),
                )])),
            },
            target_app: Some(TargetApp::ClaudeCode),
            ..Default::default()
        };

        let id = db.add_profile(&profile)?;
        assert!(id > 0);

        // 测试获取 Profile
        let retrieved = db.get_profile_by_name_and_target("test-profile", TargetApp::ClaudeCode)?;
        assert_eq!(retrieved.name, "test-profile");
        assert_eq!(retrieved.api_url, "https://api.anthropic.com");
        assert_eq!(retrieved.target_app, Some(TargetApp::ClaudeCode));

        // 测试列出 Profiles
        let profiles = db.list_profiles()?;
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].target_app, Some(TargetApp::ClaudeCode));

        // 测试共享配置
        let config = serde_json::json!({
            "permissions": {"allow": ["bash"]},
            "hooks": {}
        });
        db.save_shared_config(TargetApp::ClaudeCode, config.clone())?;

        let retrieved_config = db.get_shared_config(TargetApp::ClaudeCode)?;
        assert!(retrieved_config.is_some());

        // 测试活动 Profile
        db.set_active_profile(TargetApp::ClaudeCode, id)?;
        let active = db.get_active_profile(TargetApp::ClaudeCode)?;
        assert!(active.is_some());
        assert_eq!(active.unwrap().profile_id, id);

        Ok(())
    }

    /// 回归：无 id 时按 name 定位，**必须同时匹配 target_app**。
    ///
    /// 表的唯一约束是 `UNIQUE(name, target_app)`——不同工具下同名是被允许的。
    /// 早期实现只写 `WHERE name = ?`，会把所有同名档案一起改掉（跨工具误写）。
    #[test]
    fn update_profile_without_id_does_not_cross_tools() -> Result<()> {
        let db = Database::open(":memory:")?;

        // 两个工具下各有一个同名档案，但 key 不同。
        let make = |tool: TargetApp, key: &str| ApiProfile {
            name: "shared-name".to_string(),
            provider: "anthropic".to_string(),
            api_url: "https://api.example.com/v1".to_string(),
            api_key: key.to_string(),
            target_app: Some(tool),
            ..Default::default()
        };
        db.add_profile(&make(TargetApp::ClaudeCode, "sk-claude"))?;
        db.add_profile(&make(TargetApp::Codex, "sk-codex"))?;

        // 无 id 更新 Codex 那条：只有它该变。
        db.update_profile(&ApiProfile {
            id: None,
            api_key: "sk-codex-updated".to_string(),
            ..make(TargetApp::Codex, "sk-codex")
        })?;

        let claude = db.get_profile_by_name_and_target("shared-name", TargetApp::ClaudeCode)?;
        let codex = db.get_profile_by_name_and_target("shared-name", TargetApp::Codex)?;

        assert_eq!(
            claude.api_key, "sk-claude",
            "同名但不同工具的档案不应被改动（跨工具误写）"
        );
        assert_eq!(codex.api_key, "sk-codex-updated", "目标档案应已更新");

        Ok(())
    }

    /// 回归：无 id 且无 target_app 时无法唯一定位，必须报错而不是猜。
    #[test]
    fn update_profile_without_id_or_target_app_is_rejected() -> Result<()> {
        let db = Database::open(":memory:")?;

        let err = db
            .update_profile(&ApiProfile {
                id: None,
                name: "who-knows".to_string(),
                provider: "anthropic".to_string(),
                api_url: "https://api.example.com/v1".to_string(),
                api_key: "sk-x".to_string(),
                target_app: None,
                ..Default::default()
            })
            .expect_err("缺少定位信息应报错");

        assert_eq!(err.kind, crate::error::ErrorKind::InvalidInput);

        Ok(())
    }

    #[test]
    fn test_update_profile_can_rename() -> Result<()> {
        let db = Database::open(":memory:")?;

        let profile = ApiProfile {
            name: "old-name".to_string(),
            provider: "anthropic".to_string(),
            api_url: "https://api.example.com/v1".to_string(),
            api_key: "sk-old".to_string(),
            target_app: Some(TargetApp::ClaudeCode),
            ..Default::default()
        };
        let id = db.add_profile(&profile)?;
        // 标记为某工具的活动 profile（active 表按 id 关联）
        db.set_active_profile(TargetApp::ClaudeCode, id)?;

        // 改名 + 改 key，带上原 id
        let edited = ApiProfile {
            id: Some(id),
            name: "new-name".to_string(),
            provider: "anthropic".to_string(),
            api_url: "https://api.example.com/v1".to_string(),
            api_key: "sk-new".to_string(),
            target_app: Some(TargetApp::ClaudeCode),
            ..Default::default()
        };
        db.update_profile(&edited)?;

        // 旧名查不到，新名查得到，且 id 不变、字段已更新
        assert!(
            db.get_profile_by_name_and_target("old-name", TargetApp::ClaudeCode)
                .is_err(),
            "旧名应已不存在"
        );
        let got = db.get_profile_by_name_and_target("new-name", TargetApp::ClaudeCode)?;
        assert_eq!(got.id, Some(id), "改名不应改变 id");
        assert_eq!(got.api_key, "sk-new");

        // active 关联按 id，改名后仍指向同一条
        let active = db.get_active_profile(TargetApp::ClaudeCode)?;
        assert_eq!(active.unwrap().profile_id, id, "改名后活动关联应保留");

        Ok(())
    }

    #[test]
    fn test_legacy_null_target_profile_can_be_assigned_or_deleted() -> Result<()> {
        let db = Database::open(":memory:")?;
        let now = chrono::Utc::now().timestamp();

        db.conn.execute(
            "INSERT INTO api_profiles
             (name, provider, api_url, api_key, target_app, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?5)",
            rusqlite::params![
                "legacy",
                "custom",
                "https://legacy.example",
                "legacy-key",
                now
            ],
        )?;
        let legacy_id = db.conn.last_insert_rowid();

        assert_eq!(
            db.get_profile_by_id(legacy_id)?.and_then(|p| p.target_app),
            None
        );
        db.assign_legacy_profile(legacy_id, TargetApp::Codex)?;
        assert_eq!(
            db.get_profile_by_id(legacy_id)?.and_then(|p| p.target_app),
            Some(TargetApp::Codex)
        );
        assert!(db
            .assign_legacy_profile(legacy_id, TargetApp::ClaudeCode)
            .is_err());

        db.conn.execute(
            "INSERT INTO api_profiles
             (name, provider, api_url, api_key, target_app, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?5)",
            rusqlite::params![
                "legacy-delete",
                "custom",
                "https://legacy-delete.example",
                "legacy-delete-key",
                now
            ],
        )?;
        let delete_id = db.conn.last_insert_rowid();
        db.set_active_profile(TargetApp::ClaudeCode, delete_id)?;
        assert!(db.delete_legacy_profile(delete_id)?);
        assert!(db.get_profile_by_id(delete_id)?.is_none());
        assert!(db.get_active_profile(TargetApp::ClaudeCode)?.is_none());
        assert!(!db.delete_legacy_profile(delete_id)?);

        Ok(())
    }

    #[test]
    #[cfg(feature = "tauri-gui")]
    fn test_get_active_targets_for_profile_returns_all_matches() -> Result<()> {
        let db = Database::open(":memory:")?;

        let profile = ApiProfile {
            name: "shared".to_string(),
            provider: "anthropic".to_string(),
            api_url: "https://api.example.com/v1".to_string(),
            api_key: "sk-test-key".to_string(),
            target_app: Some(TargetApp::ClaudeCode),
            ..Default::default()
        };

        let id = db.add_profile(&profile)?;
        db.set_active_profile(TargetApp::ClaudeCode, id)?;
        db.set_active_profile(TargetApp::OpenCode, id)?;

        let targets = db.get_active_targets_for_profile(id)?;
        assert_eq!(targets, vec![TargetApp::ClaudeCode, TargetApp::OpenCode]);

        Ok(())
    }

    #[test]
    fn test_composite_lookup_delete_exists() -> Result<()> {
        let db = Database::open(":memory:")?;
        let mk = |name: &str, t: TargetApp| ApiProfile {
            name: name.into(),
            provider: "p".into(),
            api_url: "u".into(),
            api_key: "k".into(),
            target_app: Some(t),
            ..Default::default()
        };
        let id_cc = db.add_profile(&mk("一一", TargetApp::ClaudeCode))?;
        let id_cx = db.add_profile(&mk("一一", TargetApp::Codex))?;
        assert_ne!(id_cc, id_cx);

        // 精确查:各取各
        let a = db.get_profile_by_name_and_target("一一", TargetApp::ClaudeCode)?;
        let b = db.get_profile_by_name_and_target("一一", TargetApp::Codex)?;
        assert_eq!(a.id, Some(id_cc));
        assert_eq!(b.id, Some(id_cx));

        // 查重:同工具命中、排除自身不命中、另一工具命中各自的
        assert!(db.profile_name_exists("一一", TargetApp::ClaudeCode, None)?);
        assert!(!db.profile_name_exists("一一", TargetApp::ClaudeCode, Some(id_cc))?);
        assert!(!db.profile_name_exists("二二", TargetApp::ClaudeCode, None)?);

        // 删 codex 的 一一,不影响 claude 的
        assert!(db.delete_profile("一一", TargetApp::Codex)?);
        assert!(db
            .get_profile_by_name_and_target("一一", TargetApp::Codex)
            .is_err());
        assert!(db
            .get_profile_by_name_and_target("一一", TargetApp::ClaudeCode)
            .is_ok());
        Ok(())
    }

    #[test]
    fn test_max_tokens_roundtrip() -> Result<()> {
        let db = Database::open(":memory:")?;
        let id = db.add_profile(&ApiProfile {
            name: "mt".into(),
            provider: "cpa".into(),
            api_url: "https://x".into(),
            api_key: "sk".into(),
            target_app: Some(TargetApp::OpenClaw),
            context_1m: Some(true),
            openclaw: OpenClawProfileFields {
                api_mode: Some("anthropic_messages".into()),
                max_tokens: Some(65536),
            },
            ..Default::default()
        })?;
        let got = db.get_profile_by_id(id)?.unwrap();
        assert_eq!(got.openclaw.max_tokens, Some(65536));
        assert_eq!(got.context_1m, Some(true));
        assert_eq!(got.openclaw.api_mode.as_deref(), Some("anthropic_messages"));
        // Hermes group must stay empty for OpenClaw profiles
        assert!(got.hermes.api_mode.is_none());
        Ok(())
    }

    #[test]
    fn test_multi_key_roundtrip() {
        use crate::models::ApiKeyEntry;
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path().join("t.sqlite")).unwrap();
        let mut p = ApiProfile {
            name: "mk".into(),
            provider: "openai".into(),
            api_url: "https://x".into(),
            api_key: "sk-a".into(),
            target_app: Some(TargetApp::Codex),
            api_keys: Some(vec![
                ApiKeyEntry {
                    id: "1".into(),
                    label: "a".into(),
                    key: "sk-a".into(),
                    is_active: false,
                    ..Default::default()
                },
                ApiKeyEntry {
                    id: "2".into(),
                    label: "b".into(),
                    key: "sk-b".into(),
                    is_active: true,
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };
        let id = db.add_profile(&p).unwrap();
        let got = db
            .get_profile_by_name_and_target("mk", TargetApp::Codex)
            .unwrap();
        assert_eq!(got.api_key, "sk-b");
        assert_eq!(got.api_keys.as_ref().unwrap().len(), 2);
        assert_eq!(got.active_key(), "sk-b");
        // switch active
        p.id = Some(id);
        assert!(p.set_active_key_id("1"));
        db.update_profile(&p).unwrap();
        let got2 = db
            .get_profile_by_name_and_target("mk", TargetApp::Codex)
            .unwrap();
        assert_eq!(got2.api_key, "sk-a");
    }

    #[test]
    fn test_migrate_to_composite_unique_and_strip_cc() -> Result<()> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static CTR: AtomicU64 = AtomicU64::new(0);
        let n = CTR.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("helio-mig-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("old.sqlite");

        // 1) 手工造一个“旧版”库：name 全局 UNIQUE，含 -cc 数据 + 跨工具同名
        {
            let conn = rusqlite::Connection::open(&path)?;
            conn.execute_batch(
                r#"
                CREATE TABLE api_profiles (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL UNIQUE,
                    provider TEXT NOT NULL,
                    api_url TEXT NOT NULL,
                    api_key TEXT NOT NULL,
                    model_mapping TEXT,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL,
                    model TEXT, reasoning_effort TEXT, context_1m INTEGER, target_app TEXT, models TEXT
                );
                INSERT INTO api_profiles (name,provider,api_url,api_key,created_at,updated_at,target_app) VALUES
                    ('一一','anthropic','u','k',0,0,'claude-code'),
                    ('一一-cc','openai','u','k',0,0,'codex');

                CREATE TABLE active_profiles (
                    target_app TEXT PRIMARY KEY,
                    profile_id INTEGER NOT NULL,
                    FOREIGN KEY (profile_id) REFERENCES api_profiles(id) ON DELETE CASCADE
                );
                INSERT INTO active_profiles (target_app, profile_id) VALUES ('codex', 2);
                "#,
            )?;
        }

        // 2) 用 Database::open 触发迁移
        let db = Database::open(&path)?;

        // 3) 断言：复合唯一约束已生效
        let sql: String = db.conn.query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='api_profiles'",
            [],
            |r| r.get(0),
        )?;
        assert!(
            sql.contains("UNIQUE(name, target_app)") || sql.contains("UNIQUE (name, target_app)"),
            "应为复合唯一，实际: {sql}"
        );
        assert!(
            !sql.contains("name TEXT NOT NULL UNIQUE"),
            "旧的全局 name UNIQUE 应已移除"
        );

        // 4) -cc 已去后缀
        let profiles = db.list_profiles()?;
        let names: Vec<(String, Option<String>)> = profiles
            .iter()
            .map(|p| (p.name.clone(), p.target_app.map(|t| t.as_str().to_string())))
            .collect();
        assert!(names.contains(&("一一".into(), Some("claude-code".into()))));
        assert!(
            names.contains(&("一一".into(), Some("codex".into()))),
            "一一-cc 应去后缀为 一一(codex)"
        );
        assert!(
            !profiles.iter().any(|p| p.name.ends_with("-cc")),
            "不应再有 -cc 后缀"
        );

        // 5) id 必须在重建中保留(active_profiles 按 id 关联)
        let claude_yi = profiles
            .iter()
            .find(|p| p.name == "一一" && p.target_app == Some(TargetApp::ClaudeCode))
            .expect("claude 一一 存在");
        assert_eq!(claude_yi.id, Some(1), "claude 一一 应保留 id=1");
        let codex_yi = profiles
            .iter()
            .find(|p| p.name == "一一" && p.target_app == Some(TargetApp::Codex))
            .expect("codex 一一(原 一一-cc) 存在");
        assert_eq!(codex_yi.id, Some(2), "codex 一一(原 一一-cc) 应保留 id=2");

        // 5b) active_profiles 的外键记录在重建后仍存在且 profile_id 不变
        let active_pid: i64 = db.conn.query_row(
            "SELECT profile_id FROM active_profiles WHERE target_app = 'codex'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(
            active_pid, 2,
            "active_profiles(codex) 应仍指向 profile_id=2(迁移不应回滚/丢失)"
        );

        // 6) 复合唯一：codex 再插一个 一一 应失败；claude 插 一一 也应失败(同工具重名)
        let dup = ApiProfile {
            name: "一一".into(),
            provider: "x".into(),
            api_url: "u".into(),
            api_key: "k".into(),
            target_app: Some(TargetApp::Codex),
            ..Default::default()
        };
        assert!(
            db.add_profile(&dup).is_err(),
            "同工具(codex)重名应被复合唯一拒绝"
        );

        // 7) 幂等:再 open 一次不报错
        drop(db);
        let _db2 = Database::open(&path)?;

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    /// 结构化错误的价值全在 `kind` 上。如果类别不对，前端就还得回去猜文案，
    /// 这次改造就白做了。这里把每种失败的类别逐个钉死。
    #[test]
    fn legacy_profile_operations_report_structured_error_kinds() -> Result<()> {
        use crate::error::ErrorKind;

        let dir = tempfile::tempdir()?;
        let db = Database::open(dir.path().join("live.sqlite"))?;
        let now = chrono::Utc::now().timestamp();

        // 直接插一行 target_app IS NULL 的遗留 profile：
        // add_profile 现在会拒绝 target_app=None，所以不能再借它造数据。
        let insert_legacy = |name: &str| -> Result<i64> {
            db.conn.execute(
                "INSERT INTO api_profiles
                 (name, provider, api_url, api_key, target_app, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?5)",
                rusqlite::params![name, "custom", "https://legacy.example", "legacy-key", now],
            )?;
            Ok(db.conn.last_insert_rowid())
        };

        // 1) id 不存在 -> NotFound（而不是 Internal / 靠文案猜）
        let err = db
            .assign_legacy_profile(999_999, TargetApp::Codex)
            .expect_err("不存在的 id 必须失败");
        assert_eq!(err.kind(), ErrorKind::NotFound);
        assert!(
            err.message.contains("999999"),
            "文案要带上 id，实际为：{}",
            err.message
        );

        // 2) 已归属的 profile 再次指派 -> Conflict
        let legacy_id = insert_legacy("legacy-kind")?;
        db.assign_legacy_profile(legacy_id, TargetApp::Codex)?;
        let err = db
            .assign_legacy_profile(legacy_id, TargetApp::ClaudeCode)
            .expect_err("已归属的 profile 不能再指派");
        assert_eq!(
            err.kind(),
            ErrorKind::Conflict,
            "「已归属」是状态冲突，不是「不存在」"
        );

        // 3) 同工具重名 -> Conflict
        let dup_id = insert_legacy("legacy-kind")?;
        let err = db
            .assign_legacy_profile(dup_id, TargetApp::Codex)
            .expect_err("同工具重名必须失败");
        assert_eq!(err.kind(), ErrorKind::Conflict);
        assert!(
            err.message.contains("legacy-kind"),
            "重名冲突要指出是哪个名字，实际为：{}",
            err.message
        );

        // 4) add_profile 缺 target_app -> InvalidInput
        // 命令层已不再重复校验这一条，所以这里是唯一防线，必须真的返回 InvalidInput。
        let err = db
            .add_profile(&ApiProfile {
                name: "no-target".into(),
                provider: "custom".into(),
                api_url: "https://x.example".into(),
                api_key: "k".into(),
                target_app: None,
                ..Default::default()
            })
            .expect_err("缺 target_app 必须失败");
        assert_eq!(err.kind(), ErrorKind::InvalidInput);

        // 5) delete_legacy_profile 没有语义错误：删不到是 Ok(false)，不是 Err。
        // 注意要用**未指派**的行——已归属的行被 WHERE target_app IS NULL 挡掉，
        // 删不掉是设计如此，不是缺陷。
        let to_delete = insert_legacy("legacy-delete")?;
        assert!(db.delete_legacy_profile(to_delete)?);
        assert!(!db.delete_legacy_profile(to_delete)?);

        Ok(())
    }

    /// `update_profile` 与 `add_profile` 现在返回同一类错误，命令层那份重复的
    /// target_app 校验已删除——所以这里是唯一防线，必须真的返回 InvalidInput。
    #[test]
    fn update_profile_rejects_missing_target_app_as_invalid_input() -> Result<()> {
        use crate::error::ErrorKind;

        let dir = tempfile::tempdir()?;
        let db = Database::open(dir.path().join("live.sqlite"))?;
        let id = db.add_profile(&ApiProfile {
            name: "p".into(),
            provider: "custom".into(),
            api_url: "https://x.example".into(),
            api_key: "k".into(),
            target_app: Some(TargetApp::Codex),
            ..Default::default()
        })?;

        let err = db
            .update_profile(&ApiProfile {
                id: Some(id),
                target_app: None,
                ..Default::default()
            })
            .expect_err("缺 target_app 的更新必须失败");
        assert_eq!(err.kind(), ErrorKind::InvalidInput);

        Ok(())
    }

    #[test]
    fn duplicate_profile_name_is_conflict_not_io() -> anyhow::Result<()> {
        use crate::error::ErrorKind;

        let dir = tempfile::tempdir()?;
        let db = Database::open(dir.path().join("dup.sqlite"))?;
        let base = ApiProfile {
            name: "dup".into(),
            provider: "custom".into(),
            api_url: "https://x.example".into(),
            api_key: "k".into(),
            target_app: Some(TargetApp::Codex),
            ..Default::default()
        };
        db.add_profile(&base)?;

        // 同名新增：必须是 Conflict（带名字），而不是 Io 的“数据库操作失败”。
        let err = db.add_profile(&base).expect_err("同名新增必须失败");
        assert_eq!(err.kind(), ErrorKind::Conflict);
        assert!(
            err.to_string().contains("dup"),
            "冲突文案应保留名字，实得：{}",
            err
        );

        // 改名撞车：同样必须是 Conflict。
        let other = ApiProfile {
            name: "other".into(),
            ..base.clone()
        };
        let other_id = db.add_profile(&other)?;
        let renamed = ApiProfile {
            id: Some(other_id),
            name: "dup".into(),
            ..other
        };
        let err = db.update_profile(&renamed).expect_err("改名撞车必须失败");
        assert_eq!(err.kind(), ErrorKind::Conflict);
        assert!(
            err.to_string().contains("dup"),
            "冲突文案应保留名字，实得：{}",
            err
        );
        Ok(())
    }
}
