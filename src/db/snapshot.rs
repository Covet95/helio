//! 快照、导入替换与恢复。
//!
//! 三条路径都围绕「整个数据库文件」操作，而非表级读写：
//!
//! - `snapshot_to`：用 `VACUUM INTO` 产出全量副本（不是文件拷贝——直接拷
//!   主文件会漏掉还在 `-wal` 里的已提交数据）；
//! - `replace_file_from_import`：把导入库换成 live 库，含校验、staging 预跑
//!   迁移、备份、失败逐级回滚；
//! - `restore_replaced_file`：把备份还原回 live。
//!
//! `checkpoint_truncate` / `remove_sidecar_files` 是前两者的前置动作：
//! rename 不会搬走 `-wal`/`-shm`，不先合并就会丢数据。

use crate::utils::secure_fs::{ensure_private_dir, ensure_private_file, secure_export_file};
use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use super::{parent_dir, rollback_failed, Database, DB_BACKUP_KEEP};

impl Database {
    /// 把 WAL 内容合并进主文件并截断 `-wal`。
    /// 用于 rename 主文件之前——rename 不会搬走边车文件，未合并的写入会丢失。
    fn checkpoint_truncate(&self) -> Result<()> {
        self.conn
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .context("Failed to checkpoint write-ahead log")?;
        Ok(())
    }

    /// 删除数据库的 `-wal` / `-shm` 边车文件。
    ///
    /// 库以 WAL 模式打开，主文件可能落后于 `-wal`。替换或恢复主文件时若把旧 `-wal` 留在原地，
    /// SQLite 会用它去"恢复"新主文件，导致读回被替换掉的旧数据（实测可复现）。
    pub(crate) fn remove_sidecar_files(db_path: &Path) -> Result<()> {
        for suffix in ["-wal", "-shm"] {
            let mut name = db_path.as_os_str().to_os_string();
            name.push(suffix);
            let sidecar = PathBuf::from(name);
            if sidecar.exists() {
                fs::remove_file(&sidecar)
                    .with_context(|| format!("Failed to remove {}", sidecar.display()))?;
            }
        }
        Ok(())
    }

    /// 尽力清掉整个 staging 目录。
    ///
    /// staging 放在独立目录而不是直接放数据库目录：在它上面跑迁移会派生出边车文件和
    /// `*.premigrate.*` 备份（同样含明文密钥），逐个按名字删容易漏。整目录删除既覆盖
    /// 失败中止，也覆盖成功替换后的收尾。
    fn discard_staging(staging_dir: &Path) {
        let _ = fs::remove_dir_all(staging_dir);
    }

    /// 生成 `source` 的一致快照到 `dest`（单文件，含尚未 checkpoint 的 WAL 数据）。
    ///
    /// 用 `VACUUM INTO` 而非文件拷贝：拷主文件会丢 WAL 里已提交的数据，
    /// 连带 `-wal`/`-shm` 一起拷则得到三文件、不可移植且拷贝期间无快照隔离的备份。
    /// 只读连接即可执行 `VACUUM INTO`，不会写入源库。
    ///
    /// 覆盖既有 `dest` 时先写同目录临时文件，完整后再替换：失败保留旧备份
    /// （旧实现先 `remove_file(dest)` 再 VACUUM，磁盘满/中断会把唯一备份抹掉）。
    pub fn snapshot_to(source: &Path, dest: &Path) -> Result<()> {
        if !source.exists() {
            anyhow::bail!("Database does not exist: {}", source.display());
        }

        let parent = parent_dir(dest);
        // 用户导出路径可能尚未存在父目录；只创建 dest 的父目录，
        // 且不改权限（导出目录属于用户选择，不能 ensure_private_dir）。
        if parent != Path::new(".") {
            fs::create_dir_all(&parent).with_context(|| {
                format!("Failed to create export directory {}", parent.display())
            })?;
        }

        // VACUUM INTO 要求目标不存在；写到唯一临时名，成功后再替换 dest。
        let tmp = parent.join(format!(
            ".{}.tmp-{}",
            dest.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("export.db"),
            Uuid::new_v4()
        ));

        let snapshot_result: Result<()> = (|| {
            let conn = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .with_context(|| format!("Failed to read database {}", source.display()))?;
            let tmp_sql = tmp.to_string_lossy().replace('\'', "''");
            conn.execute_batch(&format!("VACUUM INTO '{tmp_sql}';"))
                .with_context(|| {
                    format!(
                        "Failed to write database snapshot to {}. \
                         请确认目标路径可写且磁盘空间充足。",
                        dest.display()
                    )
                })?;

            // VACUUM INTO 产出的文件是 0644（随 umask），凭据库必须收紧到 owner-only。
            secure_export_file(&tmp)?;

            // 优先直接 rename。目标已存在时（尤其 Windows 不能覆盖 rename）先把旧文件
            // 挪到旁路再替换；新文件此时已完整，失败则尽力把旧文件移回。
            match fs::rename(&tmp, dest) {
                Ok(()) => Ok(()),
                Err(first) if dest.exists() => {
                    let bak = parent.join(format!(
                        ".{}.replace-{}",
                        dest.file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("export.db"),
                        Uuid::new_v4()
                    ));
                    fs::rename(dest, &bak).with_context(|| {
                        format!(
                            "Failed to move old export aside {} (also: {first})",
                            dest.display()
                        )
                    })?;
                    if let Err(error) = fs::rename(&tmp, dest) {
                        let _ = fs::rename(&bak, dest);
                        return Err(error).with_context(|| {
                            format!("Failed to replace export {}", dest.display())
                        });
                    }
                    let _ = fs::remove_file(&bak);
                    Ok(())
                }
                Err(error) => Err(error)
                    .with_context(|| format!("Failed to move snapshot to {}", dest.display())),
            }
        })();

        if snapshot_result.is_err() || tmp.exists() {
            let _ = fs::remove_file(&tmp);
        }
        snapshot_result
    }

