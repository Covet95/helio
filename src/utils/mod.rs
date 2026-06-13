use anyhow::Result;
use colored::Colorize;
use std::io::{self, Write};

/// 打印成功消息
pub fn success(msg: &str) {
    println!("{} {}", "✓".green().bold(), msg);
}

/// 打印错误消息
pub fn error(msg: &str) {
    eprintln!("{} {}", "✗".red().bold(), msg);
}

/// 打印信息消息
pub fn info(msg: &str) {
    println!("{} {}", "ℹ".blue().bold(), msg);
}

/// 打印警告消息
pub fn warning(msg: &str) {
    println!("{} {}", "⚠".yellow().bold(), msg);
}

/// 确认提示
pub fn confirm(prompt: &str) -> Result<bool> {
    print!("{} {} [y/N]: ", "?".cyan().bold(), prompt);
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    Ok(input.trim().eq_ignore_ascii_case("y") || input.trim().eq_ignore_ascii_case("yes"))
}

/// 格式化文件大小
pub fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit_index = 0;

    while size >= 1024.0 && unit_index < UNITS.len() - 1 {
        size /= 1024.0;
        unit_index += 1;
    }

    format!("{:.1}{}", size, UNITS[unit_index])
}
