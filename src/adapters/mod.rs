use crate::models::{ApiProfile, TargetApp};
use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

use crate::utils::secure_fs::atomic_write_private;

#[derive(Debug)]
pub struct FileSnapshot {
    path: PathBuf,
    contents: Option<Vec<u8>>,
}

#[derive(Debug)]
pub struct ProfileApplicationResult {
    pub backup_path: Option<PathBuf>,
    pub config_path: PathBuf,
}

/// 配置适配器 trait
pub trait ConfigAdapter {
    /// 配置文件路径
    fn config_path(&self) -> PathBuf;

    /// 读取当前配置
    fn read_config(&self) -> Result<serde_json::Value>;

    /// 读取 MCP servers 的原始 JSON（mcpServers / mcp_servers / mcp）。
    fn read_mcp_servers_raw(&self) -> Result<Option<serde_json::Value>> {
        let config = self.read_config()?;
        Ok(config
            .get("mcpServers")
            .or_else(|| config.get("mcp_servers"))
            .or_else(|| config.get("mcp"))
            .cloned())
    }

    /// 提取共享配置（排除 API 信息）
    fn extract_shared_config(&self, config: &serde_json::Value) -> serde_json::Value;

    /// 合并 API Profile 和共享配置
    fn merge_config(
        &self,
        api_profile: &ApiProfile,
        shared_config: &serde_json::Value,
    ) -> serde_json::Value;

    /// Validate target-specific profile fields before creating a backup or writing files.
    fn validate_profile(&self, _api_profile: &ApiProfile) -> Result<()> {
        Ok(())
    }

    /// 写盘前校验合并结果。默认无操作；实现返回 Err 时整个切换事务回滚，
    /// 防止把语义残缺的配置静默写盘。
    fn verify_merged_config(
        &self,
        _merged: &serde_json::Value,
        _api_profile: &ApiProfile,
    ) -> Result<()> {
        Ok(())
    }

    /// 写入主配置文件之外的辅助文件（如 Claude 的 ~/.claude.json 里的 MCP）。
    /// 默认无操作；实现出错时整个切换事务回滚。
    fn apply_auxiliary_config(&self, _shared_config: &serde_json::Value) -> Result<()> {
        Ok(())
    }

    /// 原子写入配置
    fn write_config(&self, config: &serde_json::Value) -> Result<()>;

    /// 三路合并写入：以 live 文件为基底，摘掉 `previous_managed` 的覆盖，
    /// 再叠加 `next_managed`。**保留用户手写的未受管字段与键序。**
    ///
    /// 默认实现对**所有适配器**生效，且**复用各适配器自己的 `write_config`**
    /// 做序列化——不另立一套写盘约定。策略按格式分派：
    ///
    /// - **TOML**：文本级合并（`toml_edit`）。TOML 有注释，只有文本级合并
    ///   才能保住它们，因此这条路径自行序列化并写盘。
    /// - **JSON / YAML**：值级合并后交给 [`Self::write_config`]。JSON 无注释，
    ///   值级合并已能保住全部用户内容与键序，而委托给适配器可以保留它自己的
    ///   缩进/排版约定。
    ///
    /// 解析失败时退回整体写入受管内容——「切换必须能完成」是硬需求，
    /// 保真只是优化，不能因用户手改坏了文件就让切换卡死。
    ///
    /// `previous_managed` 为 `None` 表示首次切换（无可摘除的历史），此时
    /// 只叠加、不摘除，不会删除 live 中的任何用户内容。
    fn write_config_merged(
        &self,
        next_managed: &serde_json::Value,
        previous_managed: Option<&serde_json::Value>,
    ) -> Result<()> {
        let path = self.config_path();
        let format = self.config_format();

        let live_text = if path.exists() {
            match fs::read_to_string(&path) {
                Ok(text) => text,
                Err(error) => {
                    // 读不出来（权限/编码）——退回整体写入，不让切换卡死。
                    tracing::warn!("读取 {} 失败，整体写入受管配置：{error:#}", path.display());
                    return self.write_config(next_managed);
                }
            }
        } else {
            String::new()
        };

        if live_text.trim().is_empty() {
            // 无 live 文件：直接落盘受管内容，无需合并。
            return self.write_config(next_managed);
        }

        let live_value = match crate::doc::parse(format, &live_text) {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(
                    "{} 无法解析为 {}，本次切换将整体写入受管配置：{error:#}",
                    path.display(),
                    format!("{format:?}"),
                );
                return self.write_config(next_managed);
            }
        };