    /// 导入前校验候选文件是否为 Helio 档案库。**全程只读**，不写入也不迁移候选文件。
    ///
    /// 不能用 `Database::open` 当校验：`init_schema` 的 `CREATE TABLE IF NOT EXISTS`
    /// 会把任意 SQLite 文件（甚至 0 字节文件）补全成"合法"库，实测可把浏览器书签库
    /// 当备份导入并清空全部档案。`PRAGMA quick_check` 也不够——书签库同样返回 ok。
    pub fn validate_import_candidate(path: &Path) -> Result<()> {
        if !path.exists() {
            anyhow::bail!("Input database does not exist: {}", path.display());
        }
        // 0 字节文件会被 SQLite 当作合法空库接受。
        let size = fs::metadata(path)
            .with_context(|| format!("Failed to inspect {}", path.display()))?
            .len();
        if size == 0 {
            anyhow::bail!("File is empty, not a Helio database: {}", path.display());
        }

        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("Failed to open {}", path.display()))?;

        // 非 SQLite / 损坏文件在这里才报错（open 是惰性的）。
        let check: String = conn
            .query_row("PRAGMA quick_check;", [], |row| row.get(0))
            .with_context(|| format!("File is not a valid database: {}", path.display()))?;
        if check != "ok" {
            anyhow::bail!("Database is corrupted: {check}");
        }

        // 认 Helio 自己的 schema 特征。只查 api_profiles 及关键列，不要求最新 schema——
        // 旧版本导出的备份必须仍可导入，替换后由 Database::open 跑迁移补齐。
        let has_profiles: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='api_profiles'",
                [],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if !has_profiles {
            anyhow::bail!(
                "Not a Helio database (no api_profiles table): {}",
                path.display()
            );
        }

