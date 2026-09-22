//! 导出 / 导入：数据库快照、便携备份、Skills 归档。
//!
//! 三条独立的导出路径，共享同一套「用户选路径 → 校验 → 原子写」的骨架。
//! 导入侧更重：换库（`replace_database_locked`）是一个完整的补偿事务——
//! 备份 live → 替换 → 失败逐级回滚，且回滚失败要标记 `PartialFailure`。

use crate::commands::helpers::{default_db_path, home_dir, reject_export_onto_live_db};
use crate::commands::{AppError, AppState};
use serde::Serialize;
use switch_api::db::Database;
use tauri::State;

#[tauri::command]
pub async fn export_database(
    output_path: String,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    let _db = state.db.lock()?;
    let db_path = default_db_path()?;
    reject_export_onto_live_db(&output_path)?;

    // 快照而非文件拷贝：拷主文件会漏掉还在 -wal 里的已提交数据（实测可导出成空档案库）。
    // 导出目标由用户选择，snapshot_to 只收紧文件本身权限，不动其所在目录。
    Database::snapshot_to(&db_path, std::path::Path::new(&output_path))
        .map_err(|e| AppError::from(e).with_context("导出数据库失败"))?;

    Ok(())
}

#[tauri::command]
pub async fn export_portable_backup(
    output_path: String,
    state: State<'_, AppState>,
) -> Result<switch_api::utils::portable_backup::PortableBackupExportResult, AppError> {
    let _write_guard = state.config_lock.lock()?;
    let db = state.db.lock()?;
    reject_export_onto_live_db(&output_path)?;
    switch_api::adapters::sync_all_shared_configs(&db)
        .map_err(|e| AppError::from(e).with_context("导出前同步共享配置失败"))?;
    let db_path = default_db_path()?;
    let home = home_dir()?;
    switch_api::utils::portable_backup::export_portable_backup(
        &home,
        &db_path,
        std::path::Path::new(&output_path),
    )
    .map_err(|e| AppError::from(e).with_context("导出便携备份失败"))
}

/// 把全部 skills 目录打包为 tar.gz（manifest + {app}/{skill}/...）。
/// 与数据库备份正交：skills 是文件系统资产，不入库。
#[tauri::command]
pub async fn export_skills(
    output_path: String,
    state: State<'_, AppState>,
) -> Result<switch_api::utils::skills_backup::SkillsExportResult, AppError> {
    // 与配置写路径互斥：导出期间避免并发切换改到半截 skill 目录。
    let _write_guard = state.config_lock.lock()?;
    reject_export_onto_live_db(&output_path)?;
    let home = home_dir()?;
    switch_api::utils::skills_backup::export_skills(&home, std::path::Path::new(&output_path))
        .map_err(|e| AppError::from(e).with_context("导出 Skills 失败"))
}

/// 从 tar.gz 归档恢复 skills。整体校验不通过则拒绝且不写盘；
/// 同名 skill 目录已存在时跳过（不覆盖）。
#[tauri::command]
pub async fn import_skills(
    input_path: String,
    state: State<'_, AppState>,
) -> Result<switch_api::utils::skills_backup::SkillsImportResult, AppError> {
    // 与切换/写配置互斥；skills_backup 内部另有进程级 IMPORT_LOCK 防重入。
    let _write_guard = state.config_lock.lock()?;
    let home = home_dir()?;
    switch_api::utils::skills_backup::import_skills(&home, std::path::Path::new(&input_path))
        .map_err(|e| AppError::from(e).with_context("导入 Skills 失败"))
}

