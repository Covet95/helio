//! 剪贴板写入：纯 OS 集成，与业务无关。
//!
//! 三平台各一套子进程实现（macOS `pbcopy` / Windows PowerShell `Set-Clipboard`
//! / Linux `wl-copy`|`xclip`）。正文一律走 **stdin**，不拼进命令行参数——
//! 既避免命令注入，也免去转义问题。

use crate::commands::AppError;
use std::io::Write;
use std::process::{Command, Stdio};

#[tauri::command]
pub async fn copy_text(text: String) -> Result<(), AppError> {
    // 平台实现保持返回 String：Windows / Linux 分支在本机（macOS）不参与编译，
    // 改它们等于改一段无法验证的代码。边界处统一包成 Io 类错误即可，
    // 原始信息原样保留在 message 里。
    copy_text_native(&text).map_err(AppError::io)
}

/// 跨平台剪贴板写入：macOS pbcopy / Windows PowerShell Set-Clipboard / Linux wl-copy|xclip。
fn copy_text_native(text: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        copy_text_with_pbcopy(text)
    }

    #[cfg(target_os = "windows")]
    {
        copy_text_with_powershell(text)
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        copy_text_with_linux_clipboard(text)
    }

    #[cfg(not(any(
        target_os = "macos",
        target_os = "windows",
        all(unix, not(target_os = "macos"))
    )))]
    {
        let _ = text;
        Err("当前平台不支持写入剪贴板".to_string())
    }
}

#[cfg(target_os = "macos")]
fn copy_text_with_pbcopy(text: &str) -> Result<(), String> {
    let mut child = Command::new("/usr/bin/pbcopy")
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| format!("无法启动 pbcopy：{e}"))?;

    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| "无法打开 pbcopy 的标准输入".to_string())?;
        stdin
            .write_all(text.as_bytes())
            .map_err(|e| format!("写入剪贴板失败：{e}"))?;
    }

    let status = child
        .wait()
        .map_err(|e| format!("等待 pbcopy 结束失败：{e}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("pbcopy 以非零状态退出：{status}"))
    }
}

/// Windows：经 PowerShell `Set-Clipboard` 写入；stdin 用 UTF-8 文本，避免参数转义问题。
/// 空串在 Win 上 Set-Clipboard 会抛 ArgumentNullException，视为成功 no-op。
#[cfg(target_os = "windows")]
fn copy_text_with_powershell(text: &str) -> Result<(), String> {
    if text.is_empty() {
        return Ok(());
    }

    // 读 UTF-8 标准输入再 Set-Clipboard，兼容多行/特殊字符，且不把正文塞进命令行参数。
    let mut child = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[Console]::InputEncoding = [System.Text.Encoding]::UTF8; $t = [Console]::In.ReadToEnd(); if ($null -eq $t) { $t = [string]::Empty }; Set-Clipboard -Value $t",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("无法启动 PowerShell 写入剪贴板：{e}"))?;

    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| "无法打开 PowerShell 的标准输入".to_string())?;
        stdin
            .write_all(text.as_bytes())
            .map_err(|e| format!("写入剪贴板失败：{e}"))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("等待 PowerShell 结束失败：{e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "Set-Clipboard 失败（状态 {}）：{}",
            output.status,
            err.trim()
        ))
    }
}

/// Linux：优先 Wayland `wl-copy`，否则 X11 `xclip`。
#[cfg(all(unix, not(target_os = "macos")))]
fn copy_text_with_linux_clipboard(text: &str) -> Result<(), String> {
    let mut last_err = String::from("未找到可用的剪贴板后端（已尝试 wl-copy、xclip）");

    for (bin, args) in [
        ("wl-copy", vec![] as Vec<&str>),
        ("xclip", vec!["-selection", "clipboard"]),
    ] {
        match Command::new(bin)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(mut child) => {
                if let Some(stdin) = child.stdin.as_mut() {
                    if let Err(e) = stdin.write_all(text.as_bytes()) {
                        last_err = format!("{bin}：写入标准输入失败：{e}");
                        continue;
                    }
                } else {
                    last_err = format!("{bin}：无法打开标准输入");
                    continue;
                }
                match child.wait_with_output() {
                    Ok(out) if out.status.success() => return Ok(()),
                    Ok(out) => {
                        last_err = format!(
                            "{bin} 以状态 {} 退出：{}",
                            out.status,
                            String::from_utf8_lossy(&out.stderr).trim()
                        );
                    }
                    Err(e) => last_err = format!("{bin}：等待进程结束失败：{e}"),
                }
            }
            Err(e) => last_err = format!("{bin}：{e}"),
        }
    }

    Err(format!("写入剪贴板失败：{last_err}"))
}
#[cfg(test)]
mod clipboard_tests {
    use super::copy_text_native;

    /// 无头 Linux（如 CI）既无 wl-copy 也无 xclip，硬断言只会让 CI 常红；
    /// 后端缺失时跳过，有后端的开发机照常真测。macOS/Windows 后端随系统自带，不跳过。
    #[cfg(all(unix, not(target_os = "macos")))]
    fn backend_in_path(path_var: Option<std::ffi::OsString>) -> bool {
        let Some(path_var) = path_var else {
            return false;
        };
        std::env::split_paths(&path_var)
            .filter(|dir| !dir.as_os_str().is_empty())
            .any(|dir| dir.join("wl-copy").is_file() || dir.join("xclip").is_file())
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    fn backend_available() -> bool {
        backend_in_path(std::env::var_os("PATH"))
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    macro_rules! require_backend {
        () => {
            if !backend_available() {
                println!("SKIP: no wl-copy/xclip on PATH (headless environment)");
                return;
            }
        };
    }

    #[test]
    fn test_copy_text_native_accepts_empty_text() {
        #[cfg(all(unix, not(target_os = "macos")))]
        require_backend!();
        // 空串在各平台后端都应可接受（不崩、不拒）。
        copy_text_native("").expect("empty clipboard text should copy");
    }

    #[test]
    fn test_copy_text_native_accepts_unicode() {
        #[cfg(all(unix, not(target_os = "macos")))]
        require_backend!();
        copy_text_native("Helio 剪贴板 ✓")
            .expect("unicode clipboard text should copy on this platform");
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn test_backend_detection_matches_tool_presence() {
        assert!(!backend_in_path(None));
        let dir = tempfile::tempdir().unwrap();
        assert!(!backend_in_path(Some(dir.path().as_os_str().to_owned())));
        std::fs::write(dir.path().join("wl-copy"), "fake").unwrap();
        assert!(backend_in_path(Some(dir.path().as_os_str().to_owned())));
    }
}