        let mut stmt = conn.prepare("PRAGMA table_info(api_profiles)")?;
        let columns: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<_>>()?;
        // 只认初版就存在的列。`target_app` 等是后续 ALTER TABLE 加的，
        // 要求它们会把用户的旧备份挡在门外——那是回归而非加固。
        for required in ["name", "provider", "api_url", "api_key"] {
            if !columns.iter().any(|c| c == required) {
                anyhow::bail!(
                    "Not a Helio database (api_profiles missing `{required}` column): {}",
                    path.display()
                );
            }
        }
        Ok(())
    }

    /// Validates an imported database in a private staging path, then atomically replaces a
    /// closed live database. Callers must drop the old `Database` connection before this method.
    pub fn replace_file_from_import(
        input_path: &Path,
        live_path: &Path,
    ) -> Result<Option<PathBuf>> {
        Self::validate_import_candidate(input_path)?;
        let parent = parent_dir(live_path);
        ensure_private_dir(&parent)?;

        // staging 单独建目录：迁移会派生边车与 `*.premigrate.*` 备份，围在一处才好整体清理。
        // 内容用一致快照而非裸拷贝：候选库自己可能带 -wal（例如另一个 Helio 实例的库副本）。
        let staging_dir = parent.join(format!(".db.import.{}", Uuid::new_v4()));
        ensure_private_dir(&staging_dir)?;
        let staging_path = staging_dir.join("db.sqlite");
        if let Err(error) = Self::snapshot_to(input_path, &staging_path) {
            Self::discard_staging(&staging_dir);
            return Err(error);
        }

        // 在**私有 staging 副本**上跑迁移：既验证该库确实能升到当前 schema
        // （迁移失败就在替换前中止，live 库不受影响），又让替换后的库无需再迁移。
        // 候选文件本身始终保持只读，迁移只作用于我们自己的副本。
        let staged = match Self::open(&staging_path) {
            Ok(migrated) => migrated,
            Err(error) => {
                Self::discard_staging(&staging_dir);
                return Err(error.context(format!(
                    "Cannot upgrade {} to the current schema",
                    input_path.display()
                )));
            }
        };
        // 迁移写入停留在 staging 的 -wal 里，而后续 rename 只搬主文件。
        // 必须先 checkpoint 把 WAL 合并进主文件，再删除边车文件——直接删 -wal 会丢迁移结果。
        let checkpoint = staged.checkpoint_truncate();
        drop(staged);
        if let Err(error) = checkpoint.and_then(|_| Self::remove_sidecar_files(&staging_path)) {
            Self::discard_staging(&staging_dir);
            return Err(error);
        }

        // 备份用快照（含 live 库 WAL 中的数据），成功后才移除 live 文件；
        // 旧实现直接 rename 主文件，会丢 WAL 数据且多一个中间失败态。
        let backup_path = if live_path.exists() {
            let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S_%f");
            let backup = live_path.with_file_name(format!("db.backup.{timestamp}.sqlite"));
            if let Err(error) = Self::snapshot_to(live_path, &backup) {
                Self::discard_staging(&staging_dir);
                return Err(error);
            }
            Some(backup)
        } else {
            None
        };

        let restore_from_backup = |error: anyhow::Error| -> anyhow::Error {
            if let Some(backup) = backup_path.as_ref() {
                // restore_replaced_file 内部会先清掉 live 及其边车文件再回滚。
                if let Err(restore) = Self::restore_replaced_file(live_path, backup) {
                    return rollback_failed(error, restore);
                }
            }
            error
        };

        if live_path.exists() {
            if let Err(error) = fs::remove_file(live_path) {
                Self::discard_staging(&staging_dir);
                return Err(restore_from_backup(error.into()));
            }
        }
        // 关键：旧 -wal/-shm 必须清掉，否则新库会被旧 WAL"恢复"成替换前的内容。
        if let Err(error) = Self::remove_sidecar_files(live_path) {
            Self::discard_staging(&staging_dir);
            return Err(restore_from_backup(error));
        }

        if let Err(error) = fs::rename(&staging_path, live_path) {
            Self::discard_staging(&staging_dir);
            return Err(restore_from_backup(error.into()));
        }
        ensure_private_file(live_path)?;
        // 主文件已 rename 走，剩下的迁移副产物（含明文密钥）随目录一并清掉。
        Self::discard_staging(&staging_dir);

        // 轮转失败不应让「已成功替换」的导入变成 Err：否则 GUI 会回滚刚导入的库，
        // 用户看到「导入失败」但其实主库已是新内容（或被二次回滚搞乱）。
        if backup_path.is_some() {
            if let Err(error) =
                crate::adapters::backup::cleanup_prefix(&parent, "db.backup.", DB_BACKUP_KEEP)
            {
                tracing::warn!("导入成功，但旧备份轮转失败（可稍后手动清理）: {error:#}");
            }
        }
        Ok(backup_path)
    }

    pub fn restore_replaced_file(live_path: &Path, backup_path: &Path) -> Result<()> {
        if live_path.exists() {
            fs::remove_file(live_path)?;
        }
        // 恢复的库同样不能套着失败导入留下的 -wal/-shm。
        Self::remove_sidecar_files(live_path)?;
        fs::rename(backup_path, live_path)?;
        ensure_private_file(live_path)?;
        Ok(())
    }
}
