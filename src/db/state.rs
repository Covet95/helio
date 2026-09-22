//! 共享状态：`shared_configs` / OpenCode 受管模型 / provider 归属 / active profile。
//!
//! 这些是「跨 Profile 的持久状态」——不归属任何单个档案，而是记录
//! 「当前哪个档案生效」「某工具的共享配置是什么」「哪些 provider 是 Helio 建的」。
//!
//! 拆自 `db/mod.rs`；`Database` 的字段仍由父模块持有，子模块可直接访问。

use crate::error::AppError;
use crate::models::{
    ActiveProfile, ApiProfile, OpenCodeManagedModelState, SharedConfig, TargetApp,
};
use anyhow::Result;
use rusqlite::{params, OptionalExtension};

use super::Database;

impl Database {
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
