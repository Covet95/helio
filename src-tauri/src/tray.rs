//! macOS 状态栏(tray)：动态菜单 + 一键切换 profile。
use crate::models::TargetApp;

/// 切换菜单项 id 格式：switch::<tool>::<profile_name>
/// profile 名可能含 "::"，解析时从左切 2 段，名字取剩余全部。
fn encode_switch_id(tool: TargetApp, profile_name: &str) -> String {
    format!("switch::{}::{}", tool.as_str(), profile_name)
}

/// 解析切换菜单项 id。非切换 id（open_window/quit/非法）返回 None。
fn parse_switch_id(id: &str) -> Option<(TargetApp, String)> {
    let rest = id.strip_prefix("switch::")?;
    let mut parts = rest.splitn(2, "::");
    let tool_str = parts.next()?;
    let name = parts.next()?;
    if name.is_empty() {
        return None;
    }
    let tool = TargetApp::from_str(tool_str)?;
    Some((tool, name.to_string()))
}

/// 工具在状态栏菜单里的显示名。
fn tool_display_name(tool: TargetApp) -> &'static str {
    match tool {
        TargetApp::ClaudeCode => "Claude Code",
        TargetApp::Codex => "Codex",
        TargetApp::Gemini => "Gemini",
        TargetApp::OpenCode => "OpenCode",
    }
}

/// 给定某工具的 active profile id（可能没有），判断某 profile 是否该打勾。
fn is_active(profile_id: Option<i64>, active_id: Option<i64>) -> bool {
    match (profile_id, active_id) {
        (Some(p), Some(a)) => p == a,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_parse_roundtrip() {
        for (tool, name) in [
            (TargetApp::Codex, "codex-gpt5"),
            (TargetApp::OpenCode, "cpa"),
            (TargetApp::ClaudeCode, "claude-main"),
        ] {
            let id = encode_switch_id(tool, name);
            let parsed = parse_switch_id(&id);
            assert_eq!(parsed, Some((tool, name.to_string())));
        }
    }

    #[test]
    fn test_parse_name_with_double_colon() {
        // profile 名里带 "::" 不能被截断
        let id = encode_switch_id(TargetApp::Gemini, "weird::name");
        assert_eq!(
            parse_switch_id(&id),
            Some((TargetApp::Gemini, "weird::name".to_string()))
        );
    }

    #[test]
    fn test_parse_rejects_non_switch_ids() {
        assert_eq!(parse_switch_id("open_window"), None);
        assert_eq!(parse_switch_id("quit"), None);
        assert_eq!(parse_switch_id("switch::"), None);
        assert_eq!(parse_switch_id("switch::codex::"), None);
        assert_eq!(parse_switch_id("switch::unknowntool::x"), None);
        assert_eq!(parse_switch_id("garbage"), None);
    }

    #[test]
    fn test_tool_display_name() {
        assert_eq!(tool_display_name(TargetApp::ClaudeCode), "Claude Code");
        assert_eq!(tool_display_name(TargetApp::OpenCode), "OpenCode");
    }

    #[test]
    fn test_is_active() {
        assert!(is_active(Some(3), Some(3)));
        assert!(!is_active(Some(3), Some(4)));
        assert!(!is_active(Some(3), None)); // 该工具没有 active
        assert!(!is_active(None, Some(3))); // profile 无 id
    }
}
