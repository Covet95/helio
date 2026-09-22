//! 私有文件/目录的权限收紧。
//!
//! 这些路径装着明文 API key（数据库、备份、journal、导出文件），必须只对
//! 当前用户可读。
//!
//! ## 跨平台实现
//!
//! - **Unix**：POSIX mode（目录 `0700`、文件 `0600`）。
//! - **Windows**：POSIX mode 不存在，改用 ACL——把对象的 DACL 替换为
//!   「仅当前用户完全控制」，并置 `PROTECTED` 标志阻止继承来的宽松 ACE。
//!
//! 早期实现只在 Unix 上生效，Windows 分支是 `let _ = path;` 空操作——而项目
//! 分发 `.exe`，等于凭据文件在 Windows 上毫无保护。这是本模块存在的意义。

use anyhow::{Context, Result};
use std::fs;
use std::io::Write;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// 把路径的访问权限收紧为「仅当前用户」。
///
/// `is_dir` 影响继承语义（目录需要让新建的子项继承限制），Unix 下无差别。
#[cfg(unix)]
fn restrict_to_owner(path: &Path, mode: u32, _is_dir: bool) -> Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .with_context(|| format!("Failed to secure {}", path.display()))
}

/// Windows：用 ACL 把 DACL 替换为「仅当前用户」。
#[cfg(windows)]
fn restrict_to_owner(path: &Path, _mode: u32, is_dir: bool) -> Result<()> {
    windows_acl::restrict_to_owner(path, is_dir)
        .with_context(|| format!("Failed to secure {}", path.display()))
}

pub fn ensure_private_dir(path: &Path) -> Result<()> {
    // `Path::parent()` 对裸文件名返回 Some("")，调用方难以逐个防御；空路径按当前目录处理。
    let path = if path.as_os_str().is_empty() {
        Path::new(".")
    } else {
        path
    };
    fs::create_dir_all(path)
        .with_context(|| format!("Failed to create private directory {}", path.display()))?;
    restrict_to_owner(path, 0o700, true)?;
    Ok(())
}

/// 收紧导出目标文件自身的权限，但**不触碰其父目录**。
/// 导出路径由用户选择（如 `~/Desktop`），`ensure_private_dir` 会把该目录改成 0700，
/// 属于越权副作用；导出只应保证文件本身 owner-only。
pub fn secure_export_file(path: &Path) -> Result<()> {
    restrict_to_owner(path, 0o600, false)?;
    Ok(())
}

pub fn ensure_private_file(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    restrict_to_owner(path, 0o600, false)?;
    Ok(())
}