#[tauri::command]
pub async fn import_database(
    input_path: String,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    let db_path = default_db_path()?;

    // 与切换/写配置互斥：导入会替换 live 库文件，期间绝不能有其他命令仍持有旧连接写盘。
    let _write_guard = state.config_lock.lock()?;
    let snapshots = switch_api::adapters::snapshot_all_managed_files()
        .map_err(|e| AppError::from(e).with_context("导入前快照工具配置失败"))?;
    let mut db = state.db.lock()?;
    let backup = replace_database_locked(std::path::Path::new(&input_path), &db_path, &mut db)?;
    if let Err(error) = switch_api::adapters::materialize_active_profiles(&db) {
        let rollback =
            rollback_import_state(&db_path, &mut db, backup.as_deref(), &snapshots, None);
        return Err(import_after_apply_failure("恢复生效配置", error, rollback));
    }
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct PortableBackupImportResult {
    pub restored_targets: Vec<String>,
    pub skills: switch_api::utils::skills_backup::SkillsImportResult,
}

#[tauri::command]
pub async fn import_portable_backup(
    input_path: String,
    state: State<'_, AppState>,
) -> Result<PortableBackupImportResult, AppError> {
    let _write_guard = state.config_lock.lock()?;
    let archive = switch_api::utils::portable_backup::extract_portable_backup(
        std::path::Path::new(&input_path),
    )
    .map_err(|e| AppError::from(e).with_context("便携备份校验失败"))?;
    Database::validate_import_candidate(&archive.database_path)
        .map_err(|e| AppError::from(e).with_context("便携备份中的数据库无效"))?;

    let db_path = default_db_path()?;
    let snapshots = switch_api::adapters::snapshot_all_managed_files()
        .map_err(|e| AppError::from(e).with_context("导入前快照工具配置失败"))?;
    let mut db = state.db.lock()?;
    let backup = replace_database_locked(&archive.database_path, &db_path, &mut db)?;

    let home = home_dir()?;
    let skills = match switch_api::utils::skills_backup::import_skills(&home, &archive.skills_path)
    {
        Ok(skills) => skills,
        Err(error) => {
            let rollback =
                rollback_import_state(&db_path, &mut db, backup.as_deref(), &snapshots, None);
            return Err(import_after_apply_failure("恢复 Skills", error, rollback));
        }
    };
    let restored_targets = match switch_api::adapters::materialize_active_profiles(&db) {
        Ok(targets) => targets
            .into_iter()
            .map(|target| target.as_str().to_string())
            .collect(),
        Err(error) => {
            let rollback = rollback_import_state(
                &db_path,
                &mut db,
                backup.as_deref(),
                &snapshots,
                Some((&home, &skills)),
            );
            return Err(import_after_apply_failure("恢复生效配置", error, rollback));
        }
    };
    Ok(PortableBackupImportResult {
        restored_targets,
        skills,
    })
}

/// 导入流程「数据库已替换、但后续步骤失败」的统一构造。
///
/// 两个分支的语义差别很大，不能共用一句话：
///
/// - **回滚成功**：磁盘已复原到导入前，用户什么都不用做，只是这次导入没生效。
///   类别沿用底层错误的真实类型（`AppError::from` 按类型分类），不要一律标成
///   `PartialFailure`——否则前端会提示「请核对工具实际状态」，而实际什么都没落地。
/// - **回滚失败**：数据库/工具配置留在半成品状态，用户必须去核对，才叫
///   `PartialFailure`。这与 `error.rs` 里「回滚失败才算部分生效」的判定一致。
fn import_after_apply_failure(
    what_failed: &str,
    error: anyhow::Error,
    rollback: Result<(), String>,
) -> AppError {
    match rollback {
        Ok(()) => AppError::from(error)
            .with_context(format!("导入失败（{what_failed}出错），已回滚到导入前状态")),
        Err(rollback_error) => AppError::partial_failure(format!(
            "导入已部分生效（{what_failed}出错），回滚也失败，请核对工具当前配置"
        ))
        .with_detail(format!("{error:#}；回滚失败：{rollback_error}")),
    }
}

fn replace_database_locked(
    input_path: &std::path::Path,
    db_path: &std::path::Path,
    db: &mut Database,
) -> Result<Option<std::path::PathBuf>, AppError> {
    let placeholder = Database::open(":memory:")
        .map_err(|e| AppError::from(e).with_context("导入前创建数据库占位连接失败"))?;
    let previous = std::mem::replace(db, placeholder);
    drop(previous);

    let backup = match Database::replace_file_from_import(input_path, db_path) {
        Ok(backup) => backup,
        Err(error) => {
            *db = Database::open(db_path).map_err(|restore| {
                AppError::partial_failure("导入失败，且无法重新打开原数据库")
                    .with_detail(format!("{error:#}；重新打开失败：{restore:#}"))
            })?;
            return Err(AppError::from(error).with_context("导入数据库文件失败"));
        }
    };
    match Database::open(db_path) {
        Ok(reloaded) => {
            *db = reloaded;
            if let Err(error) = switch_api::adapters::journal::recover_interrupted_switch(db) {
                eprintln!("[Helio] recover after import failed: {error:#}");
            }
            Ok(backup)
        }
        Err(error) => {
            if let Some(backup_path) = backup.as_ref() {
                if let Err(restore) = Database::restore_replaced_file(db_path, backup_path) {
                    return Err(AppError::partial_failure(
                        "导入后无法重新打开数据库，且回滚失败，请核对数据库文件",
                    )
                    .with_detail(format!("{error:#}；回滚失败：{restore:#}")));
                }
                *db = Database::open(db_path).map_err(|restore| {
                    AppError::partial_failure("导入失败，已回滚但无法重新打开数据库").with_detail(
                        format!("{error:#}；重新打开回滚后的数据库失败：{restore:#}"),
                    )
                })?;
            }
            Err(AppError::from(error).with_context("导入后重新打开数据库失败"))
        }
    }
}

fn rollback_import_state(
    db_path: &std::path::Path,
    db: &mut Database,
    backup_path: Option<&std::path::Path>,
    snapshots: &[switch_api::adapters::FileSnapshot],
    skills: Option<(
        &std::path::Path,
        &switch_api::utils::skills_backup::SkillsImportResult,
    )>,
) -> Result<(), String> {
    let mut errors = Vec::new();

    if let Err(error) = switch_api::adapters::restore_snapshots(snapshots) {
        errors.push(format!("恢复工具配置失败: {error:#}"));
    }
    if let Some((home, result)) = skills {
        if let Err(error) = switch_api::utils::skills_backup::remove_restored_skills(home, result) {
            errors.push(format!("恢复 Skills 失败: {error:#}"));
        }
    }

    let placeholder =
        Database::open(":memory:").map_err(|error| format!("创建数据库占位连接失败: {error}"))?;
    let previous = std::mem::replace(db, placeholder);
    drop(previous);

    match backup_path {
        Some(backup_path) => {
            if let Err(error) = Database::restore_replaced_file(db_path, backup_path) {
                errors.push(format!("恢复数据库文件失败: {error:#}"));
            }
        }
        None => {
            if let Err(error) = std::fs::remove_file(db_path) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    errors.push(format!("删除导入失败的数据库失败: {error}"));
                }
            }
        }
    }
    let _ = std::fs::remove_file(format!("{}-wal", db_path.display()));
    let _ = std::fs::remove_file(format!("{}-shm", db_path.display()));

    match Database::open(db_path) {
        Ok(restored) => *db = restored,
        Err(error) => errors.push(format!("重新打开回滚后的数据库失败: {error:#}")),
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("；"))
    }
}

