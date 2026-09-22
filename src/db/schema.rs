//! 建表与迁移。
//!
//! `init_schema` 在每次 `Database::open` 时执行：建表（`IF NOT EXISTS`）后
//! 依次跑各迁移。迁移必须**幂等**——靠三套判据实现：
//!
//! 1. `schema_migrations` 账本（记录已跑过的迁移 id）；
//! 2. 列存在性探测（`PRAGMA table_info`）；
//! 3. DDL 尝试 + 忽略「列已存在」错误。
//!
//! 其中「重建整表」的迁移会 `DROP TABLE api_profiles`——这张表装着全部
//! 明文 API key，因此**必须先备份**（见 `backup_before_table_rebuild`）。

use anyhow::Result;
use rusqlite::{params, OptionalExtension};

use super::{backup_before_table_rebuild, Database, ForeignKeysGuard};

impl Database {
    /// 初始化数据库表结构
    pub(crate) fn init_schema(&self) -> Result<()> {
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
    pub(crate) fn migrate_drop_gemini_target(&self) -> Result<()> {
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

    pub(crate) fn ensure_current_profile_columns(&self) -> Result<()> {
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

    /// 加列；列已存在则视为已完成（迁移必须可重入）。
    ///
    /// 不用「执行失败后匹配 `duplicate column name` 文案」的写法：SQLite 对
    /// 重复列只报通用的 `SQLITE_ERROR`，没有独立的错误码，只能靠措辞判断——
    /// 而措辞随版本/本地化变化，匹配不上就会把「已存在」误判成真错误，
    /// 让整个迁移失败。改为先查 `PRAGMA table_info` 再决定，判断依据是结构
    /// 而不是文案（与 `migrate_drop_model_effort_level` 的做法一致）。
    fn try_add_column(&self, ddl: &str) -> Result<()> {
        let column = ddl
            .rsplit(" ADD COLUMN ")
            .next()
            .and_then(|rest| rest.split_whitespace().next())
            .ok_or_else(|| anyhow::anyhow!("无法从 DDL 解析列名：{ddl}"))?;
        if self.api_profiles_has_column(column)? {
            return Ok(());
        }
        self.conn.execute(ddl, [])?;
        Ok(())
    }

    /// `api_profiles` 是否已有该列。
    fn api_profiles_has_column(&self, column: &str) -> Result<bool> {
        let mut stmt = self.conn.prepare("PRAGMA table_info(api_profiles)")?;
        let found: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(found.iter().any(|name| name == column))
    }

    pub(crate) fn migrate_drop_model_effort_level(&self) -> Result<()> {
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

    pub(crate) fn migrate_drop_model_thinking_enabled(&self) -> Result<()> {
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
    pub(crate) fn migrate_normalize_codex_wire_api(&self) -> Result<()> {
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
    pub(crate) fn migrate_composite_unique(&self) -> Result<()> {
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
}
