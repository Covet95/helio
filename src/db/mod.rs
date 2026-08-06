use crate::models::{
    ActiveProfile, ApiProfile, ClaudeProfileFields, CodexProfileFields, HermesProfileFields,
    OpenClawProfileFields, OpenCodeManagedModelState, OpenCodeProfileFields, SharedConfig,
    TargetApp,
};
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::utils::secure_fs::{
    copy_private, ensure_private_dir, ensure_private_file, secure_export_file,
};

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
            // CLI 与 GUI 可能同时打开同一库：WAL + busy_timeout 避免 SQLITE_BUSY。
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

    /// 初始化数据库表结构
    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS api_profiles (
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
                service_tier TEXT,
                experimental_bearer_token TEXT,
                supports_standalone_web_search INTEGER,
                aws_profile TEXT,
                aws_region TEXT,
                api_mode TEXT,
                max_tokens INTEGER,
                api_keys_json TEXT,
                catalog_models TEXT,
                opencode_api_mode TEXT,
                opencode_model_configs TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                UNIQUE(name, target_app)
            );

            CREATE TABLE IF NOT EXISTS shared_configs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                target_app TEXT NOT NULL UNIQUE,
                config_json TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS opencode_model_state (
                provider_id TEXT PRIMARY KEY,
                model_ids_json TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS active_profiles (
                target_app TEXT PRIMARY KEY,
                profile_id INTEGER NOT NULL,
                FOREIGN KEY (profile_id) REFERENCES api_profiles(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_profiles_name ON api_profiles(name);
            CREATE INDEX IF NOT EXISTS idx_shared_configs_app ON shared_configs(target_app);

            CREATE TABLE IF NOT EXISTS schema_migrations (
                id TEXT PRIMARY KEY,
                applied_at INTEGER NOT NULL
            );
            "#,
        )?;

        self.ensure_current_profile_columns()?;
        self.migrate_composite_unique()?;
        self.migrate_drop_model_effort_level()?;
        self.migrate_drop_model_thinking_enabled()?;
        self.conn.execute(
            "UPDATE api_profiles SET wire_api = 'responses' WHERE lower(trim(wire_api)) = 'chat'",
            [],
        )?;
        self.record_migration("2026-07-19-profile-schema-ledger")?;
        self.migrate_drop_gemini_target()?;

        Ok(())
    }

    /// Drop historical Gemini target rows (tool removed in favor of Pi).
    fn migrate_drop_gemini_target(&self) -> Result<()> {
        let id = "2026-07-28-drop-gemini-target";
        let already: bool = self
            .conn
            .query_row(
                "SELECT 1 FROM schema_migrations WHERE id = ?1",
                params![id],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if already {
            return Ok(());
        }
        self.conn.execute(
            "DELETE FROM active_profiles WHERE target_app = 'gemini'",
            [],
        )?;
        self.conn
            .execute("DELETE FROM shared_configs WHERE target_app = 'gemini'", [])?;
        self.conn
            .execute("DELETE FROM api_profiles WHERE target_app = 'gemini'", [])?;
        self.record_migration(id)?;
        Ok(())
    }

    fn ensure_current_profile_columns(&self) -> Result<()> {
        for ddl in [
            "ALTER TABLE api_profiles ADD COLUMN model TEXT",
            "ALTER TABLE api_profiles ADD COLUMN reasoning_effort TEXT",
            "ALTER TABLE api_profiles ADD COLUMN context_1m INTEGER",
            "ALTER TABLE api_profiles ADD COLUMN target_app TEXT",
            "ALTER TABLE api_profiles ADD COLUMN models TEXT",
            "ALTER TABLE api_profiles ADD COLUMN wire_api TEXT",
            "ALTER TABLE api_profiles ADD COLUMN env_key TEXT",
            "ALTER TABLE api_profiles ADD COLUMN requires_openai_auth INTEGER",
            "ALTER TABLE api_profiles ADD COLUMN service_tier TEXT",
            "ALTER TABLE api_profiles ADD COLUMN experimental_bearer_token TEXT",
            "ALTER TABLE api_profiles ADD COLUMN supports_standalone_web_search INTEGER",
            "ALTER TABLE api_profiles ADD COLUMN aws_profile TEXT",
            "ALTER TABLE api_profiles ADD COLUMN aws_region TEXT",
            "ALTER TABLE api_profiles ADD COLUMN api_mode TEXT",
            "ALTER TABLE api_profiles ADD COLUMN max_tokens INTEGER",
            "ALTER TABLE api_profiles ADD COLUMN api_keys_json TEXT",
            "ALTER TABLE api_profiles ADD COLUMN catalog_models TEXT",
            "ALTER TABLE api_profiles ADD COLUMN opencode_api_mode TEXT",
            "ALTER TABLE api_profiles ADD COLUMN opencode_model_configs TEXT",
        ] {
            self.try_add_column(ddl)?;
        }
        Ok(())
    }

    fn record_migration(&self, id: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO schema_migrations (id, applied_at) VALUES (?1, ?2)",
            params![id, chrono::Utc::now().timestamp()],
        )?;
        Ok(())
    }

    fn try_add_column(&self, ddl: &str) -> Result<()> {
        match self.conn.execute(ddl, []) {
            Ok(_) => Ok(()),
            Err(e) if e.to_string().contains("duplicate column name") => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    fn migrate_drop_model_effort_level(&self) -> Result<()> {
        let mut stmt = self.conn.prepare("PRAGMA table_info(api_profiles)")?;
        let cols: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        if !cols.iter().any(|c| c == "model_effort_level") {
            return Ok(());
        }
        // guard 负责失败时回滚残留事务并恢复 foreign_keys=ON。
        let _guard = ForeignKeysGuard::off(&self.conn)?;
        self.conn.execute_batch(r#"
            BEGIN;
            CREATE TABLE api_profiles_no_effort (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL, provider TEXT NOT NULL, api_url TEXT NOT NULL, api_key TEXT NOT NULL,
                model_mapping TEXT, model TEXT, reasoning_effort TEXT, context_1m INTEGER,
                target_app TEXT, models TEXT, wire_api TEXT, env_key TEXT, requires_openai_auth INTEGER,
                service_tier TEXT, experimental_bearer_token TEXT,
                supports_standalone_web_search INTEGER, aws_profile TEXT, aws_region TEXT,
                api_mode TEXT, max_tokens INTEGER, api_keys_json TEXT, catalog_models TEXT,
                opencode_api_mode TEXT, opencode_model_configs TEXT,
                created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
                UNIQUE(name, target_app)
            );
            INSERT INTO api_profiles_no_effort (
                id, name, provider, api_url, api_key, model_mapping, model, reasoning_effort,
                context_1m, target_app, models, wire_api, env_key, requires_openai_auth,
                service_tier, experimental_bearer_token, api_mode, max_tokens,
                supports_standalone_web_search, aws_profile, aws_region, api_keys_json,
                catalog_models, opencode_api_mode, opencode_model_configs, created_at, updated_at
            )
            SELECT id, name, provider, api_url, api_key, model_mapping, model, reasoning_effort,
                context_1m, target_app, models, wire_api, env_key, requires_openai_auth,
                service_tier, experimental_bearer_token,
                api_mode, max_tokens, supports_standalone_web_search, aws_profile, aws_region,
                api_keys_json, catalog_models, opencode_api_mode, opencode_model_configs,
                created_at, updated_at
            FROM api_profiles;
            DROP TABLE api_profiles;
            ALTER TABLE api_profiles_no_effort RENAME TO api_profiles;
            CREATE INDEX IF NOT EXISTS idx_profiles_name ON api_profiles(name);
            COMMIT;
        "#)?;
        Ok(())
    }

    fn migrate_drop_model_thinking_enabled(&self) -> Result<()> {
        let id = "2026-08-03-drop-codex-model-thinking-enabled";
        let already: bool = self
            .conn
            .query_row(
                "SELECT 1 FROM schema_migrations WHERE id = ?1",
                params![id],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if already {
            return Ok(());
        }

        let mut stmt = self.conn.prepare("PRAGMA table_info(api_profiles)")?;
        let cols: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        if !cols.iter().any(|column| column == "model_thinking_enabled") {
            self.record_migration(id)?;
            return Ok(());
        }

        let _guard = ForeignKeysGuard::off(&self.conn)?;
        self.conn.execute_batch(
            r#"
            BEGIN;
            CREATE TABLE api_profiles_no_thinking (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL, provider TEXT NOT NULL, api_url TEXT NOT NULL, api_key TEXT NOT NULL,
                model_mapping TEXT, model TEXT, reasoning_effort TEXT, context_1m INTEGER,
                target_app TEXT, models TEXT, wire_api TEXT, env_key TEXT, requires_openai_auth INTEGER,
                service_tier TEXT, experimental_bearer_token TEXT,
                supports_standalone_web_search INTEGER, aws_profile TEXT, aws_region TEXT,
                api_mode TEXT, max_tokens INTEGER, api_keys_json TEXT, catalog_models TEXT,
                opencode_api_mode TEXT, opencode_model_configs TEXT,
                created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
                UNIQUE(name, target_app)
            );
            INSERT INTO api_profiles_no_thinking (
                id, name, provider, api_url, api_key, model_mapping, model, reasoning_effort,
                context_1m, target_app, models, wire_api, env_key, requires_openai_auth,
                service_tier, experimental_bearer_token, supports_standalone_web_search,
                aws_profile, aws_region, api_mode, max_tokens, api_keys_json, catalog_models,
                opencode_api_mode, opencode_model_configs, created_at, updated_at
            )
            SELECT id, name, provider, api_url, api_key, model_mapping, model, reasoning_effort,
                context_1m, target_app, models, wire_api, env_key, requires_openai_auth,
                service_tier, experimental_bearer_token, supports_standalone_web_search,
                aws_profile, aws_region, api_mode, max_tokens, api_keys_json, catalog_models,
                opencode_api_mode, opencode_model_configs, created_at, updated_at
            FROM api_profiles;
            DROP TABLE api_profiles;
            ALTER TABLE api_profiles_no_thinking RENAME TO api_profiles;
            CREATE INDEX IF NOT EXISTS idx_profiles_name ON api_profiles(name);
            COMMIT;
            "#,
        )?;
        self.record_migration(id)
    }

    /// 幂等迁移:name 全局 UNIQUE → UNIQUE(name, target_app)，并去掉历史 `-cc` 后缀。
    /// 仅当旧约束仍存在时执行;执行前备份库文件。
    fn migrate_composite_unique(&self) -> Result<()> {
        let create_sql: String = self.conn.query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='api_profiles'",
            [],
            |r| r.get(0),
        )?;
        // 已是复合唯一 → 跳过
        if create_sql.contains("UNIQUE(name, target_app)")
            || create_sql.contains("UNIQUE (name, target_app)")
        {
            return Ok(());
        }
        // 不含旧的全局 name UNIQUE 也跳过(防御)
        if !create_sql.contains("name TEXT NOT NULL UNIQUE") {
            return Ok(());
        }

        // 备份库文件(若是文件库)。:memory: 没有路径，跳过备份。
        if let Some(db_path) = self.conn.path() {
            if db_path != ":memory:" && !db_path.is_empty() {
                let ts = chrono::Local::now().format("%Y%m%d_%H%M%S_%f");
                let backup = format!("{db_path}.premigrate.{ts}.sqlite");
                // 迁移会重建整表,备份失败必须中止迁移,否则旧数据无兜底。
                copy_private(Path::new(db_path), Path::new(&backup)).with_context(|| {
                    format!("Failed to back up database before migration: {}", backup)
                })?;
                // 备份含明文 key，必须轮转；文件名形如 `db.sqlite.premigrate.*`。
                let path = Path::new(db_path);
                if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                    crate::adapters::backup::cleanup_prefix(
                        &parent_dir(path),
                        &format!("{file_name}.premigrate."),
                        DB_BACKUP_KEEP,
                    )
                    .with_context(|| "Failed to rotate pre-migration backups")?;
                }
            }
        }

        // 重建表:新表用复合唯一。注意去 -cc 后缀(仅 target_app 非空、去后缀后同工具不冲突)。
        // 整个重建流程包在单个事务中以保证原子性(防止 DROP 与 RENAME 之间进程被杀留下孤表)。
        //
        // 关键:active_profiles 有 FOREIGN KEY ... REFERENCES api_profiles(id)。开启外键检查时
        // `DROP TABLE api_profiles` 会触发 FOREIGN KEY constraint failed 导致整个事务回滚。
        // 按 SQLite 官方安全重建表流程,重建期间必须关闭外键检查;
        // 而 `PRAGMA foreign_keys` 在事务内是 no-op,必须在 BEGIN 之前设置、COMMIT 之后恢复。
        // guard 负责失败时回滚残留事务并恢复 foreign_keys=ON。
        let _guard = ForeignKeysGuard::off(&self.conn)?;
        self.conn.execute_batch(
            r#"
            BEGIN;
            CREATE TABLE api_profiles_new (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                provider TEXT NOT NULL,
                api_url TEXT NOT NULL,
                api_key TEXT NOT NULL,
                model_mapping TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                model TEXT,
                reasoning_effort TEXT,
                context_1m INTEGER,
                target_app TEXT,
                models TEXT,
                wire_api TEXT,
                env_key TEXT,
                requires_openai_auth INTEGER,
                service_tier TEXT,
                experimental_bearer_token TEXT,
                supports_standalone_web_search INTEGER,
                aws_profile TEXT,
                aws_region TEXT,
                api_mode TEXT,
                max_tokens INTEGER,
                api_keys_json TEXT,
                catalog_models TEXT,
                opencode_api_mode TEXT,
                opencode_model_configs TEXT,
                UNIQUE(name, target_app)
            );

            INSERT INTO api_profiles_new
                (id,name,provider,api_url,api_key,model_mapping,created_at,updated_at,model,reasoning_effort,context_1m,target_app,models,
                 wire_api,env_key,requires_openai_auth,service_tier,experimental_bearer_token,
                 supports_standalone_web_search,aws_profile,aws_region,api_mode,max_tokens,api_keys_json,
                 catalog_models,opencode_api_mode,opencode_model_configs)
            SELECT id,name,provider,api_url,api_key,model_mapping,created_at,updated_at,model,reasoning_effort,context_1m,target_app,models,
                 wire_api,env_key,requires_openai_auth,service_tier,experimental_bearer_token,
                 supports_standalone_web_search,aws_profile,aws_region,api_mode,max_tokens,api_keys_json,
                 catalog_models,opencode_api_mode,opencode_model_configs
            FROM api_profiles;

            UPDATE api_profiles_new
            SET name = substr(name, 1, length(name) - 3)
            WHERE name LIKE '%-cc'
              AND target_app IS NOT NULL
              AND NOT EXISTS (
                  SELECT 1 FROM api_profiles_new b
                  WHERE b.target_app = api_profiles_new.target_app
                    AND b.name = substr(api_profiles_new.name, 1, length(api_profiles_new.name) - 3)
                    AND b.id != api_profiles_new.id
              );

            DROP TABLE api_profiles;
            ALTER TABLE api_profiles_new RENAME TO api_profiles;
            CREATE INDEX IF NOT EXISTS idx_profiles_name ON api_profiles(name);
            COMMIT;
            "#,
        )?;

        Ok(())
    }

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
    fn remove_sidecar_files(db_path: &Path) -> Result<()> {
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
                    return anyhow::anyhow!("{error}; rollback failed: {restore}");
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

    // ========== API Profile 操作 ==========

    /// 添加 API Profile
    pub fn add_profile(&self, profile: &ApiProfile) -> Result<i64> {
        let mut profile = profile.clone();
        profile.normalize_keys();
        let now = chrono::Utc::now().timestamp();
        let model_mapping_json = profile
            .claude
            .model_mapping
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let models_json = profile
            .opencode
            .models
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let catalog_models_json = profile
            .codex
            .catalog_models
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let opencode_model_configs_json = profile
            .opencode
            .model_configs
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let api_keys_json = Self::serialize_api_keys_json(&profile)?;
        let (api_mode, max_tokens, opencode_api_mode) = match profile.target_app {
            Some(TargetApp::OpenClaw) => (
                profile.openclaw.api_mode.as_ref(),
                profile.openclaw.max_tokens,
                None,
            ),
            Some(TargetApp::Hermes) => (profile.hermes.api_mode.as_ref(), None, None),
            Some(TargetApp::OpenCode) => (None, None, profile.opencode.opencode_api_mode.as_ref()),
            _ => (
                profile
                    .hermes
                    .api_mode
                    .as_ref()
                    .or(profile.openclaw.api_mode.as_ref()),
                profile.openclaw.max_tokens,
                None,
            ),
        };

        self.conn.execute(
            "INSERT INTO api_profiles (name, provider, api_url, api_key, model_mapping, model, reasoning_effort, context_1m, target_app, models, wire_api, env_key, requires_openai_auth, service_tier, experimental_bearer_token, supports_standalone_web_search, aws_profile, aws_region, api_mode, max_tokens, api_keys_json, catalog_models, opencode_api_mode, opencode_model_configs, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26)",
            params![
                &profile.name,
                &profile.provider,
                &profile.api_url,
                &profile.api_key,
                model_mapping_json,
                &profile.model,
                &profile.codex.reasoning_effort,
                profile.context_1m.map(|b| b as i64),
                profile.target_app.as_ref().map(|t| t.as_str()),
                models_json,
                Some("responses"),
                &profile.codex.env_key,
                profile.codex.requires_openai_auth.map(|b| b as i64),
                &profile.codex.service_tier,
                &profile.codex.experimental_bearer_token,
                profile.codex.supports_standalone_web_search.map(|b| b as i64),
                &profile.codex.aws_profile,
                &profile.codex.aws_region,
                api_mode,
                max_tokens,
                api_keys_json,
                catalog_models_json,
                opencode_api_mode,
                opencode_model_configs_json,
                now,
                now
            ],
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    /// 把一行 (13 列固定顺序) 映射为 ApiProfile，供各 SELECT 复用。
    const PROFILE_SELECT: &'static str = concat!(
        "id, name, provider, api_url, api_key, model_mapping, model, ",
        "reasoning_effort, context_1m, created_at, updated_at, target_app, models, ",
        "wire_api, env_key, requires_openai_auth, service_tier, experimental_bearer_token, ",
        "supports_standalone_web_search, aws_profile, aws_region, api_mode, max_tokens, ",
        "api_keys_json, catalog_models, opencode_api_mode, opencode_model_configs"
    );

    fn row_to_profile(row: &rusqlite::Row) -> rusqlite::Result<ApiProfile> {
        let model_mapping_str: Option<String> = row.get("model_mapping")?;
        let model_mapping = model_mapping_str
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        let context_1m: Option<i64> = row.get("context_1m")?;
        let target_app_str: Option<String> = row.get("target_app")?;
        let target_app = target_app_str.as_deref().and_then(TargetApp::parse);
        let models_str: Option<String> = row.get("models")?;
        let models = models_str
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        let requires_openai_auth: Option<i64> = row.get("requires_openai_auth")?;
        let supports_standalone_web_search: Option<i64> =
            row.get("supports_standalone_web_search")?;
        let api_mode: Option<String> = row.get("api_mode")?;
        let max_tokens: Option<i64> = row.get("max_tokens")?;
        let api_keys_str: Option<String> = row.get("api_keys_json")?;
        let api_keys = api_keys_str
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(serde_json::from_str)
            .transpose()
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        let catalog_models_str: Option<String> = row.get("catalog_models")?;
        let catalog_models = catalog_models_str
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(serde_json::from_str)
            .transpose()
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        let opencode_model_configs_str: Option<String> = row.get("opencode_model_configs")?;
        let opencode_model_configs = opencode_model_configs_str
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(serde_json::from_str)
            .transpose()
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        let opencode_api_mode: Option<String> = row.get("opencode_api_mode")?;

        // 工具字段按 target_app 归属，避免 Hermes/OpenClaw 互相污染
        let (hermes_api_mode, openclaw_api_mode, openclaw_max_tokens) = match target_app {
            Some(TargetApp::Hermes) => (api_mode, None, None),
            Some(TargetApp::OpenClaw) => (None, api_mode, max_tokens),
            _ => (api_mode.clone(), api_mode, max_tokens),
        };

        let mut profile = ApiProfile {
            id: Some(row.get("id")?),
            name: row.get("name")?,
            provider: row.get("provider")?,
            api_url: row.get("api_url")?,
            api_key: row.get("api_key")?,
            api_keys,
            model: row.get("model")?,
            context_1m: context_1m.map(|v| v != 0),
            created_at: Some(row.get("created_at")?),
            updated_at: Some(row.get("updated_at")?),
            target_app,
            claude: ClaudeProfileFields { model_mapping },
            codex: CodexProfileFields {
                reasoning_effort: row.get("reasoning_effort")?,
                wire_api: Some("responses".to_string()),
                env_key: row.get("env_key")?,
                requires_openai_auth: requires_openai_auth.map(|v| v != 0),
                service_tier: row.get("service_tier")?,
                experimental_bearer_token: row.get("experimental_bearer_token")?,
                supports_standalone_web_search: supports_standalone_web_search.map(|v| v != 0),
                aws_profile: row.get("aws_profile")?,
                aws_region: row.get("aws_region")?,
                catalog_models,
            },
            opencode: OpenCodeProfileFields {
                models,
                opencode_api_mode,
                model_configs: opencode_model_configs,
            },
            hermes: HermesProfileFields {
                api_mode: hermes_api_mode,
            },
            openclaw: OpenClawProfileFields {
                api_mode: openclaw_api_mode,
                max_tokens: openclaw_max_tokens,
            },
        };
        // 老数据：仅有 api_key → 运行时归一为单条 default active
        profile.normalize_keys();
        Ok(profile)
    }

    fn serialize_api_keys_json(profile: &ApiProfile) -> Result<Option<String>> {
        match &profile.api_keys {
            Some(keys) if !keys.is_empty() => Ok(Some(serde_json::to_string(keys)?)),
            _ => Ok(None),
        }
    }

    /// 按 (name, target_app) 精确获取 profile。
    pub fn get_profile_by_name_and_target(
        &self,
        name: &str,
        target: TargetApp,
    ) -> Result<ApiProfile> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {} FROM api_profiles WHERE name = ?1 AND target_app = ?2",
            Self::PROFILE_SELECT
        ))?;
        let profile = stmt.query_row(params![name, target.as_str()], Self::row_to_profile)?;
        Ok(profile)
    }

    /// 某工具下是否已存在同名 profile(可排除某 id,用于改名校验)。
    ///
    /// 仅 GUI(tauri-gui)的 import 流程调用;CLI bin 不走此路径,故对 CLI 编译标记 allow。
    #[cfg_attr(not(feature = "tauri-gui"), allow(dead_code))]
    pub fn profile_name_exists(
        &self,
        name: &str,
        target: TargetApp,
        exclude_id: Option<i64>,
    ) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM api_profiles WHERE name = ?1 AND target_app = ?2 AND (?3 IS NULL OR id != ?3)",
            params![name, target.as_str(), exclude_id],
            |r| r.get(0),
        )?;
        Ok(count > 0)
    }

    /// 列出所有 API Profiles
    pub fn list_profiles(&self) -> Result<Vec<ApiProfile>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {} FROM api_profiles ORDER BY name",
            Self::PROFILE_SELECT
        ))?;

        let profiles = stmt
            .query_map([], Self::row_to_profile)?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(profiles)
    }

    /// 更新 API Profile
    ///
    /// 按 `id` 定位记录（而非 name），因此**支持改名**。
    /// id 为空时回退到按旧 name 定位（理论上现有 profile 都带 id）。
    pub fn update_profile(&self, profile: &ApiProfile) -> Result<()> {
        let mut profile = profile.clone();
        profile.normalize_keys();
        let now = chrono::Utc::now().timestamp();
        let model_mapping_json = profile
            .claude
            .model_mapping
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let models_json = profile
            .opencode
            .models
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let catalog_models_json = profile
            .codex
            .catalog_models
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let opencode_model_configs_json = profile
            .opencode
            .model_configs
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let api_keys_json = Self::serialize_api_keys_json(&profile)?;
        let (api_mode, max_tokens, opencode_api_mode) = match profile.target_app {
            Some(TargetApp::OpenClaw) => (
                profile.openclaw.api_mode.as_ref(),
                profile.openclaw.max_tokens,
                None,
            ),
            Some(TargetApp::Hermes) => (profile.hermes.api_mode.as_ref(), None, None),
            Some(TargetApp::OpenCode) => (None, None, profile.opencode.opencode_api_mode.as_ref()),
            _ => (
                profile
                    .hermes
                    .api_mode
                    .as_ref()
                    .or(profile.openclaw.api_mode.as_ref()),
                profile.openclaw.max_tokens,
                None,
            ),
        };

        match profile.id {
            Some(id) => {
                self.conn.execute(
                    "UPDATE api_profiles SET name = ?1, provider = ?2, api_url = ?3, api_key = ?4,
                     model_mapping = ?5, model = ?6, reasoning_effort = ?7, context_1m = ?8, target_app = ?9, models = ?10, wire_api = ?11, env_key = ?12, requires_openai_auth = ?13, service_tier = ?14, experimental_bearer_token = ?15, supports_standalone_web_search = ?16, aws_profile = ?17, aws_region = ?18, api_mode = ?19, max_tokens = ?20, api_keys_json = ?21, catalog_models = ?22, opencode_api_mode = ?23, opencode_model_configs = ?24, updated_at = ?25 WHERE id = ?26",
                    params![
                        &profile.name,
                        &profile.provider,
                        &profile.api_url,
                        &profile.api_key,
                        model_mapping_json,
                        &profile.model,
                        &profile.codex.reasoning_effort,
                        profile.context_1m.map(|b| b as i64),
                        profile.target_app.as_ref().map(|t| t.as_str()),
                        models_json,
                        Some("responses"),
                        &profile.codex.env_key,
                        profile.codex.requires_openai_auth.map(|b| b as i64),
                        &profile.codex.service_tier,
                        &profile.codex.experimental_bearer_token,
                        profile.codex.supports_standalone_web_search.map(|b| b as i64),
                        &profile.codex.aws_profile,
                        &profile.codex.aws_region,
                        api_mode,
                        max_tokens,
                        api_keys_json,
                        catalog_models_json,
                        opencode_api_mode,
                        opencode_model_configs_json,
                        now,
                        id
                    ],
                )?;
            }
            None => {
                // 无 id：按 name 定位，不改名
                self.conn.execute(
                    "UPDATE api_profiles SET provider = ?1, api_url = ?2, api_key = ?3,
                     model_mapping = ?4, model = ?5, reasoning_effort = ?6, context_1m = ?7, target_app = ?8, models = ?9, wire_api = ?10, env_key = ?11, requires_openai_auth = ?12, service_tier = ?13, experimental_bearer_token = ?14, supports_standalone_web_search = ?15, aws_profile = ?16, aws_region = ?17, api_mode = ?18, max_tokens = ?19, api_keys_json = ?20, catalog_models = ?21, opencode_api_mode = ?22, opencode_model_configs = ?23, updated_at = ?24 WHERE name = ?25",
                    params![
                        &profile.provider,
                        &profile.api_url,
                        &profile.api_key,
                        model_mapping_json,
                        &profile.model,
                        &profile.codex.reasoning_effort,
                        profile.context_1m.map(|b| b as i64),
                        profile.target_app.as_ref().map(|t| t.as_str()),
                        models_json,
                        Some("responses"),
                        &profile.codex.env_key,
                        profile.codex.requires_openai_auth.map(|b| b as i64),
                        &profile.codex.service_tier,
                        &profile.codex.experimental_bearer_token,
                        profile.codex.supports_standalone_web_search.map(|b| b as i64),
                        &profile.codex.aws_profile,
                        &profile.codex.aws_region,
                        api_mode,
                        max_tokens,
                        api_keys_json,
                        catalog_models_json,
                        opencode_api_mode,
                        opencode_model_configs_json,
                        now,
                        &profile.name
                    ],
                )?;
            }
        }

        Ok(())
    }

    /// 删除某工具下指定名称的 Profile。
    pub fn delete_profile(&self, name: &str, target: TargetApp) -> Result<bool> {
        // 先查 id 清理 active 引用，再删
        let id: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM api_profiles WHERE name = ?1 AND target_app = ?2",
                params![name, target.as_str()],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(pid) = id {
            self.conn.execute(
                "DELETE FROM active_profiles WHERE profile_id = ?1",
                params![pid],
            )?;
        }
        let rows = self.conn.execute(
            "DELETE FROM api_profiles WHERE name = ?1 AND target_app = ?2",
            params![name, target.as_str()],
        )?;
        Ok(rows > 0)
    }

    // ========== 共享配置操作 ==========

    /// 保存共享配置
    pub fn save_shared_config(
        &self,
        target_app: TargetApp,
        config: serde_json::Value,
    ) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        let config_json = serde_json::to_string(&config)?;

        self.conn.execute(
            "INSERT OR REPLACE INTO shared_configs (target_app, config_json, updated_at)
             VALUES (?1, ?2, ?3)",
            params![target_app.as_str(), config_json, now],
        )?;

        Ok(())
    }

    /// 删除共享配置（切换事务回滚到「从未保存过」状态时使用）。
    pub fn delete_shared_config(&self, target_app: TargetApp) -> Result<()> {
        self.conn.execute(
            "DELETE FROM shared_configs WHERE target_app = ?1",
            params![target_app.as_str()],
        )?;
        Ok(())
    }

    /// 获取共享配置
    pub fn get_shared_config(&self, target_app: TargetApp) -> Result<Option<SharedConfig>> {
        let mut stmt = self.conn.prepare(
            "SELECT target_app, config_json, updated_at FROM shared_configs WHERE target_app = ?1",
        )?;

        let result = stmt
            .query_row(params![target_app.as_str()], |row| {
                let config_json: String = row.get(1)?;
                let config = serde_json::from_str(&config_json)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;

                Ok(SharedConfig {
                    target_app,
                    config,
                    updated_at: Some(row.get(2)?),
                })
            })
            .optional()?;

        Ok(result)
    }

    /// Return the model IDs last written by Helio for each OpenCode provider.
    pub fn get_opencode_managed_models(&self) -> Result<OpenCodeManagedModelState> {
        let mut stmt = self
            .conn
            .prepare("SELECT provider_id, model_ids_json FROM opencode_model_state")?;
        let mut state = OpenCodeManagedModelState::new();
        let rows = stmt.query_map([], |row| {
            let provider_id: String = row.get(0)?;
            let model_ids_json: String = row.get(1)?;
            let model_ids = serde_json::from_str(&model_ids_json)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
            Ok((provider_id, model_ids))
        })?;
        for row in rows {
            let (provider_id, model_ids) = row?;
            state.insert(provider_id, model_ids);
        }
        Ok(state)
    }

    /// Replace the complete OpenCode model ownership snapshot atomically.
    pub fn replace_opencode_managed_models(&self, state: &OpenCodeManagedModelState) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM opencode_model_state", [])?;
        let now = chrono::Utc::now().timestamp();
        for (provider_id, model_ids) in state {
            tx.execute(
                "INSERT INTO opencode_model_state (provider_id, model_ids_json, updated_at)
                 VALUES (?1, ?2, ?3)",
                params![provider_id, serde_json::to_string(model_ids)?, now],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Remove ownership metadata for a provider that is no longer used.
    pub fn clear_opencode_managed_provider(&self, provider_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM opencode_model_state WHERE provider_id = ?1",
            params![provider_id.to_lowercase()],
        )?;
        Ok(())
    }

    // ========== 活动 Profile 操作 ==========

    /// 设置活动 Profile
    pub fn set_active_profile(&self, target_app: TargetApp, profile_id: i64) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO active_profiles (target_app, profile_id) VALUES (?1, ?2)",
            params![target_app.as_str(), profile_id],
        )?;
        Ok(())
    }

    /// 清除某工具的活动 Profile（切换事务在「重复切换同一 profile」时用来制造
    /// `active != target` 窗口，使崩溃恢复能区分「已完成」与「半完成」）。
    pub fn clear_active_profile(&self, target_app: TargetApp) -> Result<()> {
        self.conn.execute(
            "DELETE FROM active_profiles WHERE target_app = ?1",
            params![target_app.as_str()],
        )?;
        Ok(())
    }

    /// 获取活动 Profile
    pub fn get_active_profile(&self, target_app: TargetApp) -> Result<Option<ActiveProfile>> {
        let mut stmt = self
            .conn
            .prepare("SELECT profile_id FROM active_profiles WHERE target_app = ?1")?;

        let result = stmt
            .query_row(params![target_app.as_str()], |row| {
                Ok(ActiveProfile {
                    profile_id: row.get(0)?,
                })
            })
            .optional()?;

        Ok(result)
    }

    pub fn get_profile_by_id(&self, id: i64) -> Result<Option<ApiProfile>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {} FROM api_profiles WHERE id = ?1",
            Self::PROFILE_SELECT
        ))?;
        Ok(stmt
            .query_row(params![id], Self::row_to_profile)
            .optional()?)
    }

    pub fn get_active_profile_full(&self, target_app: TargetApp) -> Result<Option<ApiProfile>> {
        match self.get_active_profile(target_app)? {
            Some(active) => self.get_profile_by_id(active.profile_id),
            None => Ok(None),
        }
    }

    /// 获取某个 Profile 当前被哪些工具启用。
    #[cfg(feature = "tauri-gui")]
    pub fn get_active_targets_for_profile(&self, profile_id: i64) -> Result<Vec<TargetApp>> {
        let mut stmt = self.conn.prepare(
            "SELECT target_app FROM active_profiles WHERE profile_id = ?1 ORDER BY target_app",
        )?;

        let targets = stmt
            .query_map(params![profile_id], |row| row.get::<_, String>(0))?
            .filter_map(|row| row.ok())
            .filter_map(|target| TargetApp::parse(&target))
            .collect();

        Ok(targets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

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

        let backup_path = Database::replace_file_from_import(&import_path, &live_path)?
            .expect("替换已存在的库应产生备份");
        drop(reader);

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
                wire_api: Some("responses".into()),
                env_key: Some("MY_CODEX_KEY".into()),
                experimental_bearer_token: Some("sk-b".into()),
                supports_standalone_web_search: Some(true),
                aws_profile: Some("production".into()),
                aws_region: Some("us-east-1".into()),
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
        assert_eq!(got.codex.env_key.as_deref(), Some("MY_CODEX_KEY"));
        assert_eq!(got.codex.experimental_bearer_token.as_deref(), Some("sk-b"));
        assert_eq!(got.codex.supports_standalone_web_search, Some(true));
        assert_eq!(got.codex.aws_profile.as_deref(), Some("production"));
        assert_eq!(got.codex.aws_region.as_deref(), Some("us-east-1"));
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
    #[cfg(feature = "tauri-gui")]
    fn test_get_active_targets_for_profile_returns_all_matches() -> Result<()> {
        let db = Database::open(":memory:")?;

        let profile = ApiProfile {
            name: "shared".to_string(),
            provider: "anthropic".to_string(),
            api_url: "https://api.example.com/v1".to_string(),
            api_key: "sk-test-key".to_string(),
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
}
