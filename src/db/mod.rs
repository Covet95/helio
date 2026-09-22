use crate::error::AppError;
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
                reasoning_summary TEXT,
                verbosity TEXT,
                auth_command TEXT,
                auth_args TEXT,
                auth_timeout_ms INTEGER,
                auth_refresh_interval_ms INTEGER,
                auth_cwd TEXT,
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

            CREATE TABLE IF NOT EXISTS provider_ownership (
                target_app TEXT NOT NULL,
                provider_id TEXT NOT NULL,
                managed_by_helio INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (target_app, provider_id)
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
        self.migrate_normalize_codex_wire_api()?;
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
            "ALTER TABLE api_profiles ADD COLUMN reasoning_summary TEXT",
            "ALTER TABLE api_profiles ADD COLUMN verbosity TEXT",
            "ALTER TABLE api_profiles ADD COLUMN auth_command TEXT",
            "ALTER TABLE api_profiles ADD COLUMN auth_args TEXT",
            "ALTER TABLE api_profiles ADD COLUMN auth_timeout_ms INTEGER",
            "ALTER TABLE api_profiles ADD COLUMN auth_refresh_interval_ms INTEGER",
            "ALTER TABLE api_profiles ADD COLUMN auth_cwd TEXT",
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
        // 重建整表前备份：这张表装着全部明文 key，备份失败必须中止迁移。
        backup_before_table_rebuild(&self.conn)?;
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
                reasoning_summary TEXT, verbosity TEXT,
                auth_command TEXT, auth_args TEXT, auth_timeout_ms INTEGER,
                auth_refresh_interval_ms INTEGER, auth_cwd TEXT,
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
                catalog_models, opencode_api_mode, opencode_model_configs, created_at, updated_at,
                reasoning_summary, verbosity, auth_command, auth_args, auth_timeout_ms,
                auth_refresh_interval_ms, auth_cwd
            )
            SELECT id, name, provider, api_url, api_key, model_mapping, model, reasoning_effort,
                context_1m, target_app, models, wire_api, env_key, requires_openai_auth,
                service_tier, experimental_bearer_token,
                api_mode, max_tokens, supports_standalone_web_search, aws_profile, aws_region,
                api_keys_json, catalog_models, opencode_api_mode, opencode_model_configs,
                created_at, updated_at,
                reasoning_summary, verbosity, auth_command, auth_args, auth_timeout_ms,
                auth_refresh_interval_ms, auth_cwd
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

        // 重建整表前备份：这张表装着全部明文 key，备份失败必须中止迁移。
        backup_before_table_rebuild(&self.conn)?;
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
                reasoning_summary TEXT, verbosity TEXT,
                auth_command TEXT, auth_args TEXT, auth_timeout_ms INTEGER,
                auth_refresh_interval_ms INTEGER, auth_cwd TEXT,
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
                opencode_api_mode, opencode_model_configs, created_at, updated_at,
                reasoning_summary, verbosity, auth_command, auth_args, auth_timeout_ms,
                auth_refresh_interval_ms, auth_cwd
            )
            SELECT id, name, provider, api_url, api_key, model_mapping, model, reasoning_effort,
                context_1m, target_app, models, wire_api, env_key, requires_openai_auth,
                service_tier, experimental_bearer_token, supports_standalone_web_search,
                aws_profile, aws_region, api_mode, max_tokens, api_keys_json, catalog_models,
                opencode_api_mode, opencode_model_configs, created_at, updated_at,
                reasoning_summary, verbosity, auth_command, auth_args, auth_timeout_ms,
                auth_refresh_interval_ms, auth_cwd
            FROM api_profiles;
            DROP TABLE api_profiles;
            ALTER TABLE api_profiles_no_thinking RENAME TO api_profiles;
            CREATE INDEX IF NOT EXISTS idx_profiles_name ON api_profiles(name);
            COMMIT;
            "#,
        )?;
        self.record_migration(id)
    }

    /// wire_api="chat" 系取值已被官方删除（2026-02，discussion #7782），
    /// 存量数据归一为 responses；写入路径本身已固定 responses，这里只修历史行。
    fn migrate_normalize_codex_wire_api(&self) -> Result<()> {
        let id = "2026-09-07-normalize-codex-wire-api";
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
            "UPDATE api_profiles SET wire_api = 'responses' WHERE wire_api IS NOT NULL AND lower(trim(wire_api)) IN ('chat', 'chat_completions', 'openai-chat')",
            [],
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

        // 重建整表前备份：备份失败必须中止迁移，否则旧数据无兜底。
        backup_before_table_rebuild(&self.conn)?;

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
                reasoning_summary TEXT,
                verbosity TEXT,
                auth_command TEXT,
                auth_args TEXT,
                auth_timeout_ms INTEGER,
                auth_refresh_interval_ms INTEGER,
                auth_cwd TEXT,
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
                 supports_standalone_web_search,aws_profile,aws_region,reasoning_summary,verbosity,
                 auth_command,auth_args,auth_timeout_ms,auth_refresh_interval_ms,auth_cwd,
                 api_mode,max_tokens,api_keys_json,
                 catalog_models,opencode_api_mode,opencode_model_configs)
            SELECT id,name,provider,api_url,api_key,model_mapping,created_at,updated_at,model,reasoning_effort,context_1m,target_app,models,
                 wire_api,env_key,requires_openai_auth,service_tier,experimental_bearer_token,
                 supports_standalone_web_search,aws_profile,aws_region,reasoning_summary,verbosity,
                 auth_command,auth_args,auth_timeout_ms,auth_refresh_interval_ms,auth_cwd,
                 api_mode,max_tokens,api_keys_json,
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

    // ========== API Profile 操作 ==========

    /// 添加 API Profile
    ///
    /// 返回 `AppError` 而非 `anyhow::Error`：调用方需要知道「参数不合法」这个
    /// 类别，而 `anyhow` 会把类别信息抹掉。未分类的内部失败仍可经
    /// `From<anyhow::Error>` 自动转换。
    pub fn add_profile(&self, profile: &ApiProfile) -> Result<i64, AppError> {
        let mut profile = profile.clone();
        if profile.target_app.is_none() {
            return Err(AppError::invalid_input(
                "API Profile 必须指定目标工具；暂不支持通用 Profile",
            ));
        }
        // 同名预检：直接 INSERT 撞 UNIQUE 会退化成 Io 的“数据库操作失败”。
        // 按类型给出 Conflict 需要先查一次；并发写由命令层 config_lock 串行化。
        if let Some(target) = profile.target_app {
            if self.profile_name_exists(&profile.name, target, None)? {
                return Err(AppError::conflict(format!(
                    "目标工具 {} 已存在同名 Profile：{}",
                    target.as_str(),
                    profile.name
                )));
            }
        }
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
        let auth_args_json = profile
            .codex
            .auth_args
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
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
            "INSERT INTO api_profiles (name, provider, api_url, api_key, model_mapping, model, reasoning_effort, context_1m, target_app, models, wire_api, env_key, requires_openai_auth, service_tier, experimental_bearer_token, supports_standalone_web_search, aws_profile, aws_region, reasoning_summary, verbosity, auth_command, auth_args, auth_timeout_ms, auth_refresh_interval_ms, auth_cwd, api_mode, max_tokens, api_keys_json, catalog_models, opencode_api_mode, opencode_model_configs, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32, ?33)",
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
                &profile.codex.wire_api,
                &profile.codex.env_key,
                profile.codex.requires_openai_auth.map(|b| b as i64),
                &profile.codex.service_tier,
                &profile.codex.experimental_bearer_token,
                profile.codex.supports_standalone_web_search.map(|b| b as i64),
                &profile.codex.aws_profile,
                &profile.codex.aws_region,
                &profile.codex.reasoning_summary,
                &profile.codex.verbosity,
                &profile.codex.auth_command,
                auth_args_json,
                profile.codex.auth_timeout_ms,
                profile.codex.auth_refresh_interval_ms,
                &profile.codex.auth_cwd,
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
        "supports_standalone_web_search, aws_profile, aws_region, reasoning_summary, verbosity, ",
        "auth_command, auth_args, auth_timeout_ms, auth_refresh_interval_ms, auth_cwd, ",
        "api_mode, max_tokens, ",
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
        let wire_api: Option<String> = row.get("wire_api")?;
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
        let auth_args_str: Option<String> = row.get("auth_args")?;
        let auth_args = auth_args_str
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(serde_json::from_str)
            .transpose()
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;

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
                reasoning_summary: row.get("reasoning_summary")?,
                verbosity: row.get("verbosity")?,
                wire_api,
                env_key: row.get("env_key")?,
                requires_openai_auth: requires_openai_auth.map(|v| v != 0),
                service_tier: row.get("service_tier")?,
                experimental_bearer_token: row.get("experimental_bearer_token")?,
                supports_standalone_web_search: supports_standalone_web_search.map(|v| v != 0),
                aws_profile: row.get("aws_profile")?,
                aws_region: row.get("aws_region")?,
                auth_command: row.get("auth_command")?,
                auth_args,
                auth_timeout_ms: row.get("auth_timeout_ms")?,
                auth_refresh_interval_ms: row.get("auth_refresh_interval_ms")?,
                auth_cwd: row.get("auth_cwd")?,
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
    /// GUI 导入流程使用的同名校验。
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

    /// Assign one legacy profile whose target_app is NULL to an explicit tool.
    /// The caller must make the ownership decision; the database never guesses.
    /// 把「无归属」的遗留 Profile 指派给某个工具。
    ///
    /// 三种失败在语义上完全不同，因此必须给出不同的 `ErrorKind`——这正是
    /// 本方法不能返回 `anyhow::Error` 的原因：上层要能区分「被删了」和
    /// 「状态冲突」，两者的处置方式不一样。
    pub fn assign_legacy_profile(
        &self,
        profile_id: i64,
        target: TargetApp,
    ) -> Result<(), AppError> {
        let (name, current_target): (String, Option<String>) = self
            .conn
            .query_row(
                "SELECT name, target_app FROM api_profiles WHERE id = ?1",
                params![profile_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .ok_or_else(|| AppError::not_found(format!("Profile id={profile_id} 不存在")))?;
        if current_target.is_some() {
            return Err(AppError::conflict(format!(
                "Profile id={profile_id} 已经归属明确工具"
            )));
        }
        if self.profile_name_exists(&name, target, None)? {
            return Err(AppError::conflict(format!(
                "目标工具 {} 已存在同名 Profile：{}",
                target.as_str(),
                name
            )));
        }
        self.conn.execute(
            "UPDATE api_profiles SET target_app = ?1, updated_at = ?2 WHERE id = ?3 AND target_app IS NULL",
            params![target.as_str(), chrono::Utc::now().timestamp(), profile_id],
        )?;
        Ok(())
    }

    /// Delete a legacy unassigned profile by id. Active rows are protected by
    /// the foreign key cascade, but a NULL-target row cannot be a normal active
    /// profile in the first place.
    pub fn delete_legacy_profile(&self, profile_id: i64) -> Result<bool, AppError> {
        let rows = self.conn.execute(
            "DELETE FROM api_profiles WHERE id = ?1 AND target_app IS NULL",
            params![profile_id],
        )?;
        Ok(rows > 0)
    }

    /// 更新 API Profile
    ///
    /// 按 `id` 定位记录（而非 name），因此**支持改名**。
    /// id 为空时回退到按旧 name 定位（理论上现有 profile 都带 id）。
    ///
    /// 与 `add_profile` 一致：返回 `AppError`，让调用方能区分「参数不合法」
    /// 与「数据库/磁盘故障」，而不是只拿到一段文本。
    pub fn update_profile(&self, profile: &ApiProfile) -> Result<(), AppError> {
        let mut profile = profile.clone();
        if profile.target_app.is_none() {
            return Err(AppError::invalid_input(
                "API Profile 必须指定目标工具；暂不支持通用 Profile",
            ));
        }
        profile.normalize_keys();
        // 改名撞车与 add 同理：UPDATE 撞 UNIQUE 会退化成 Io，这里先给 Conflict。
        // 无 id 时按 name 定位（不改名），不可能撞到别的行，跳过。
        if let (Some(target), Some(id)) = (profile.target_app, profile.id) {
            if self.profile_name_exists(&profile.name, target, Some(id))? {
                return Err(AppError::conflict(format!(
                    "目标工具 {} 已存在同名 Profile：{}",
                    target.as_str(),
                    profile.name
                )));
            }
        }
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
        let auth_args_json = profile
            .codex
            .auth_args
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
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
                     model_mapping = ?5, model = ?6, reasoning_effort = ?7, context_1m = ?8, target_app = ?9, models = ?10, wire_api = ?11, env_key = ?12, requires_openai_auth = ?13, service_tier = ?14, experimental_bearer_token = ?15, supports_standalone_web_search = ?16, aws_profile = ?17, aws_region = ?18, reasoning_summary = ?19, verbosity = ?20, auth_command = ?21, auth_args = ?22, auth_timeout_ms = ?23, auth_refresh_interval_ms = ?24, auth_cwd = ?25, api_mode = ?26, max_tokens = ?27, api_keys_json = ?28, catalog_models = ?29, opencode_api_mode = ?30, opencode_model_configs = ?31, updated_at = ?32 WHERE id = ?33",
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
                        &profile.codex.wire_api,
                        &profile.codex.env_key,
                        profile.codex.requires_openai_auth.map(|b| b as i64),
                        &profile.codex.service_tier,
                        &profile.codex.experimental_bearer_token,
                        profile.codex.supports_standalone_web_search.map(|b| b as i64),
                        &profile.codex.aws_profile,
                        &profile.codex.aws_region,
                        &profile.codex.reasoning_summary,
                        &profile.codex.verbosity,
                        &profile.codex.auth_command,
                        auth_args_json,
                        profile.codex.auth_timeout_ms,
                        profile.codex.auth_refresh_interval_ms,
                        &profile.codex.auth_cwd,
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
                // 无 id：按 (name, target_app) 定位，不改名。
                //
                // `target_app` 是必须的谓词：表的唯一约束是
                // `UNIQUE(name, target_app)`，**不同工具下同名是被允许的**。
                // 只按 name 更新会把所有同名档案一起改掉（跨工具误写）。
                let Some(target_app) = profile.target_app.as_ref() else {
                    return Err(AppError::invalid_input(
                        "更新 Profile 时若未提供 id，必须同时提供 target_app 以唯一定位",
                    ));
                };
                self.conn.execute(
                    "UPDATE api_profiles SET provider = ?1, api_url = ?2, api_key = ?3,
                     model_mapping = ?4, model = ?5, reasoning_effort = ?6, context_1m = ?7, target_app = ?8, models = ?9, wire_api = ?10, env_key = ?11, requires_openai_auth = ?12, service_tier = ?13, experimental_bearer_token = ?14, supports_standalone_web_search = ?15, aws_profile = ?16, aws_region = ?17, reasoning_summary = ?18, verbosity = ?19, auth_command = ?20, auth_args = ?21, auth_timeout_ms = ?22, auth_refresh_interval_ms = ?23, auth_cwd = ?24, api_mode = ?25, max_tokens = ?26, api_keys_json = ?27, catalog_models = ?28, opencode_api_mode = ?29, opencode_model_configs = ?30, updated_at = ?31 WHERE name = ?32 AND target_app = ?33",
                    params![
                        &profile.provider,
                        &profile.api_url,
                        &profile.api_key,
                        model_mapping_json,
                        &profile.model,
                        &profile.codex.reasoning_effort,
                        profile.context_1m.map(|b| b as i64),
                        target_app.as_str(),
                        models_json,
                        &profile.codex.wire_api,
                        &profile.codex.env_key,
                        profile.codex.requires_openai_auth.map(|b| b as i64),
                        &profile.codex.service_tier,
                        &profile.codex.experimental_bearer_token,
                        profile.codex.supports_standalone_web_search.map(|b| b as i64),
                        &profile.codex.aws_profile,
                        &profile.codex.aws_region,
                        &profile.codex.reasoning_summary,
                        &profile.codex.verbosity,
                        &profile.codex.auth_command,
                        auth_args_json,
                        profile.codex.auth_timeout_ms,
                        profile.codex.auth_refresh_interval_ms,
                        &profile.codex.auth_cwd,
                        api_mode,
                        max_tokens,
                        api_keys_json,
                        catalog_models_json,
                        opencode_api_mode,
                        opencode_model_configs_json,
                        now,
                        &profile.name,
                        target_app.as_str()
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
    ) -> Result<(), AppError> {
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
    pub fn delete_shared_config(&self, target_app: TargetApp) -> Result<(), AppError> {
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

    /// Record provider ownership only once. Existing records are intentionally
    /// preserved because a later switch cannot reliably distinguish a provider
    /// created manually from one created by an older Helio version.
    pub fn record_provider_ownership_if_missing(
        &self,
        target_app: TargetApp,
        provider_id: &str,
        managed_by_helio: bool,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO provider_ownership
             (target_app, provider_id, managed_by_helio, updated_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                target_app.as_str(),
                provider_id.trim().to_lowercase(),
                managed_by_helio as i64,
                chrono::Utc::now().timestamp()
            ],
        )?;
        Ok(())
    }

    pub fn provider_managed_by_helio(
        &self,
        target_app: TargetApp,
        provider_id: &str,
    ) -> Result<Option<bool>> {
        self.conn
            .query_row(
                "SELECT managed_by_helio FROM provider_ownership
                 WHERE target_app = ?1 AND provider_id = ?2",
                params![target_app.as_str(), provider_id.trim().to_lowercase()],
                |row| row.get::<_, i64>(0).map(|value| value != 0),
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn clear_provider_ownership(&self, target_app: TargetApp, provider_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM provider_ownership WHERE target_app = ?1 AND provider_id = ?2",
            params![target_app.as_str(), provider_id.trim().to_lowercase()],
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
