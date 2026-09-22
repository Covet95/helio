//! Profile CRUD：增删改查 + 遗留档案认领。
//!
//! 拆自 `db/mod.rs`。表的唯一约束是 `UNIQUE(name, target_app)`——
//! **不同工具下同名是被允许的**，因此所有定位都必须带 `target_app`。

use crate::error::AppError;
use crate::models::{
    ApiProfile, ClaudeProfileFields, CodexProfileFields, HermesProfileFields,
    OpenClawProfileFields, OpenCodeProfileFields, TargetApp,
};
use anyhow::Result;
use rusqlite::{params, OptionalExtension};

use super::Database;

impl Database {
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
    pub(crate) const PROFILE_SELECT: &'static str = concat!(
        "id, name, provider, api_url, api_key, model_mapping, model, ",
        "reasoning_effort, context_1m, created_at, updated_at, target_app, models, ",
        "wire_api, env_key, requires_openai_auth, service_tier, experimental_bearer_token, ",
        "supports_standalone_web_search, aws_profile, aws_region, reasoning_summary, verbosity, ",
        "auth_command, auth_args, auth_timeout_ms, auth_refresh_interval_ms, auth_cwd, ",
        "api_mode, max_tokens, ",
        "api_keys_json, catalog_models, opencode_api_mode, opencode_model_configs"
    );

    pub(crate) fn row_to_profile(row: &rusqlite::Row) -> rusqlite::Result<ApiProfile> {
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
}

/// 宽表字段同步守卫。
///
/// `api_profiles` 有 33 列，加一个工具字段要同时改**五处**：结构体、CREATE TABLE、
/// ALTER 迁移列表、INSERT/UPDATE 的列与占位符、`row_to_profile` 的读取。漏掉任何
/// 一处都是**静默**的——编译通过、大多数测试也通过，只是那个字段存不进去或读不出来。
///
/// 这里把三处能自动比对的钉在一起：
/// - INSERT 的列名集合 == `row_to_profile` 实际读取的列名集合；
/// - 两者都 == 建表后真实的列集合（减去已知的「非 profile 字段」）。
///
/// 新增字段时若只改了结构体没改 SQL，这个测试会失败并指出差在哪一列。
#[cfg(test)]
mod schema_sync_tests {
    use super::Database;
    use std::collections::BTreeSet;

    /// INSERT 语句里的列名（从源码常量解析，避免复制一份清单再手工同步）。
    fn insert_columns() -> BTreeSet<String> {
        const SQL: &str = include_str!("profiles.rs");
        let start = SQL
            .find("INSERT INTO api_profiles (")
            .expect("找不到 INSERT 语句——若已重构请同步更新本测试");
        let rest = &SQL[start + "INSERT INTO api_profiles (".len()..];
        let end = rest.find(')').expect("INSERT 列清单没有右括号");
        rest[..end]
            .split(',')
            .map(|c| c.trim().to_string())
            .collect()
    }

    /// `row_to_profile` 实际读取的列名。
    fn mapped_columns() -> BTreeSet<String> {
        const SQL: &str = include_str!("profiles.rs");
        let start = SQL
            .find("fn row_to_profile")
            .expect("找不到 row_to_profile");
        // 函数体到下一个顶层 `}` 为止（粗略但够用：函数内没有裸 `\n    }`）
        let body_end = SQL[start..]
            .find("\n    }\n")
            .map(|i| start + i)
            .unwrap_or(SQL.len());
        let body = &SQL[start..body_end];

        let mut cols = BTreeSet::new();
        let mut rest = body;
        while let Some(i) = rest.find("row.get(\"") {
            let after = &rest[i + "row.get(\"".len()..];
            let Some(j) = after.find('"') else { break };
            cols.insert(after[..j].to_string());
            rest = &after[j..];
        }
        cols
    }

    /// 建表后的真实列集合。
    fn table_columns() -> BTreeSet<String> {
        let db = Database::open(":memory:").expect("开内存库");
        let mut stmt = db
            .conn
            .prepare("PRAGMA table_info(api_profiles)")
            .expect("PRAGMA");
        let cols: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .expect("query")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect");
        cols.into_iter().collect()
    }

    /// 不在 INSERT 清单里、也不算 profile 字段的列。
    ///
    /// `id` 由 SQLite 自增分配；其余是内部/派生列，不该出现在 INSERT 里。
    const NON_PROFILE_COLUMNS: &[&str] = &[
        "id",
        "api_keys_json",
        "catalog_models",
        "models",
        "model_mapping",
    ];

    #[test]
    fn insert_columns_match_row_mapping() {
        let inserted = insert_columns();
        // `id` 由 SQLite 自增分配，只在读取侧出现——不算不一致。
        let mapped: BTreeSet<String> = mapped_columns()
            .difference(&BTreeSet::from(["id".to_string()]))
            .cloned()
            .collect();

        let only_insert = inserted.difference(&mapped).cloned().collect::<Vec<_>>();
        let only_mapped = mapped.difference(&inserted).cloned().collect::<Vec<_>>();

        assert!(
            only_insert.is_empty() && only_mapped.is_empty(),
            "INSERT 与 row_to_profile 的列不一致——存进去的读不出来，或反之。\n\
             只在 INSERT 里: {only_insert:?}\n\
             只在读取里: {only_mapped:?}"
        );
    }

    #[test]
    fn every_table_column_is_accounted_for() {
        let table = table_columns();
        let known: BTreeSet<String> = insert_columns()
            .union(&mapped_columns())
            .cloned()
            .chain(NON_PROFILE_COLUMNS.iter().map(|s| s.to_string()))
            .collect();

        let unaccounted = table.difference(&known).cloned().collect::<Vec<_>>();
        assert!(
            unaccounted.is_empty(),
            "表里有列既不在 INSERT 也不在 row_to_profile 中，也不在已知例外清单里：\n\
             {unaccounted:?}\n\
             新增字段时请同步 INSERT 与 row_to_profile（或加进 NON_PROFILE_COLUMNS 并说明原因）。"
        );
    }

    #[test]
    fn guard_actually_sees_the_columns() {
        // 守卫自身不能是空转的：确认解析确实拿到了内容。
        let inserted = insert_columns();
        assert!(
            inserted.len() > 25,
            "解析出的 INSERT 列太少，守卫可能已失效: {inserted:?}"
        );
        assert!(inserted.contains("name"), "应包含 name");
        assert!(mapped_columns().contains("provider"), "映射应包含 provider");
    }
}
