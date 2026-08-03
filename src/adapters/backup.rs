//! 配置备份公共逻辑：微秒时间戳命名 + 按前缀清理。
//! 各 adapter 的 `backup_config` / `cleanup_old_backups` 均委托到此模块，
//! 避免 6 处重复实现且行为不一（如 `%Y%m%d_%H%M%S` 秒级时间戳同秒互覆盖）。

use crate::utils::secure_fs::atomic_write_private;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// 生成备份时间戳。含微秒，避免同一秒内多次备份互相覆盖。
pub fn stamp() -> String {
    chrono::Local::now().format("%Y%m%d_%H%M%S_%f").to_string()
}

/// 备份单个文件为 `{config_dir}/{label}.backup.{stamp}.{ext}`。
/// 文件不存在时返回 `Ok(None)`（调用方决定是否继续）。
///
/// 备份走「临时文件 + rename」原子写入：进程在备份中途崩溃不会留下
/// 截断的备份文件（截断备份若 mtime 最新会被清理逻辑误保留、挤掉好备份）。
pub fn backup_one(config_dir: &Path, src: &Path, label: &str) -> Result<Option<PathBuf>> {
    if !src.exists() {
        return Ok(None);
    }
    let stamp = stamp();
    let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("bak");
    let backup_path = config_dir.join(format!("{label}.backup.{stamp}.{ext}"));
    let bytes = fs::read(src).with_context(|| format!("Failed to read {}", src.display()))?;
    atomic_write_private(&backup_path, &bytes)
        .with_context(|| format!("Failed to backup {}", src.display()))?;
    Ok(Some(backup_path))
}

/// 主配置文件必须存在：不存在时直接报错（语义与原实现一致）。
pub fn backup_required(config_dir: &Path, src: &Path, label: &str) -> Result<PathBuf> {
    backup_one(config_dir, src, label)?.ok_or_else(|| anyhow::anyhow!("Config file does not exist"))
}

/// 清理 `config_dir` 下以 `prefix` 开头的备份文件，保留最近 `keep` 个（按备份时间）。
///
/// 排序优先解析文件名内嵌的 `%Y%m%d_%H%M%S[_%f]` 时间戳——文件名时间不受
/// mtime 被同步工具改写或系统时钟回拨的影响；解析失败退回文件 mtime；
/// 仍失败（metadata 异常）则视为「最新」予以保留，避免误删可能完好的备份。
/// 删除失败不再静默：返回错误让调用方知晓，防止旧备份无限堆积。
pub fn cleanup_prefix(config_dir: &Path, prefix: &str, keep: usize) -> Result<()> {
    if !config_dir.exists() {
        return Ok(());
    }
    let mut backups: Vec<(PathBuf, SystemTime)> = Vec::new();
    for entry in fs::read_dir(config_dir)? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with(prefix) {
            continue;
        }
        let path = entry.path();
        backups.push((path.clone(), backup_time(&name, &path)));
    }

    backups.sort_by_key(|b| std::cmp::Reverse(b.1));

    for (path, _) in backups.iter().skip(keep) {
        fs::remove_file(path)
            .with_context(|| format!("Failed to remove old backup {}", path.display()))?;
    }

    Ok(())
}

/// 备份文件信息（`list_backups` 的返回项）。
pub struct BackupInfo {
    pub path: PathBuf,
    /// 备份时间（解析文件名内嵌时间戳，解析失败退回文件 mtime）。
    pub time: SystemTime,
    /// 恢复该备份时将写回的目标配置文件（`{config_dir}/{label}.{ext}`）。
    /// 文件名无法解析出目标时（格式异常）为 `None`，仍会列出但不可恢复。
    pub target: Option<PathBuf>,
}

/// 备份文件名格式常量：`{label}.backup.{stamp}.{ext}`。
const MARKER: &str = ".backup.";

/// 解析备份文件名，返回 `(label, stamp, ext)`。格式不符返回 `None`。
fn parse_backup_name(name: &str) -> Option<(String, String, String)> {
    let idx = name.find(MARKER)?;
    let label = &name[..idx];
    if label.is_empty() {
        return None;
    }
    let rest = &name[idx + MARKER.len()..];
    let dot = rest.rfind('.')?;
    let (stamp, ext) = (&rest[..dot], &rest[dot + 1..]);
    if stamp.is_empty() || ext.is_empty() {
        return None;
    }
    Some((label.to_string(), stamp.to_string(), ext.to_string()))
}

/// 列出 `config_dir` 下全部 `*.backup.*` 配置备份，按时间新→旧排序。
pub fn list_backups(config_dir: &Path) -> Result<Vec<BackupInfo>> {
    if !config_dir.exists() {
        return Ok(Vec::new());
    }
    let mut backups: Vec<BackupInfo> = Vec::new();
    for entry in fs::read_dir(config_dir)? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.contains(MARKER) {
            continue;
        }
        let path = entry.path();
        let target = parse_backup_name(&name)
            .map(|(label, _, ext)| config_dir.join(format!("{label}.{ext}")));
        backups.push(BackupInfo {
            path,
            time: backup_time(&name, &entry.path()),
            target,
        });
    }
    backups.sort_by_key(|b| std::cmp::Reverse(b.time));
    Ok(backups)
}