/// `import_after_apply_failure` 是纯函数（不改磁盘、不碰数据库），
/// 所以两个分支都能直接测——这正是把它从 `import_database` /
/// `import_portable_backup` 里抽出来的原因：留在命令里就只能靠集成测试碰运气。
#[cfg(test)]
mod import_failure_tests {
    use super::import_after_apply_failure;
    use switch_api::error::ErrorKind;

    fn io_error() -> anyhow::Error {
        anyhow::Error::from(std::io::Error::from(std::io::ErrorKind::NotFound))
    }

    #[test]
    fn a_clean_rollback_keeps_the_underlying_kind_instead_of_partial_failure() {
        // 回滚成功 = 磁盘已复原，用户什么都不用做。此时报「部分生效」会误导用户
        // 去核对工具状态，而实际上什么都没落地。类别应沿用底层错误的真实类型。
        let err = import_after_apply_failure("恢复生效配置", io_error(), Ok(()));

        assert_eq!(
            err.kind(),
            ErrorKind::Io,
            "已回滚的失败要保留底层类别（io），不能被改写成 PartialFailure"
        );
        assert!(
            err.message.contains("已回滚到导入前状态"),
            "文案要告诉用户「没留下痕迹」，实际为：{}",
            err.message
        );
        assert!(
            err.message.contains("恢复生效配置"),
            "文案要说清是哪一步失败，实际为：{}",
            err.message
        );
    }

