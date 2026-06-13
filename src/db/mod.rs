use crate::models::{ActiveProfile, ApiProfile, SharedConfig, TargetApp};
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

pub struct Database {
    conn: Connection,
}

impl Database {
    /// 打开或创建数据库
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let conn = Connection::open(path)?;
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
                name TEXT NOT NULL UNIQUE,
                provider TEXT NOT NULL,
                api_url TEXT NOT NULL,
                api_key TEXT NOT NULL,
                model_mapping TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
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
            "#,
        )?;
        Ok(())
    }

    // ========== API Profile 操作 ==========

    /// 添加 API Profile
    pub fn add_profile(&self, profile: &ApiProfile) -> Result<i64> {
        let now = chrono::Utc::now().timestamp();
        let model_mapping_json = profile
            .model_mapping
            .as_ref()
            .map(|m| serde_json::to_string(m))
            .transpose()?;

        self.conn.execute(
            "INSERT INTO api_profiles (name, provider, api_url, api_key, model_mapping, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                &profile.name,
                &profile.provider,
                &profile.api_url,
                &profile.api_key,
                model_mapping_json,
                now,
                now
            ],
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    /// 根据名称获取 API Profile
    pub fn get_profile_by_name(&self, name: &str) -> Result<ApiProfile> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, provider, api_url, api_key, model_mapping, created_at, updated_at
             FROM api_profiles WHERE name = ?1",
        )?;

        let profile = stmt.query_row(params![name], |row| {
            let model_mapping_str: Option<String> = row.get(5)?;
            let model_mapping = model_mapping_str
                .as_deref()
                .map(|s| serde_json::from_str(s))
                .transpose()
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;

            Ok(ApiProfile {
                id: Some(row.get(0)?),
                name: row.get(1)?,
                provider: row.get(2)?,
                api_url: row.get(3)?,
                api_key: row.get(4)?,
                model_mapping,
                created_at: Some(row.get(6)?),
                updated_at: Some(row.get(7)?),
            })
        })?;

        Ok(profile)
    }

    /// 列出所有 API Profiles
    pub fn list_profiles(&self) -> Result<Vec<ApiProfile>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, provider, api_url, api_key, model_mapping, created_at, updated_at
             FROM api_profiles ORDER BY name",
        )?;

        let profiles = stmt
            .query_map([], |row| {
                let model_mapping_str: Option<String> = row.get(5)?;
                let model_mapping = model_mapping_str
                    .as_deref()
                    .map(|s| serde_json::from_str(s))
                    .transpose()
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;

                Ok(ApiProfile {
                    id: Some(row.get(0)?),
                    name: row.get(1)?,
                    provider: row.get(2)?,
                    api_url: row.get(3)?,
                    api_key: row.get(4)?,
                    model_mapping,
                    created_at: Some(row.get(6)?),
                    updated_at: Some(row.get(7)?),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(profiles)
    }

    /// 更新 API Profile
    pub fn update_profile(&self, profile: &ApiProfile) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        let model_mapping_json = profile
            .model_mapping
            .as_ref()
            .map(|m| serde_json::to_string(m))
            .transpose()?;

        self.conn.execute(
            "UPDATE api_profiles SET provider = ?1, api_url = ?2, api_key = ?3,
             model_mapping = ?4, updated_at = ?5 WHERE name = ?6",
            params![
                &profile.provider,
                &profile.api_url,
                &profile.api_key,
                model_mapping_json,
                now,
                &profile.name
            ],
        )?;

        Ok(())
    }

    /// 删除 API Profile
    pub fn delete_profile(&self, name: &str) -> Result<bool> {
        let rows = self
            .conn
            .execute("DELETE FROM api_profiles WHERE name = ?1", params![name])?;
        Ok(rows > 0)
    }

    // ========== 共享配置操作 ==========

    /// 保存共享配置
    pub fn save_shared_config(&self, target_app: TargetApp, config: serde_json::Value) -> Result<()> {
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
            .prepare("SELECT target_app, profile_id FROM active_profiles WHERE target_app = ?1")?;

        let result = stmt
            .query_row(params![target_app.as_str()], |row| {
                Ok(ActiveProfile {
                    target_app,
                    profile_id: row.get(1)?,
                })
            })
            .optional()?;

        Ok(result)
    }

    /// 获取活动 Profile 的完整信息
    pub fn get_active_profile_full(&self, target_app: TargetApp) -> Result<Option<ApiProfile>> {
        let active = self.get_active_profile(target_app)?;

        if let Some(active) = active {
            let profiles = self.list_profiles()?;
            Ok(profiles.into_iter().find(|p| p.id == Some(active.profile_id)))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_database_operations() -> Result<()> {
        let db = Database::open(":memory:")?;

        // 测试添加 Profile
        let profile = ApiProfile::new(
            "test-profile".to_string(),
            "anthropic".to_string(),
            "https://api.anthropic.com".to_string(),
            "sk-test-key".to_string(),
            Some(HashMap::from([
                ("opus".to_string(), "claude-opus-4".to_string()),
            ])),
        );

        let id = db.add_profile(&profile)?;
        assert!(id > 0);

        // 测试获取 Profile
        let retrieved = db.get_profile_by_name("test-profile")?;
        assert_eq!(retrieved.name, "test-profile");
        assert_eq!(retrieved.api_url, "https://api.anthropic.com");

        // 测试列出 Profiles
        let profiles = db.list_profiles()?;
        assert_eq!(profiles.len(), 1);

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
}