/// 恢复 `backup_path` 到其对应的 live 配置文件并原子写回，返回写回的路径。
///
/// 安全约束：
/// - 备份文件必须位于 `config_dir` 内（防恢复任意路径文件）；
/// - 文件名必须匹配 `{label}.backup.{stamp}.{ext}` 且时间戳为数字/下划线，
///   目标路径完全由文件名推导（`{label}.{ext}`），不存在任意路径写；
/// - 覆盖前先对当前 live 配置做一次备份（走 `backup_one` 原子备份），
///   避免恢复操作本身成为数据丢失来源。
pub fn restore_backup(config_dir: &Path, backup_path: &Path) -> Result<PathBuf> {
    let dir = config_dir
        .canonicalize()
        .with_context(|| format!("配置目录不存在: {}", config_dir.display()))?;
    let backup_path = validate_backup_path(&dir, backup_path)?;
    let name = backup_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("无效的备份文件名: {}", backup_path.display()))?;
    let (label, _, ext) = parse_valid_backup_name(name)
        .ok_or_else(|| anyhow::anyhow!("不是有效的备份文件名: {name}"))?;
    restore_backup_to_validated(
        &dir,
        &backup_path,
        &dir.join(format!("{label}.{ext}")),
        &label,
    )
}

/// 恢复到已知的显式目标路径。用于备份名无法表达真实嵌套路径的受管文件。
///
/// 调用方必须提供配置目录内的目标路径；备份文件的归属与命名仍会完整校验。
pub fn restore_backup_to(config_dir: &Path, backup_path: &Path, target: &Path) -> Result<PathBuf> {
    let dir = config_dir
        .canonicalize()
        .with_context(|| format!("配置目录不存在: {}", config_dir.display()))?;
    let backup_path = validate_backup_path(&dir, backup_path)?;
    let name = backup_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("无效的备份文件名: {}", backup_path.display()))?;
    let (label, _, _) = parse_valid_backup_name(name)
        .ok_or_else(|| anyhow::anyhow!("不是有效的备份文件名: {name}"))?;
    let target = if target.is_absolute() {
        target
            .strip_prefix(config_dir)
            .map(|relative| dir.join(relative))
            .unwrap_or_else(|_| target.to_path_buf())
    } else {
        dir.join(target)
    };
    if !target.starts_with(&dir) {
        anyhow::bail!("恢复目标不在配置目录内: {}", target.display());
    }
    restore_backup_to_validated(&dir, &backup_path, &target, &label)
}

fn restore_backup_to_validated(
    dir: &Path,
    backup_path: &Path,
    target: &Path,
    label: &str,
) -> Result<PathBuf> {
    let bytes = fs::read(backup_path)
        .with_context(|| format!("Failed to read backup {}", backup_path.display()))?;
    backup_one(dir, target, label)?;
    atomic_write_private(target, &bytes)
        .with_context(|| format!("Failed to restore {}", target.display()))?;
    Ok(target.to_path_buf())
}

fn validate_backup_path(dir: &Path, backup_path: &Path) -> Result<PathBuf> {
    let backup_path = backup_path
        .canonicalize()
        .with_context(|| format!("备份文件不存在: {}", backup_path.display()))?;
    if !backup_path.starts_with(dir) {
        anyhow::bail!("备份文件不在配置目录内: {}", backup_path.display());
    }
    Ok(backup_path)
}

fn parse_valid_backup_name(name: &str) -> Option<(String, String, String)> {
    let (label, stamp, ext) = parse_backup_name(name)?;
    if stamp.is_empty() || !stamp.chars().all(|c| c.is_ascii_digit() || c == '_') {
        return None;
    }
    Some((label, stamp, ext))
}