        // TOML 与 YAML 都有注释，走文本级合并才能保住它们——这条路径自行
        // 序列化并写盘。JSON 无注释概念，值级合并后委托给适配器自己的
        // `write_config`，以保留它各自的缩进/排版约定。
        match format {
            crate::doc::DocFormat::Toml | crate::doc::DocFormat::Yaml => {
                if let Some(parent) = path.parent() {
                    crate::utils::secure_fs::ensure_private_dir(parent)
                        .context("Failed to create config directory")?;
                }
                let content =
                    crate::doc::merge_document(format, &live_text, previous_managed, next_managed)
                        .with_context(|| format!("Failed to merge {}", path.display()))?;
                crate::utils::secure_fs::atomic_write_private(&path, content.as_bytes())
                    .with_context(|| format!("Failed to write {}", path.display()))?;
                Ok(())
            }
            crate::doc::DocFormat::Json => {
                let merged =
                    crate::doc::merge_three_way(&live_value, previous_managed, next_managed);
                self.write_config(&merged)
            }
        }
    }

    /// 主配置文件的格式。决定保真合并走哪条路径。
    ///
    /// 默认按扩展名推断；扩展名不标准时覆盖本方法。
    fn config_format(&self) -> crate::doc::DocFormat {
        crate::doc::DocFormat::from_path(&self.config_path()).unwrap_or(crate::doc::DocFormat::Json)
    }

    /// 备份配置
    fn backup_config(&self) -> Result<PathBuf>;

    /// 清理旧备份
    fn cleanup_old_backups(&self, keep: usize) -> Result<()>;

    fn managed_paths(&self) -> Vec<PathBuf> {
        vec![self.config_path()]
    }

    fn snapshot_files(&self) -> Result<Vec<FileSnapshot>> {
        self.managed_paths()
            .into_iter()
            .map(|path| {
                let contents = if path.exists() {
                    Some(fs::read(&path)?)
                } else {
                    None
                };
                Ok(FileSnapshot { path, contents })
            })
            .collect()
    }

    fn restore_files(&self, snapshots: &[FileSnapshot]) -> Result<()> {
        restore_snapshots(snapshots)
    }

    /// 应用 API 凭据到工具特定的位置（默认无操作）。
    /// 大多数工具的 API 凭据通过 merge_config 写入主配置文件即可。
    /// Pi 等工具的 key 在 auth.json / models.json，需要重写此方法。
    fn apply_api_credentials(&self, _api_profile: &ApiProfile) -> Result<()> {
        Ok(())
    }
}