/// Windows ACL 后端。独立成模块便于集中说明平台细节。
#[cfg(windows)]
mod windows_acl {
    use anyhow::{bail, Result};
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows_sys::Win32::Security::Authorization::{
        SetEntriesInAclW, SetNamedSecurityInfoW, EXPLICIT_ACCESS_W, GRANT_ACCESS, SE_FILE_OBJECT,
        TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
    };
    use windows_sys::Win32::Security::{
        GetTokenInformation, TokenUser, ACL, DACL_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, SUB_CONTAINERS_AND_OBJECTS_INHERIT, TOKEN_QUERY,
        TOKEN_USER,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    // 全部取自 windows-sys 的具名常量，**不手抄数值**。
    //
    // 教训：这里原先手写了 `const GRANT_ACCESS: i32 = 0;`，而真值是 1
    // （0 是 `NOT_USED_ACCESS`）。`SetEntriesInAclW` 收到非法模式直接失败，
    // 于是 Windows 上**所有写盘路径全部瘫痪**——而 CI 的 Windows job 才是
    // 唯一能发现它的地方。手抄常量等于把 SDK 的取值背错一次就全线崩溃。

    /// 把 `path` 的 DACL 换成「仅当前用户完全控制」。
    ///
    /// 置 `PROTECTED_DACL_SECURITY_INFORMATION` 很关键：不置的话，父目录继承
    /// 下来的宽松 ACE（如 `Users` 组可读）仍会留在 DACL 上，收紧形同虚设。
    pub(super) fn restrict_to_owner(path: &Path, is_dir: bool) -> Result<()> {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();

        // SAFETY: 以下全部是 Win32 API 调用，参数按文档要求构造；缓冲区在
        // 使用期间存活，指针指向的 SID 由 token 缓冲区持有。
        unsafe {
            // 1) 取当前进程 token 的用户 SID。
            let mut token = std::mem::zeroed();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                bail!("打开进程令牌失败");
            }

            // 先问长度，再按长度分配——TOKEN_USER 大小随 SID 长度变化。
            let mut size = 0u32;
            GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut size);
            if size == 0 {
                bail!("查询令牌信息时未能取得缓冲区大小");
            }
            let mut buf = vec![0u8; size as usize];
            if GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), size, &mut size) == 0
            {
                bail!("查询令牌信息失败");
            }
            let user: &TOKEN_USER = &*buf.as_ptr().cast();

            // 2) 构造「仅该用户」的 ACE。
            let mut access: EXPLICIT_ACCESS_W = std::mem::zeroed();
            access.grfAccessPermissions = FILE_ALL_ACCESS;
            access.grfAccessMode = GRANT_ACCESS;
            access.grfInheritance = if is_dir {
                SUB_CONTAINERS_AND_OBJECTS_INHERIT
            } else {
                0
            };
            access.Trustee = TRUSTEE_W {
                pMultipleTrustee: std::ptr::null_mut(),
                MultipleTrusteeOperation: 0,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_USER,
                ptstrName: user.User.Sid.cast(),
            };

            // 3) 生成只含该 ACE 的 ACL（不合并继承项，父目录的宽松 ACE 就此消失）。
            let mut acl: *mut ACL = std::ptr::null_mut();
            if SetEntriesInAclW(1, &access, std::ptr::null_mut(), &mut acl) != 0 {
                bail!("构造 ACL 失败");
            }

            // 4) 写回并置 PROTECTED，阻止继承覆盖。
            let rc = SetNamedSecurityInfoW(
                wide.as_ptr() as *mut u16,
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                acl,
                std::ptr::null_mut(),
            );
            if rc != 0 {
                bail!("设置文件 ACL 失败（错误码 {rc}）");
            }
        }

        Ok(())
    }
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

// 测试在**所有平台**编译：Windows 的 ACL 分支必须被真正执行过。
//
// 事故背景：这些测试原先整体 `#[cfg(all(test, unix))]`，于是 Windows 的 ACL
// 实现从未被任何测试碰过——把 GRANT_ACCESS 抄成 0 这种错误一路绿灯到用户手里。
// 平台特有的断言各自加 `#[cfg(unix)]`，模块本身不再按平台排除。
#[cfg(test)]
mod tests {
    use super::{ensure_private_dir, restrict_to_owner};
    #[cfg(unix)]
    use super::{atomic_write_private, copy_private, ensure_private_file, secure_export_file};
    use anyhow::Result;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    /// 收紧权限的**跨平台**冒烟测试。
    ///
    /// 这条存在的理由是一次真实事故：Windows 的 ACL 实现把 `GRANT_ACCESS`
    /// 手抄成了 0（真值 1），`SetEntriesInAclW` 收到非法模式直接失败，
    /// 于是 Windows 上所有写盘路径全部瘫痪——而当时 `secure_fs` 的测试
    /// **全是 `#[cfg(unix)]`**，本地与 macOS CI 一路绿灯。
    ///
    /// 本测试不加 cfg，在哪个平台就跑哪个平台的分支，确保 ACL 代码
    /// 至少被真正执行过一次。
    #[test]
    fn restrict_to_owner_works_on_this_platform() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("secret.txt");
        fs::write(&file, b"x")?;

        restrict_to_owner(&file, 0o600, false)
            .map_err(|e| anyhow::anyhow!("文件权限收紧失败（{e:#}）"))?;

        let sub = dir.path().join("subdir");
        fs::create_dir_all(&sub)?;
        restrict_to_owner(&sub, 0o700, true)
            .map_err(|e| anyhow::anyhow!("目录权限收紧失败（{e:#}）"))?;

        // 收紧后仍可正常读写——权限设置不该把文件锁死。
        fs::write(&file, b"y")?;
        assert_eq!(fs::read(&file)?, b"y");
        Ok(())
    }

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
