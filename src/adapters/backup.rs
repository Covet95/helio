//! 配置备份公共逻辑：微秒时间戳命名 + 按前缀清理。
//! 各 adapter 的 `backup_config` / `cleanup_old_backups` 均委托到此模块，
//! 避免 6 处重复实现且行为不一（如 `%Y%m%d_%H%M%S` 秒级时间戳同秒互覆盖）。

use crate::utils::secure_fs::copy_private;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// 生成备份时间戳。含微秒，避免同一秒内多次备份互相覆盖。
pub fn stamp() -> String {
    chrono::Local::now().format("%Y%m%d_%H%M%S_%f").to_string()
}

/// 备份单个文件为 `{config_dir}/{label}.backup.{stamp}.{ext}`。
/// 文件不存在时返回 `Ok(None)`（调用方决定是否继续）。
pub fn backup_one(config_dir: &Path, src: &Path, label: &str) -> Result<Option<PathBuf>> {
    if !src.exists() {
        return Ok(None);
    }
    let stamp = stamp();
    let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("bak");
    let backup_path = config_dir.join(format!("{label}.backup.{stamp}.{ext}"));
    copy_private(src, &backup_path)
        .with_context(|| format!("Failed to backup {}", src.display()))?;
    Ok(Some(backup_path))
}

/// 主配置文件必须存在：不存在时直接报错（语义与原实现一致）。
pub fn backup_required(config_dir: &Path, src: &Path, label: &str) -> Result<PathBuf> {
    backup_one(config_dir, src, label)?.ok_or_else(|| anyhow::anyhow!("Config file does not exist"))
}

/// 清理 `config_dir` 下以 `prefix` 开头的备份文件，保留最近 `keep` 个（按修改时间）。
pub fn cleanup_prefix(config_dir: &Path, prefix: &str, keep: usize) -> Result<()> {
    if !config_dir.exists() {
        return Ok(());
    }
    let mut backups: Vec<_> = fs::read_dir(config_dir)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(prefix))
        .collect();

    backups.sort_by_key(|entry| {
        entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(UNIX_EPOCH)
    });
    backups.reverse();

    for entry in backups.iter().skip(keep) {
        let _ = fs::remove_file(entry.path());
    }

    Ok(())
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
        // 设置不同的 mtime，保证"保留最近 keep 个"的判定确定
        let base = std::time::Duration::from_secs(1_700_000_000);
        for i in 0..3 {
            let name = format!("config.backup.20260101_000000_00000{i}.toml");
            fs::write(dir.join(&name), b"x").unwrap();
            let f = fs::File::options()
                .write(true)
                .open(dir.join(&name))
                .unwrap();
            f.set_modified(UNIX_EPOCH + base + std::time::Duration::from_secs(i))
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
        // mtime 最小（i=0）的最先被清掉
        assert!(!remaining.contains(&"config.backup.20260101_000000_000000.toml".to_string()));
        // auth 前缀不受影响
        assert!(dir.join("auth.backup.20260101_000000_000000.json").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