/// 把前镜像逐文件写回（存在 → 原子写回；不存在 → 删除）。失败不中断，收集首个错误。
/// 供事务错误回滚与崩溃恢复 journal 共用。
pub fn restore_snapshots(snapshots: &[FileSnapshot]) -> Result<()> {
    let mut first_error = None;
    for snapshot in snapshots {
        let result: Result<()> = match &snapshot.contents {
            Some(contents) => atomic_write_private(&snapshot.path, contents),
            None if snapshot.path.exists() => fs::remove_file(&snapshot.path).map_err(Into::into),
            None => Ok(()),
        };
        if let Err(error) = result {
            if first_error.is_none() {
                first_error = Some(error);
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}

/// Snapshot every adapter-managed file so multi-target imports can restore the
/// complete pre-import filesystem state after a later phase fails.
pub fn snapshot_all_managed_files() -> Result<Vec<FileSnapshot>> {
    let mut snapshots = Vec::new();
    for target_app in TargetApp::all() {
        let adapter = get_adapter(target_app);
        snapshots.extend(adapter.snapshot_files()?);
    }
    Ok(snapshots)
}

pub fn apply_profile_transaction(
    adapter: &dyn ConfigAdapter,
    api_profile: &ApiProfile,
    shared_config: &serde_json::Value,
) -> Result<()> {
    apply_profile_transaction_with_previous(adapter, api_profile, shared_config, None)
}

/// 带 `previous_managed` 的事务入口。
///
/// `previous_managed` 是**上次切换时写入的受管片段**，用于在保真写入路径上
/// 摘除「上次受管、本次不再受管」的字段。为 `None` 时等价于首次切换：
/// 只叠加、不摘除。
///
/// 未迁移到保真路径的适配器会忽略该参数（默认实现退化为整体写入）。
pub fn apply_profile_transaction_with_previous(
    adapter: &dyn ConfigAdapter,
    api_profile: &ApiProfile,
    shared_config: &serde_json::Value,
    previous_managed: Option<&serde_json::Value>,
) -> Result<()> {
    let snapshots = adapter.snapshot_files()?;
    let merged = adapter.merge_config(api_profile, shared_config);
    // 写盘前语义校验：不通过则直接走快照回滚，避免残缺配置落地。
    adapter.verify_merged_config(&merged, api_profile)?;
    if let Err(error) = adapter
        .write_config_merged(&merged, previous_managed)
        .and_then(|_| adapter.apply_api_credentials(api_profile))
        .and_then(|_| adapter.apply_auxiliary_config(shared_config))
    {
        if let Err(restore_error) = adapter.restore_files(&snapshots) {
            // 用标记类型而非纯文案：上层据此能区分「已回滚的干净失败」与
            // 「部分生效、回滚也失败」，不必去匹配字符串。
            return Err(anyhow::Error::new(crate::error::RollbackFailed::new(
                format!("{error}；回滚失败：{restore_error}"),
            )));
        }
        return Err(error);
    }
    Ok(())
}

/// 解析切换时应写入的共享配置（API 凭据除外）。
///
/// 权威规则（磁盘为准）：
/// - 主配置文件不存在 → 直接用数据库，无库则空对象；
/// - 磁盘任一受管文件不早于数据库 `updated_at`（用户在 Helio 之外改过，
///   含冷启动后、托盘驻留期间的手改）→ 以磁盘为准，跳过数据库补缺，
///   避免把用户删掉的键/条目从旧库里复活；
/// - 否则（Helio 自己的写入，或取不到 mtime）→ 保留“磁盘优先 + 数据库补齐”。
pub fn resolve_shared_config(
    target_app: TargetApp,
    persisted_shared_config: Option<crate::models::SharedConfig>,
) -> Result<serde_json::Value> {
    let adapter = get_adapter(target_app);
    resolve_shared_config_with_adapter(persisted_shared_config, adapter.as_ref())
}

fn resolve_shared_config_with_adapter(
    persisted_shared_config: Option<crate::models::SharedConfig>,
    adapter: &dyn ConfigAdapter,
) -> Result<serde_json::Value> {
    let mut shared_config = if adapter.config_path().exists() {
        let current_config = adapter.read_config()?;
        adapter.extract_shared_config(&current_config)
    } else {
        persisted_shared_config
            .as_ref()
            .map(|persisted| persisted.config.clone())
            .unwrap_or_else(|| serde_json::json!({}))
    };

    if let Some(previous) = persisted_shared_config {
        if !disk_is_newer_than_db(adapter, previous.updated_at) {
            backfill_missing_top_level(&mut shared_config, &previous.config);
            backfill_mcp_entries(&mut shared_config, &previous.config);
        }
    }
    Ok(shared_config)
}

/// 任一受管文件 mtime（秒级）不早于数据库 `updated_at` 即视为磁盘更新。
/// 取“不早于”而非“晚于”：Helio 自己的写盘与入库多在同一秒，跳过补缺无影响
///（刚写盘的内容本就没有缺失）；同秒内的外部手改同样归磁盘赢。
/// 取不到 mtime 时回退到旧行为（补缺），偏向迁移不断。
fn disk_is_newer_than_db(adapter: &dyn ConfigAdapter, updated_at: Option<i64>) -> bool {
    let Some(updated_at) = updated_at else {
        return false;
    };
    let updated_at = updated_at.max(0) as u64;
    adapter.managed_paths().into_iter().any(|path| {
        std::fs::metadata(&path)
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .is_some_and(|elapsed| elapsed.as_secs() >= updated_at)
    })
}

pub fn apply_profile_configuration(
    target_app: TargetApp,
    api_profile: &ApiProfile,
    shared_config: &serde_json::Value,
    create_backup: bool,
    previous_managed: Option<&serde_json::Value>,
) -> Result<ProfileApplicationResult> {
    let adapter = get_adapter(target_app);
    adapter.validate_profile(api_profile)?;
    let backup_path = if create_backup && adapter.config_path().exists() {
        Some(adapter.backup_config()?)
    } else {
        None
    };
    apply_profile_transaction_with_previous(
        adapter.as_ref(),
        api_profile,
        shared_config,
        previous_managed,
    )?;
    Ok(ProfileApplicationResult {
        backup_path,
        config_path: adapter.config_path(),
    })
}

/// 推导「上次写入的受管片段」，供保真写入路径摘除陈旧字段。
///
/// 做法：把**上一个 active Profile** 对当前共享配置跑一遍 `merge_config`，
/// 结果就是「若它此刻被应用，Helio 会写入的内容」。因为两侧共用同一份
/// `shared_config`，两份合并结果的共享部分完全相同，差集恰好是受管字段。
///
/// 上一个 Profile 不存在（首次切换）或已被删除时返回 `None`——此时保真路径
/// 退化为纯叠加，不会摘除 live 中的任何内容。
///
/// 已知取舍：按**路径**摘除、不比对值。若用户手改过某个受管字段，切换时它
/// 仍会被摘除（受管字段归 Helio 所有）。这样做的收益是陈旧字段不会无限累积。
pub fn derive_previous_managed(
    target_app: TargetApp,
    previous_active: Option<&ApiProfile>,
    shared_config: &serde_json::Value,
) -> Option<serde_json::Value> {
    let previous = previous_active?;
    Some(get_adapter(target_app).merge_config(previous, shared_config))
}

/// 一次完整的配置切换（GUI / 托盘共用入口）：
/// 写 journal（意图 + 前镜像 + 旧 active）→ 若已是目标 profile 则先清 active →
/// DB 写 shared_config → 写配置文件（含备份）→ 记 active_profile → 删 journal。
///
/// 清 active 是为了「重复切换同一 profile」时的崩溃恢复：若不先清，半完成时
/// `active_profile` 仍等于目标，恢复逻辑会误判「已完成」而保留半状态配置。
///
/// 软失败（本进程内 Err）会**立即**按 journal 回滚，不把半状态留给下次启动。
/// 进程崩溃后由 `journal::recover_interrupted_switch` 在下次启动时完成同样恢复。
pub fn apply_profile_switch(
    db: &crate::db::Database,
    target_app: TargetApp,
    api_profile: &ApiProfile,
    shared_config: &serde_json::Value,
    create_backup: bool,
) -> Result<ProfileApplicationResult> {
    let profile_id = api_profile
        .id
        .ok_or_else(|| anyhow::anyhow!("Profile '{}' has no id", api_profile.name))?;
    let adapter = get_adapter(target_app);
    // 切换前的 active Profile：既用于「制造 active != target 窗口」的判定，
    // 也用于推导上次写入的受管片段。必须在任何写操作之前取。
    let previous_active_profile = db.get_active_profile_full(target_app)?;
    let previous_opencode_state = if target_app == TargetApp::OpenCode {
        Some(db.get_opencode_managed_models()?)
    } else {
        None
    };
    let provider_ownership = match target_app {
        TargetApp::OpenCode => {
            let provider_id =
                opencode::OpenCodeAdapter::normalize_provider_id(&api_profile.provider);
            let exists = shared_config
                .get("provider")
                .and_then(|value| value.as_object())
                .map(|providers| providers.contains_key(&provider_id))
                .unwrap_or(false);
            Some((provider_id, !exists))
        }
        TargetApp::ZCode => {
            let provider_id = zcode::ZCodeAdapter::normalize_provider_id(&api_profile.provider);
            let exists = shared_config
                .get("provider")
                .and_then(|value| value.as_object())
                .map(|providers| providers.contains_key(&provider_id))
                .unwrap_or(false);
            Some((provider_id, !exists))
        }
        _ => None,
    };
    let journal = journal::begin_switch(
        db,
        adapter.as_ref(),
        target_app,
        profile_id,
        &api_profile.name,
    )?;

    let result = (|| -> Result<ProfileApplicationResult> {
        let (effective_shared_config, next_opencode_state) = if target_app == TargetApp::OpenCode {
            let previous = previous_opencode_state
                .as_ref()
                .cloned()
                .unwrap_or_default();
            let (config, next_state) = opencode::OpenCodeAdapter::prepare_shared_config_for_switch(
                shared_config,
                api_profile,
                &previous,
            );
            (config, Some(next_state))
        } else {
            (shared_config.clone(), None)
        };

        // 制造 `active != target` 窗口：仅当当前 active 已是目标时需要。
        // 不同 profile 之间切换时 active 本来就不是目标，无需动。
        let already_active = previous_active_profile
            .as_ref()
            .and_then(|profile| profile.id)
            .is_some_and(|id| id == profile_id);
        if already_active {
            db.clear_active_profile(target_app)?;
        }
        db.save_shared_config(target_app, effective_shared_config.clone())?;
        // 上一个 active Profile 必须在写盘前取——切换成功后 active 已指向新档案。
        // 它的 merge 结果即「上次写入的受管片段」，供保真路径摘除陈旧字段。
        let previous_managed = derive_previous_managed(
            target_app,
            previous_active_profile.as_ref(),
            &effective_shared_config,
        );
        let applied = apply_profile_configuration(
            target_app,
            api_profile,
            &effective_shared_config,
            create_backup,
            previous_managed.as_ref(),
        )?;
        if let Some(state) = next_opencode_state {
            db.replace_opencode_managed_models(&state)?;
        }
        db.set_active_profile(target_app, profile_id)?;
        if let Some((provider_id, managed_by_helio)) = provider_ownership.as_ref() {
            db.record_provider_ownership_if_missing(target_app, provider_id, *managed_by_helio)?;
        }
        Ok(applied)
    })();

    match result {
        Ok(applied) => {
            if let Some(journal) = journal {
                if let Err(error) = journal.commit() {
                    // 切换已完成，清理失败仅导致下次启动多一次「已完成」判定。
                    tracing::warn!("{error:#}");
                }
            }
            Ok(applied)
        }
        Err(error) => {
            // 软失败立即按 journal 回滚（与启动恢复同一路径），避免半状态一直留到下次启动。
            // 无 journal 的场景（:memory: 测试库）只能返回原错误。
            if journal.is_some() {
                if let Err(recover_error) = journal::recover_interrupted_switch(db) {
                    return Err(anyhow::anyhow!(
                        "{error}; immediate journal recovery also failed: {recover_error}"
                    ));
                }
            }
            Err(error)
        }
    }
}

/// 读取所有已注册工具的当前共享配置并持久化到 Helio 数据库。
///
/// API profile 仍是 URL/Key 的唯一来源；这里只同步 permissions、hooks、MCP 等
/// adapter 提取出的非档案配置，供便携备份取得导出瞬间的真实状态。
pub fn sync_all_shared_configs(db: &crate::db::Database) -> Result<()> {
    for target_app in TargetApp::all() {
        let adapter = get_adapter(target_app);
        sync_shared_config_if_present(db, target_app, adapter.as_ref())?;
    }
    Ok(())
}

/// 冷启动时把磁盘上的共享配置同步回 Helio 数据库（仅共享配置，不碰 API 凭据）。
///
/// 逐工具处理：主配置文件不存在则跳过（保留数据库原值，避免把未安装工具的
/// 共享配置清零）；读取失败则记 warn 后跳过，不阻塞启动。磁盘为准。
/// 返回实际同步成功的工具列表。
pub fn sync_startup_shared_configs(db: &crate::db::Database) -> Vec<TargetApp> {
    let mut synced = Vec::new();
    for target_app in TargetApp::all() {
        let adapter = get_adapter(target_app);
        match sync_shared_config_if_present(db, target_app, adapter.as_ref()) {
            Ok(true) => synced.push(target_app),
            Ok(false) => {}
            Err(error) => tracing::warn!("[Helio] startup sync: skip {target_app}: {error:#}"),
        }
    }
    synced
}

/// 读取磁盘配置、提取共享部分后入库。主配置文件不存在时返回 `Ok(false)`
/// 且不写库，调用方以此区分“未安装”与“已同步”。
fn sync_shared_config_if_present(
    db: &crate::db::Database,
    target_app: TargetApp,
    adapter: &dyn ConfigAdapter,
) -> Result<bool> {
    // 文件不存在（工具未安装）→ 保留数据库原值，不用空配置清零。
    if !adapter.config_path().exists() {
        return Ok(false);
    }
    let config = adapter.read_config().with_context(|| {
        format!("Failed to read {target_app} configuration before shared-config sync")
    })?;
    db.save_shared_config(target_app, adapter.extract_shared_config(&config))
        .with_context(|| format!("Failed to save {target_app} shared configuration"))?;
    Ok(true)
}

/// 将导入数据库中的 active profile 写回对应工具。使用数据库内的 shared config
/// 作为事实源，避免目标机现有文件把备份中的 MCP 配置覆盖掉。
pub fn materialize_active_profiles(db: &crate::db::Database) -> Result<Vec<TargetApp>> {
    let mut restored = Vec::new();
    for target_app in TargetApp::all() {
        let Some(mut profile) = db.get_active_profile_full(target_app)? else {
            continue;
        };
        profile.normalize_keys();
        let shared_config = db
            .get_shared_config(target_app)?
            .map(|shared| shared.config)
            .unwrap_or_else(|| serde_json::json!({}));
        apply_profile_switch(db, target_app, &profile, &shared_config, true)
            .with_context(|| format!("Failed to restore imported {target_app} configuration"))?;
        restored.push(target_app);
    }
    Ok(restored)
}

#[cfg(test)]
mod switch_journal_tests {
    use super::*;
    use crate::db::Database;
    use crate::models::ApiProfile;
    use std::fs;
    use std::path::PathBuf;

    // 验证：begin_switch 遇到残留 journal 时会先恢复，且恢复失败时拒绝开新事务。
    #[test]
    fn begin_switch_refuses_to_overwrite_unrecoverable_journal() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let db_path = dir.path().join("db.sqlite");
        let db = Database::open(&db_path)?;

        // 未知 version：recover 会忽略并保留 journal。
        let journal_path = PathBuf::from(format!("{}.switch-journal.json", db_path.display()));
        fs::write(
            &journal_path,
            br#"{
              "version": 999,
              "app": "claude-code",
              "profile_id": 1,
              "profile_name": "x",
              "created_at": 0,
              "previous_shared_config": null,
              "snapshots": []
            }"#,
        )?;

        struct Dummy;
        impl ConfigAdapter for Dummy {
            fn config_path(&self) -> PathBuf {
                PathBuf::from("/tmp/unused")
            }
            fn read_config(&self) -> Result<serde_json::Value> {
                Ok(serde_json::json!({}))
            }
            fn extract_shared_config(&self, c: &serde_json::Value) -> serde_json::Value {
                c.clone()
            }
            fn merge_config(&self, _: &ApiProfile, s: &serde_json::Value) -> serde_json::Value {
                s.clone()
            }
            fn write_config(&self, _: &serde_json::Value) -> Result<()> {
                Ok(())
            }
            fn backup_config(&self) -> Result<PathBuf> {
                Ok(PathBuf::from("/tmp/unused"))
            }
            fn cleanup_old_backups(&self, _: usize) -> Result<()> {
                Ok(())
            }
        }

        let err = journal::begin_switch(&db, &Dummy, TargetApp::ClaudeCode, 1, "x").unwrap_err();
        assert!(
            err.to_string().contains("恢复未完成") || err.to_string().contains("journal"),
            "unexpected: {err}"
        );
        assert!(journal_path.exists(), "不得覆盖未恢复的 journal");
        Ok(())
    }
}

pub fn backfill_missing_top_level(live: &mut serde_json::Value, previous: &serde_json::Value) {
    if let (Some(live_object), Some(previous_object)) = (live.as_object_mut(), previous.as_object())
    {
        for (key, value) in previous_object {
            if !live_object.contains_key(key) {
                live_object.insert(key.clone(), value.clone());
            }
        }
    }
}

/// 对 MCP 键做条目级补缺:previous(数据库)有而 live(磁盘)缺失的 MCP 条目补入 live,
/// live 已有条目以 live 为准(磁盘优先)。openclaw 的 `mcp.servers` 嵌套层同样处理。
fn backfill_mcp_entries(live: &mut serde_json::Value, previous: &serde_json::Value) {
    const MCP_KEYS: &[&str] = &["mcpServers", "mcp_servers", "mcp"];
    let (Some(previous_object), Some(live_object)) = (previous.as_object(), live.as_object_mut())
    else {
        return;
    };

    for key in MCP_KEYS {
        let Some(previous_value) = previous_object.get(*key) else {
            continue;
        };
        let Some(previous_map) = previous_value.as_object() else {
            continue;
        };

        let live_value = live_object
            .entry((*key).to_string())
            .or_insert_with(|| serde_json::json!({}));
        if let Some(live_map) = live_value.as_object_mut() {
            for (name, value) in previous_map {
                if !live_map.contains_key(name) {
                    live_map.insert(name.clone(), value.clone());
                }
            }
        }

        // openclaw: mcp.servers 深一层补缺
        if *key == "mcp" {
            if let Some(previous_servers) =
                previous_value.get("servers").and_then(|v| v.as_object())
            {
                if let Some(live_servers) = live_value
                    .get_mut("servers")
                    .and_then(|v| v.as_object_mut())
                {
                    for (name, value) in previous_servers {
                        if !live_servers.contains_key(name) {
                            live_servers.insert(name.clone(), value.clone());
                        }
                    }
                }
            }
        }
    }
}

pub mod backup;
pub mod claude_code;
pub mod codex;
mod credentials;
pub mod hermes;
pub mod journal;
pub mod openclaw;
pub mod opencode;
pub mod pi;
pub mod zcode;

/// 获取适配器
pub fn get_adapter(target_app: TargetApp) -> Box<dyn ConfigAdapter> {
    match target_app {
        TargetApp::ClaudeCode => Box::new(claude_code::ClaudeCodeAdapter::new()),
        TargetApp::Codex => Box::new(codex::CodexAdapter::new()),
        TargetApp::Pi => Box::new(pi::PiAdapter::new()),
        TargetApp::OpenCode => Box::new(opencode::OpenCodeAdapter::new()),
        TargetApp::Hermes => Box::new(hermes::HermesAdapter::new()),
        TargetApp::OpenClaw => Box::new(openclaw::OpenClawAdapter::new()),
        TargetApp::ZCode => Box::new(zcode::ZCodeAdapter::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::{apply_profile_transaction, ConfigAdapter};
    use crate::db::Database;
    use crate::models::ApiProfile;
    use anyhow::Result;
    use std::fs;
    use std::path::PathBuf;

    struct FailingAdapter {
        config: PathBuf,
        credentials: PathBuf,
    }

    impl ConfigAdapter for FailingAdapter {
        fn config_path(&self) -> PathBuf {
            self.config.clone()
        }
        fn read_config(&self) -> Result<serde_json::Value> {
            Ok(serde_json::json!({}))
        }
        fn extract_shared_config(&self, config: &serde_json::Value) -> serde_json::Value {
            config.clone()
        }
        fn merge_config(&self, _: &ApiProfile, _: &serde_json::Value) -> serde_json::Value {
            serde_json::json!({"changed": true})
        }
        fn write_config(&self, _: &serde_json::Value) -> Result<()> {
            fs::write(&self.config, b"changed")?;
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
        fn apply_api_credentials(&self, _: &ApiProfile) -> Result<()> {
            fs::write(&self.credentials, b"new secret")?;
            anyhow::bail!("injected credential write failure")
        }
    }

    #[test]
    fn transaction_restores_existing_files_after_failure() {
        let dir = tempfile::tempdir().unwrap();
        let adapter = FailingAdapter {
            config: dir.path().join("config"),
            credentials: dir.path().join("auth"),
        };
        fs::write(&adapter.config, b"old config").unwrap();
        fs::write(&adapter.credentials, b"old secret").unwrap();

        let error =
            apply_profile_transaction(&adapter, &ApiProfile::default(), &serde_json::json!({}))
                .unwrap_err();
        assert!(error.to_string().contains("injected"));
        assert_eq!(fs::read(&adapter.config).unwrap(), b"old config");
        assert_eq!(fs::read(&adapter.credentials).unwrap(), b"old secret");
    }

    struct FailingAuxAdapter {
        config: PathBuf,
        aux: PathBuf,
    }

    impl ConfigAdapter for FailingAuxAdapter {
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
            vec![self.config.clone(), self.aux.clone()]
        }
        fn apply_auxiliary_config(&self, _: &serde_json::Value) -> Result<()> {
            anyhow::bail!("injected auxiliary write failure")
        }
    }

    #[test]
    fn transaction_restores_all_managed_files_after_aux_failure() {
        let dir = tempfile::tempdir().unwrap();
        let adapter = FailingAuxAdapter {
            config: dir.path().join("config"),
            aux: dir.path().join("claude.json"),
        };
        fs::write(&adapter.config, b"old config").unwrap();
        fs::write(&adapter.aux, b"old mcp").unwrap();

        let error = apply_profile_transaction(
            &adapter,
            &ApiProfile::default(),
            &serde_json::json!({ "mcpServers": { "x": { "command": "y" } } }),
        )
        .unwrap_err();
        assert!(error.to_string().contains("injected"));
        assert_eq!(fs::read(&adapter.config).unwrap(), b"old config");
        assert_eq!(fs::read(&adapter.aux).unwrap(), b"old mcp");
    }

    #[test]
    fn transaction_removes_new_files_after_failure() {
        let dir = tempfile::tempdir().unwrap();
        let adapter = FailingAdapter {
            config: dir.path().join("config"),
            credentials: dir.path().join("auth"),
        };

        assert!(apply_profile_transaction(
            &adapter,
            &ApiProfile::default(),
            &serde_json::json!({})
        )
        .is_err());
        assert!(!adapter.config.exists());
        assert!(!adapter.credentials.exists());
    }

    #[test]
    fn backfill_mcp_entries_fills_missing_codex_servers() {
        let mut live = serde_json::json!({ "mcp_servers": {} });
        let previous = serde_json::json!({
            "mcp_servers": {
                "bing-search": { "command": "npx" },
                "github": { "url": "https://x/mcp" }
            }
        });
        super::backfill_mcp_entries(&mut live, &previous);
        assert_eq!(live["mcp_servers"]["bing-search"]["command"], "npx");
        assert_eq!(live["mcp_servers"]["github"]["url"], "https://x/mcp");
    }

    #[test]
    fn backfill_mcp_entries_keeps_live_conflict() {
        let mut live = serde_json::json!({
            "mcp_servers": { "github": { "url": "https://live/mcp" } }
        });
        let previous = serde_json::json!({
            "mcp_servers": { "github": { "url": "https://db/mcp" }, "new": { "command": "x" } }
        });
        super::backfill_mcp_entries(&mut live, &previous);
        assert_eq!(live["mcp_servers"]["github"]["url"], "https://live/mcp");
        assert_eq!(live["mcp_servers"]["new"]["command"], "x");
    }

    #[test]
    fn backfill_mcp_entries_merges_openclaw_nested_servers() {
        let mut live = serde_json::json!({ "mcp": {} });
        let previous = serde_json::json!({
            "mcp": { "servers": { "cdp-bridge": { "command": "uvx" } } }
        });
        super::backfill_mcp_entries(&mut live, &previous);
        assert_eq!(live["mcp"]["servers"]["cdp-bridge"]["command"], "uvx");
    }

    #[test]
    fn backfill_mcp_entries_skips_non_object_values() {
        let mut live = serde_json::json!({});
        let previous = serde_json::json!({ "mcp_servers": "nope" });
        super::backfill_mcp_entries(&mut live, &previous);
        assert!(live.get("mcp_servers").is_none());
    }

    #[test]
    fn portable_sync_persists_current_mcp_config() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config");
        fs::write(&path, b"{}")?;
        let db = Database::open(":memory:")?;
        let adapter = StartupSyncAdapter {
            path,
            config: serde_json::json!({
                "mcp_servers": { "new-server": { "command": "npx" } }
            }),
        };
        assert!(super::sync_shared_config_if_present(
            &db,
            crate::models::TargetApp::Codex,
            &adapter
        )?);
        assert_eq!(
            db.get_shared_config(crate::models::TargetApp::Codex)?
                .unwrap()
                .config["mcp_servers"]["new-server"]["command"],
            "npx"
        );
        Ok(())
    }

    struct StartupSyncAdapter {
        path: PathBuf,
        config: serde_json::Value,
    }

    impl ConfigAdapter for StartupSyncAdapter {
        fn config_path(&self) -> PathBuf {
            self.path.clone()
        }
        fn read_config(&self) -> Result<serde_json::Value> {
            Ok(self.config.clone())
        }
        fn extract_shared_config(&self, config: &serde_json::Value) -> serde_json::Value {
            config.clone()
        }
        fn merge_config(&self, _: &ApiProfile, shared: &serde_json::Value) -> serde_json::Value {
            shared.clone()
        }
        fn write_config(&self, _: &serde_json::Value) -> Result<()> {
            Ok(())
        }
        fn backup_config(&self) -> Result<PathBuf> {
            Ok(self.path.clone())
        }
        fn cleanup_old_backups(&self, _: usize) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn startup_sync_skips_missing_config_file() -> Result<()> {
        let db = Database::open(":memory:")?;
        db.save_shared_config(
            crate::models::TargetApp::Codex,
            serde_json::json!({ "keep": true }),
        )?;
        let adapter = StartupSyncAdapter {
            path: PathBuf::from("/tmp/helio-startup-sync-missing-dir/config"),
            config: serde_json::json!({ "mcp_servers": {} }),
        };
        assert!(!adapter.config_path().exists());
        let synced =
            super::sync_shared_config_if_present(&db, crate::models::TargetApp::Codex, &adapter)?;
        assert!(!synced);
        let kept = db
            .get_shared_config(crate::models::TargetApp::Codex)?
            .unwrap();
        assert_eq!(kept.config["keep"], true);
        Ok(())
    }

    #[test]
    fn startup_sync_overwrites_db_with_disk_config() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config");
        fs::write(&path, b"{}")?;
        let db = Database::open(":memory:")?;
        db.save_shared_config(
            crate::models::TargetApp::Codex,
            serde_json::json!({ "stale": true }),
        )?;
        let adapter = StartupSyncAdapter {
            path,
            config: serde_json::json!({
                "mcp_servers": { "fresh": { "command": "npx" } }
            }),
        };
        let synced =
            super::sync_shared_config_if_present(&db, crate::models::TargetApp::Codex, &adapter)?;
        assert!(synced);
        let current = db
            .get_shared_config(crate::models::TargetApp::Codex)?
            .unwrap();
        assert_eq!(current.config["mcp_servers"]["fresh"]["command"], "npx");
        assert!(current.config.get("stale").is_none());
        Ok(())
    }

    fn startup_persisted(
        target: crate::models::TargetApp,
        config: serde_json::Value,
        updated_at: Option<i64>,
    ) -> crate::models::SharedConfig {
        crate::models::SharedConfig {
            target_app: target,
            config,
            updated_at,
        }
    }

    #[test]
    fn resolve_skips_backfill_when_disk_is_newer() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config");
        fs::write(&path, b"{}")?;
        let adapter = StartupSyncAdapter {
            path,
            config: serde_json::json!({ "live": true, "mcp_servers": {} }),
        };
        let persisted = startup_persisted(
            crate::models::TargetApp::Codex,
            serde_json::json!({
                "live": true,
                "db_only": 1,
                "mcp_servers": { "old": { "command": "x" } }
            }),
            Some(0),
        );
        let resolved = super::resolve_shared_config_with_adapter(Some(persisted), &adapter)?;
        assert_eq!(resolved["live"], true);
        assert!(resolved.get("db_only").is_none());
        assert!(resolved["mcp_servers"].as_object().unwrap().is_empty());
        Ok(())
    }

    #[test]
    fn resolve_backfills_when_db_is_newer() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config");
        fs::write(&path, b"{}")?;
        let adapter = StartupSyncAdapter {
            path,
            config: serde_json::json!({ "live": true, "mcp_servers": {} }),
        };
        let future = chrono::Utc::now().timestamp() + 3600;
        let persisted = startup_persisted(
            crate::models::TargetApp::Codex,
            serde_json::json!({
                "live": true,
                "db_only": 1,
                "mcp_servers": { "old": { "command": "x" } }
            }),
            Some(future),
        );
        let resolved = super::resolve_shared_config_with_adapter(Some(persisted), &adapter)?;
        assert_eq!(resolved["db_only"], 1);
        assert_eq!(resolved["mcp_servers"]["old"]["command"], "x");
        Ok(())
    }
}