    #[test]
    fn a_failed_rollback_is_partial_failure_and_keeps_both_errors_in_detail() {
        let err = import_after_apply_failure(
            "恢复生效配置",
            io_error(),
            Err("恢复工具配置失败: 权限不足".to_string()),
        );

        assert_eq!(
            err.kind(),
            ErrorKind::PartialFailure,
            "回滚也失败才叫「部分生效」"
        );
        assert!(
            err.message.contains("请核对"),
            "部分生效必须给出可行动的下一步，实际为：{}",
            err.message
        );
        let detail = err.detail.as_deref().expect("detail 必须保留原始错误");
        assert!(
            detail.contains("恢复工具配置失败"),
            "detail 要带上回滚错误，实际为：{detail}"
        );
    }

    #[test]
    fn a_failed_rollback_does_not_claim_the_state_was_restored() {
        // 回归保护：两个分支曾共用同一句文案「Database imported but ...」，
        // 回滚失败时也在说「导入已生效」，用户无法判断该不该动手。
        let err = import_after_apply_failure("恢复 Skills", io_error(), Err("磁盘只读".into()));
        assert!(
            !err.message.contains("已回滚到导入前状态"),
            "回滚失败时绝不能声称已复原，实际为：{}",
            err.message
        );
    }
}

/// 这是本次迁移改动最激进的一段：占位连接 → 替换文件 → 重新打开 → 失败时还原。
/// 纯函数单测覆盖不到它，只跑「编译通过」也不算验证——必须拿真实 sqlite 文件
/// 跑一遍，才能确认换库和失败恢复都还对。
#[cfg(test)]
mod replace_database_tests {
    use super::*;
    use switch_api::error::ErrorKind;
    use switch_api::models::{ApiProfile, TargetApp};

    fn named_profile(name: &str) -> ApiProfile {
        ApiProfile {
            name: name.into(),
            provider: "custom".into(),
            api_url: "https://x.example".into(),
            api_key: "k".into(),
            target_app: Some(TargetApp::Codex),
            ..Default::default()
        }
    }

    /// 建一个含若干 profile 的库，返回其路径。
    fn make_db(path: &std::path::Path, names: &[&str]) -> std::path::PathBuf {
        let db = Database::open(path).expect("open");
        for name in names {
            db.add_profile(&named_profile(name)).expect("add_profile");
        }
        drop(db);
        path.to_path_buf()
    }

    fn profile_names(db: &Database) -> Vec<String> {
        db.list_profiles()
            .expect("list_profiles")
            .into_iter()
            .map(|p| p.name)
            .collect()
    }

    #[test]
    fn swapping_in_an_import_keeps_the_new_content_and_returns_a_usable_backup() {
        let dir = tempfile::tempdir().expect("tempdir");
        let live = dir.path().join("live.sqlite");
        let incoming = dir.path().join("incoming.sqlite");
        make_db(&live, &["old"]);
        make_db(&incoming, &["new-a", "new-b"]);

        let mut db = Database::open(&live).expect("open live");
        let backup = replace_database_locked(&incoming, &live, &mut db).expect("导入应成功");

        // 换进来的是候选库的内容
        let names = profile_names(&db);
        assert_eq!(names.len(), 2, "实际：{names:?}");
        assert!(names.contains(&"new-a".to_string()), "实际：{names:?}");

        // 原库被快照成备份，且文件真实存在、内容确为替换前的样子——
        // 只返回一个路径但文件是空的，回滚能力就是假的。
        let backup = backup.expect("原库存在时必须返回备份路径");
        assert!(
            backup.exists(),
            "备份文件必须真实存在：{}",
            backup.display()
        );
        let backup_db = Database::open(&backup).expect("备份库必须可打开");
        assert_eq!(profile_names(&backup_db), vec!["old".to_string()]);
    }

    #[test]
    fn a_missing_import_file_fails_cleanly_and_leaves_the_live_db_usable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let live = dir.path().join("live.sqlite");
        make_db(&live, &["old"]);

        let mut db = Database::open(&live).expect("open live");
        let err = replace_database_locked(&dir.path().join("nope.sqlite"), &live, &mut db)
            .expect_err("不存在的候选库必须失败");

        assert_ne!(
            err.kind(),
            ErrorKind::PartialFailure,
            "替换前就中止、且原库能重新打开，不算「部分生效」"
        );
        // 关键：db 句柄必须被重新打开回真实库，而不是停在 :memory: 占位连接上。
        // 停在占位连接上会让后续命令读到空库——这是最难在人工点测里发现的失败形态。
        assert_eq!(
            profile_names(&db),
            vec!["old".to_string()],
            "原库内容必须仍然可读"
        );
    }
}
