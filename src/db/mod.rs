use crate::models::{
    ActiveProfile, ApiProfile, ClaudeProfileFields, CodexProfileFields, HermesProfileFields,
    OpenClawProfileFields, OpenCodeProfileFields, SharedConfig, TargetApp,
};
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::utils::secure_fs::{copy_private, ensure_private_dir, ensure_private_file};

pub struct Database {
    conn: Connection,
}

impl Database {
    /// 打开或创建数据库
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        if path != Path::new(":memory:") {
            if let Some(parent) = path.parent() {
                ensure_private_dir(parent)?;
            }
        }
        let conn = Connection::open(path)?;
        if path != Path::new(":memory:") {
            ensure_private_file(path)?;
        }
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
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

            CREATE TABLE IF NOT EXISTS shared_configs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                target_app TEXT NOT NULL UNIQUE,
                config_json TEXT NOT NULL,
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
        self.conn.execute(
            "DELETE FROM shared_configs WHERE target_app = 'gemini'",
            [],
        )?;
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
            "ALTER TABLE api_profiles ADD COLUMN model_thinking_enabled INTEGER",
            "ALTER TABLE api_profiles ADD COLUMN service_tier TEXT",
            "ALTER TABLE api_profiles ADD COLUMN experimental_bearer_token TEXT",
            "ALTER TABLE api_profiles ADD COLUMN api_mode TEXT",
            "ALTER TABLE api_profiles ADD COLUMN max_tokens INTEGER",
            "ALTER TABLE api_profiles ADD COLUMN api_keys_json TEXT",
            "ALTER TABLE api_profiles ADD COLUMN catalog_models TEXT",
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
        self.conn.execute_batch("PRAGMA foreign_keys=OFF;")?;
        self.conn.execute_batch(r#"
            BEGIN;
            CREATE TABLE api_profiles_no_effort (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL, provider TEXT NOT NULL, api_url TEXT NOT NULL, api_key TEXT NOT NULL,
                model_mapping TEXT, model TEXT, reasoning_effort TEXT, context_1m INTEGER,
                target_app TEXT, models TEXT, wire_api TEXT, env_key TEXT, requires_openai_auth INTEGER,
                model_thinking_enabled INTEGER, service_tier TEXT, experimental_bearer_token TEXT,
                api_mode TEXT, max_tokens INTEGER, api_keys_json TEXT, catalog_models TEXT,
                created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
                UNIQUE(name, target_app)
            );
            INSERT INTO api_profiles_no_effort (
                id, name, provider, api_url, api_key, model_mapping, model, reasoning_effort,
                context_1m, target_app, models, wire_api, env_key, requires_openai_auth,
                model_thinking_enabled, service_tier, experimental_bearer_token, api_mode, max_tokens,
                api_keys_json, catalog_models, created_at, updated_at
            )
            SELECT id, name, provider, api_url, api_key, model_mapping, model, reasoning_effort,
                context_1m, target_app, models, wire_api, env_key, requires_openai_auth,
                model_thinking_enabled, service_tier, experimental_bearer_token,
                api_mode, max_tokens, api_keys_json, catalog_models, created_at, updated_at
            FROM api_profiles;
            DROP TABLE api_profiles;
            ALTER TABLE api_profiles_no_effort RENAME TO api_profiles;
            CREATE INDEX IF NOT EXISTS idx_profiles_name ON api_profiles(name);
            COMMIT;
        "#)?;
        self.conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        Ok(())
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
                let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
                let backup = format!("{db_path}.premigrate.{ts}.sqlite");
                let _ = copy_private(Path::new(db_path), Path::new(&backup));
            }
        }

        // 重建表:新表用复合唯一。注意去 -cc 后缀(仅 target_app 非空、去后缀后同工具不冲突)。
        // 整个重建流程包在单个事务中以保证原子性(防止 DROP 与 RENAME 之间进程被杀留下孤表)。
        //
        // 关键:active_profiles 有 FOREIGN KEY ... REFERENCES api_profiles(id)。开启外键检查时
        // `DROP TABLE api_profiles` 会触发 FOREIGN KEY constraint failed 导致整个事务回滚。
        // 按 SQLite 官方安全重建表流程,重建期间必须关闭外键检查;
        // 而 `PRAGMA foreign_keys` 在事务内是 no-op,必须在 BEGIN 之前设置、COMMIT 之后恢复。
        self.conn.execute_batch("PRAGMA foreign_keys=OFF;")?;
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
                model_thinking_enabled INTEGER,
                service_tier TEXT,
                experimental_bearer_token TEXT,
                api_mode TEXT,
                max_tokens INTEGER,
                api_keys_json TEXT,
                catalog_models TEXT,
                UNIQUE(name, target_app)
            );

            INSERT INTO api_profiles_new
                (id,name,provider,api_url,api_key,model_mapping,created_at,updated_at,model,reasoning_effort,context_1m,target_app,models,
                 wire_api,env_key,requires_openai_auth,model_thinking_enabled,service_tier,experimental_bearer_token,api_mode,max_tokens,api_keys_json,catalog_models)
            SELECT id,name,provider,api_url,api_key,model_mapping,created_at,updated_at,model,reasoning_effort,context_1m,target_app,models,
                 wire_api,env_key,requires_openai_auth,model_thinking_enabled,service_tier,experimental_bearer_token,api_mode,max_tokens,api_keys_json,catalog_models
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
        self.conn.execute_batch("PRAGMA foreign_keys=ON;")?;

        Ok(())
    }

    /// Validates an imported database in a private staging path, then atomically replaces a
    /// closed live database. Callers must drop the old `Database` connection before this method.
    pub fn replace_file_from_import(
        input_path: &Path,
        live_path: &Path,
    ) -> Result<Option<PathBuf>> {
        if !input_path.exists() {
            anyhow::bail!("Input database does not exist: {}", input_path.display());
        }
        let parent = live_path.parent().ok_or_else(|| {
            anyhow::anyhow!("Database path has no parent: {}", live_path.display())
        })?;
        ensure_private_dir(parent)?;

        let staging_path = parent.join(format!(".db.import.{}.sqlite", Uuid::new_v4()));
        copy_private(input_path, &staging_path)?;
        let validation = match Database::open(&staging_path) {
            Ok(database) => database,
            Err(error) => {
                let _ = fs::remove_file(&staging_path);
                return Err(error);
            }
        };
        drop(validation);

        let backup_path = if live_path.exists() {
            let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
            let backup = live_path.with_file_name(format!("db.backup.{timestamp}.sqlite"));
            fs::rename(live_path, &backup)?;
            ensure_private_file(&backup)?;
            Some(backup)
        } else {
            None
        };

        if let Err(error) = fs::rename(&staging_path, live_path) {
            if let Some(backup) = backup_path.as_ref() {
                let _ = fs::rename(backup, live_path);
            }
            return Err(error.into());
        }
        ensure_private_file(live_path)?;
        Ok(backup_path)
    }

    pub fn restore_replaced_file(live_path: &Path, backup_path: &Path) -> Result<()> {
        if live_path.exists() {
            fs::remove_file(live_path)?;
        }
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
        let api_keys_json = Self::serialize_api_keys_json(&profile)?;
        let (api_mode, max_tokens) = match profile.target_app {
            Some(TargetApp::OpenClaw) => (
                profile.openclaw.api_mode.as_ref(),
                profile.openclaw.max_tokens,
            ),
            Some(TargetApp::Hermes) => (profile.hermes.api_mode.as_ref(), None),
            _ => (
                profile
                    .hermes
                    .api_mode
                    .as_ref()
                    .or(profile.openclaw.api_mode.as_ref()),
                profile.openclaw.max_tokens,
            ),
        };

        self.conn.execute(
            "INSERT INTO api_profiles (name, provider, api_url, api_key, model_mapping, model, reasoning_effort, context_1m, target_app, models, wire_api, env_key, requires_openai_auth, model_thinking_enabled, service_tier, experimental_bearer_token, api_mode, max_tokens, api_keys_json, catalog_models, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)",
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
                profile.codex.model_thinking_enabled.map(|b| b as i64),
                &profile.codex.service_tier,
                &profile.codex.experimental_bearer_token,
                api_mode,
                max_tokens,
                api_keys_json,
                catalog_models_json,
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
        "wire_api, env_key, requires_openai_auth, model_thinking_enabled, service_tier, ",
        "experimental_bearer_token, api_mode, max_tokens, api_keys_json, catalog_models"
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
        let model_thinking_enabled: Option<i64> = row.get("model_thinking_enabled")?;
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
                model_thinking_enabled: model_thinking_enabled.map(|v| v != 0),
                service_tier: row.get("service_tier")?,
                experimental_bearer_token: row.get("experimental_bearer_token")?,
                catalog_models,
            },
            opencode: OpenCodeProfileFields { models },
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
        let api_keys_json = Self::serialize_api_keys_json(&profile)?;
        let (api_mode, max_tokens) = match profile.target_app {
            Some(TargetApp::OpenClaw) => (
                profile.openclaw.api_mode.as_ref(),
                profile.openclaw.max_tokens,
            ),
            Some(TargetApp::Hermes) => (profile.hermes.api_mode.as_ref(), None),
            _ => (
                profile
                    .hermes
                    .api_mode
                    .as_ref()
                    .or(profile.openclaw.api_mode.as_ref()),
                profile.openclaw.max_tokens,
            ),
        };

        match profile.id {
            Some(id) => {
                self.conn.execute(
                    "UPDATE api_profiles SET name = ?1, provider = ?2, api_url = ?3, api_key = ?4,
                     model_mapping = ?5, model = ?6, reasoning_effort = ?7, context_1m = ?8, target_app = ?9, models = ?10, wire_api = ?11, env_key = ?12, requires_openai_auth = ?13, model_thinking_enabled = ?14, service_tier = ?15, experimental_bearer_token = ?16, api_mode = ?17, max_tokens = ?18, api_keys_json = ?19, catalog_models = ?20, updated_at = ?21 WHERE id = ?22",
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
                        profile.codex.model_thinking_enabled.map(|b| b as i64),
                        &profile.codex.service_tier,
                        &profile.codex.experimental_bearer_token,
                        api_mode,
                        max_tokens,
                        api_keys_json,
                        catalog_models_json,
                        now,
                        id
                    ],
                )?;
            }
            None => {
                // 无 id：按 name 定位，不改名
                self.conn.execute(
                    "UPDATE api_profiles SET provider = ?1, api_url = ?2, api_key = ?3,
                     model_mapping = ?4, model = ?5, reasoning_effort = ?6, context_1m = ?7, target_app = ?8, models = ?9, wire_api = ?10, env_key = ?11, requires_openai_auth = ?12, model_thinking_enabled = ?13, service_tier = ?14, experimental_bearer_token = ?15, api_mode = ?16, max_tokens = ?17, api_keys_json = ?18, catalog_models = ?19, updated_at = ?20 WHERE name = ?21",
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
                        profile.codex.model_thinking_enabled.map(|b| b as i64),
                        &profile.codex.service_tier,
                        &profile.codex.experimental_bearer_token,
                        api_mode,
                        max_tokens,
                        api_keys_json,
                        catalog_models_json,
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

    // ========== 活动 Profile 操作 ==========

    /// 设置活动 Profile
    pub fn set_active_profile(&self, target_app: TargetApp, profile_id: i64) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO active_profiles (target_app, profile_id) VALUES (?1, ?2)",
            params![target_app.as_str(), profile_id],
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

    #[test]
    fn test_fresh_schema_has_no_effort_level() -> Result<()> {
        let db = Database::open(":memory:")?;
        let mut stmt = db.conn.prepare("PRAGMA table_info(api_profiles)")?;
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get(1))?
            .collect::<Result<Vec<_>, _>>()?;
        assert!(cols.contains(&"experimental_bearer_token".into()));
        assert!(!cols.contains(&"model_effort_level".into()));
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
                model_thinking_enabled: Some(true),
                catalog_models: Some(vec![crate::models::CodexCatalogModel {
                    slug: "gpt-5.6-sol".into(),
                    display_name: Some("GPT-5.6 Sol".into()),
                    context_window: Some(400_000),
                    supports_reasoning: Some(true),
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
        let cm = got.codex.catalog_models.as_ref().unwrap();
        assert_eq!(cm.len(), 1);
        assert_eq!(cm[0].slug, "gpt-5.6-sol");
        assert_eq!(cm[0].display_name.as_deref(), Some("GPT-5.6 Sol"));
        assert_eq!(cm[0].context_window, Some(400_000));
        assert_eq!(cm[0].supports_reasoning, Some(true));
        assert_eq!(cm[0].supports_images, Some(true));
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
        let _ = std::fs::remove_dir_all(&dir);
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
