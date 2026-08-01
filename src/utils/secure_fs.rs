use anyhow::{Context, Result};
use std::fs;
use std::io::Write;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

pub fn ensure_private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)
        .with_context(|| format!("Failed to create private directory {}", path.display()))?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("Failed to secure directory {}", path.display()))?;
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
    use super::{atomic_write_private, copy_private, ensure_private_dir, ensure_private_file};
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
}
