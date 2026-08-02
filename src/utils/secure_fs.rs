use anyhow::{Context, Result};
use std::fs;
use std::io::Write;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

pub fn ensure_private_dir(path: &Path) -> Result<()> {
    // `Path::parent()` 对裸文件名返回 Some("")，调用方难以逐个防御；空路径按当前目录处理。
    let path = if path.as_os_str().is_empty() {
        Path::new(".")
    } else {
        path
    };
    fs::create_dir_all(path)
        .with_context(|| format!("Failed to create private directory {}", path.display()))?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("Failed to secure directory {}", path.display()))?;
    Ok(())
}

/// 收紧导出目标文件自身的权限，但**不触碰其父目录**。
/// 导出路径由用户选择（如 `~/Desktop`），`ensure_private_dir` 会把该目录改成 0700，
/// 属于越权副作用；导出只应保证文件本身 owner-only。
pub fn secure_export_file(path: &Path) -> Result<()> {
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("Failed to secure exported file {}", path.display()))?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

pub fn ensure_private_file(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("Failed to secure file {}", path.display()))?;
    Ok(())
}

pub fn atomic_write_private(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Path has no parent: {}", path.display()))?;
    ensure_private_dir(parent)?;

    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("Failed to create temp file for {}", path.display()))?;
    #[cfg(unix)]
    temp.as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))
        .with_context(|| format!("Failed to secure temp file for {}", path.display()))?;
    temp.write_all(contents)
        .with_context(|| format!("Failed to write temp file for {}", path.display()))?;
    temp.as_file()
        .sync_all()
        .with_context(|| format!("Failed to sync temp file for {}", path.display()))?;
    temp.persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("Failed to replace {}", path.display()))?;
    ensure_private_file(path)?;
    Ok(())
}

pub fn copy_private(source: &Path, destination: &Path) -> Result<u64> {
    if let Some(parent) = destination.parent() {
        ensure_private_dir(parent)?;
    }
    let bytes = fs::copy(source, destination).with_context(|| {
        format!(
            "Failed to copy {} to {}",
            source.display(),
            destination.display()
        )
    })?;
    ensure_private_file(destination)?;
    Ok(bytes)
}

// 权限测试仅 Unix 有意义（Windows 无 POSIX mode），整体在 Windows 上不编译。
#[cfg(all(test, unix))]
mod tests {
    use super::{
        atomic_write_private, copy_private, ensure_private_dir, ensure_private_file,
        secure_export_file,
    };
    use anyhow::Result;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[cfg(unix)]
    #[test]
    fn private_write_and_copy_use_owner_only_modes() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let private_dir = dir.path().join("private");
        ensure_private_dir(&private_dir)?;
        assert_eq!(
            fs::metadata(&private_dir)?.permissions().mode() & 0o777,
            0o700
        );

        let source = private_dir.join("source.json");
        atomic_write_private(&source, br#"{"key":"secret"}"#)?;
        assert_eq!(fs::metadata(&source)?.permissions().mode() & 0o777, 0o600);

        let destination = private_dir.join("backup.json");
        copy_private(&source, &destination)?;
        assert_eq!(
            fs::metadata(&destination)?.permissions().mode() & 0o777,
            0o600
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn ensure_private_file_repairs_existing_mode() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("credentials.db");
        fs::write(&path, b"secret")?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644))?;
        ensure_private_file(&path)?;
        assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o777, 0o600);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn secure_export_file_leaves_parent_directory_untouched() -> Result<()> {
        let dir = tempfile::tempdir()?;
        // 模拟用户目录（如 ~/Desktop）：0755，不应被导出流程收紧。
        let user_dir = dir.path().join("Desktop");
        fs::create_dir(&user_dir)?;
        fs::set_permissions(&user_dir, fs::Permissions::from_mode(0o755))?;

        let exported = user_dir.join("helio-backup.db");
        fs::write(&exported, b"snapshot")?;
        fs::set_permissions(&exported, fs::Permissions::from_mode(0o644))?;
        secure_export_file(&exported)?;

        assert_eq!(
            fs::metadata(&exported)?.permissions().mode() & 0o777,
            0o600,
            "导出文件应收紧为 owner-only"
        );
        assert_eq!(
            fs::metadata(&user_dir)?.permissions().mode() & 0o777,
            0o755,
            "导出不应改动用户选择的目标目录权限"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn copy_private_tightens_parent_directory() -> Result<()> {
        // 记录既有行为：copy_private 会把目标父目录设为 0700，
        // 因此它只适用于应用私有目录，不可用于用户选择的导出路径。
        let dir = tempfile::tempdir()?;
        let nested = dir.path().join("private-store");
        fs::create_dir(&nested)?;
        fs::set_permissions(&nested, fs::Permissions::from_mode(0o755))?;

        let source = dir.path().join("source.db");
        fs::write(&source, b"x")?;
        copy_private(&source, &nested.join("copy.db"))?;

        assert_eq!(fs::metadata(&nested)?.permissions().mode() & 0o777, 0o700);
        Ok(())
    }

    #[test]
    fn ensure_private_dir_accepts_empty_path_as_current_dir() -> Result<()> {
        // `Path::new("live.sqlite").parent()` 是 Some("")，不能当作错误。
        ensure_private_dir(std::path::Path::new(""))?;
        Ok(())
    }
}