/// 判定备份文件的时间（见 [`cleanup_prefix`] 的说明）。
fn backup_time(name: &str, path: &Path) -> SystemTime {
    if let Some(idx) = name.find(MARKER) {
        let stamp_part = &name[idx + MARKER.len()..];
        // 去掉扩展名：时间戳是扩展名之前的部分。
        let stamp_part = match stamp_part.rfind('.') {
            Some(dot) => &stamp_part[..dot],
            None => stamp_part,
        };
        for format in ["%Y%m%d_%H%M%S_%f", "%Y%m%d_%H%M%S"] {
            if let Ok(parsed) = chrono::NaiveDateTime::parse_from_str(stamp_part, format) {
                return parsed.and_utc().into();
            }
        }
    }
    fs::metadata(path)
        .and_then(|m| m.modified())
        // 无法判定时间的按「最新」处理：排序时排最前，保留而非删除。
        .unwrap_or(SystemTime::now())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> PathBuf {
        let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "switch-api-backup-test-{}-{unique}",
            std::process::id()
        ))
    }

    #[test]
    fn backup_one_skips_missing_file() {
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        let missing = dir.join("nope.json");
        assert!(backup_one(&dir, &missing, "settings").unwrap().is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn backup_required_errors_on_missing() {
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        let missing = dir.join("nope.json");
        assert!(backup_required(&dir, &missing, "settings").is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn backup_one_uses_subsecond_stamp_and_preserves_ext() {
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        let src = dir.join("config.toml");
        fs::write(&src, b"x").unwrap();
        let first = backup_one(&dir, &src, "config").unwrap().unwrap();
        let second = backup_one(&dir, &src, "config").unwrap().unwrap();
        assert_ne!(first, second, "同秒两次备份不应互相覆盖");
        assert!(first.to_string_lossy().ends_with(".toml"));
        assert!(first.to_string_lossy().contains("config.backup."));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cleanup_prefix_keeps_only_newest() {
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        // mtime 故意设成与文件名时间戳相反：按文件名时间戳排序保留，不受 mtime 影响。
        let base = std::time::Duration::from_secs(1_700_000_000);
        for i in 0..3 {
            let name = format!("config.backup.20260101_000000_00000{i}.toml");
            fs::write(dir.join(&name), b"x").unwrap();
            let f = fs::File::options()
                .write(true)
                .open(dir.join(&name))
                .unwrap();
            // mtime 倒序(名字最新的 mtime 最旧)，验证清理按文件名时间而非 mtime
            f.set_modified(std::time::UNIX_EPOCH + base + std::time::Duration::from_secs(10 - i))
                .unwrap();
        }
        fs::write(dir.join("auth.backup.20260101_000000_000000.json"), b"x").unwrap();
        cleanup_prefix(&dir, "config.backup.", 2).unwrap();
        let remaining: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with("config.backup."))
            .collect();
        assert_eq!(remaining.len(), 2, "应只保留最近 2 个 config 备份");
        // 文件名时间戳最旧（i=0）的最先被清掉（尽管它的 mtime 最新）
        assert!(!remaining.contains(&"config.backup.20260101_000000_000000.toml".to_string()));
        // auth 前缀不受影响
        assert!(dir.join("auth.backup.20260101_000000_000000.json").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_backups_sorted_newest_first() {
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("settings.backup.20260101_000000_000001.json"),
            b"a",
        )
        .unwrap();
        fs::write(
            dir.join("settings.backup.20260102_000000_000000.json"),
            b"b",
        )
        .unwrap();
        fs::write(dir.join("settings.json"), b"live").unwrap();
        let backups = list_backups(&dir).unwrap();
        assert_eq!(backups.len(), 2, "live 文件不应被列出");
        assert!(backups[0].time >= backups[1].time);
        assert!(backups[0]
            .path
            .to_string_lossy()
            .contains("20260102_000000"));
        assert_eq!(
            backups[0].target.as_ref().unwrap(),
            &dir.join("settings.json"),
            "目标应由文件名推导为 settings.json"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_backups_empty_when_dir_missing() {
        let dir = temp_dir().join("does-not-exist");
        assert!(list_backups(&dir).unwrap().is_empty());
    }

    #[test]
    fn restore_backup_writes_live_file() {
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("settings.json"), b"current").unwrap();
        let backup = dir.join("settings.backup.20260101_000000_000001.json");
        fs::write(&backup, b"restored").unwrap();
        let restored = restore_backup(&dir, &backup).unwrap();
        assert_eq!(
            restored,
            dir.canonicalize().unwrap().join("settings.json"),
            "目标应为 settings.json（canonicalize 后 /var → /private/var）"
        );
        assert_eq!(fs::read(&restored).unwrap(), b"restored");
        // 恢复前自动备份了当前 live 配置
        let current_backups: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with("settings.backup."))
            .collect();
        assert_eq!(current_backups.len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_backup_to_writes_nested_target() {
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join("agents/main/agent/models.json");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, b"current").unwrap();
        let backup = dir.join("models.backup.20260101_000000_000001.json");
        fs::write(&backup, b"restored").unwrap();

        let restored = restore_backup_to(&dir, &backup, &target).unwrap();
        assert_eq!(restored, target.canonicalize().unwrap());
        assert_eq!(fs::read(&target).unwrap(), b"restored");
        assert!(!dir.join("models.json").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_backup_rejects_outside_dir() {
        let dir = temp_dir();
        let outside = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let backup = outside.join("settings.backup.20260101_000000_000001.json");
        fs::write(&backup, b"x").unwrap();
        assert!(restore_backup(&dir, &backup).is_err());
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&outside);
    }

    #[test]
    fn restore_backup_rejects_non_backup_name() {
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        let live = dir.join("settings.json");
        fs::write(&live, b"x").unwrap();
        // 无 .backup. 标记
        assert!(restore_backup(&dir, &live).is_err());
        // 时间戳含非数字字符
        let evil = dir.join("settings.backup.evil.json");
        fs::write(&evil, b"x").unwrap();
        assert!(restore_backup(&dir, &evil).is_err());
        assert!(!dir.join("evil.json").exists(), "非法文件名不应产生写盘");
        let _ = fs::remove_dir_all(&dir);
    }
}
