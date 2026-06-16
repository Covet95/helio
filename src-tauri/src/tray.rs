//! macOS 状态栏(tray)：动态菜单 + 一键切换 profile。
use crate::models::TargetApp;
use crate::commands::AppState;
use crate::models::ApiProfile;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Manager};

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

/// 所有工具，固定顺序。
const TOOLS: [TargetApp; 4] = [
    TargetApp::ClaudeCode,
    TargetApp::Codex,
    TargetApp::Gemini,
    TargetApp::OpenCode,
];

/// 从数据库读 profiles + 各工具 active，构建完整 tray 菜单。
/// 读库失败时降级：仍返回含「打开 Helio / 退出」的菜单，不阻塞 tray。
fn build_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    // 读数据：profiles 列表 + 每个工具的 active id。失败则视为空。
    let (profiles, actives): (Vec<ApiProfile>, Vec<(TargetApp, Option<i64>)>) = {
        let state = app.state::<AppState>();
        let db = match state.db.lock() {
            Ok(db) => db,
            Err(_) => {
                // 库锁中毒：降级为只有固定项的菜单
                return fallback_menu(app);
            }
        };
        let profiles = db.list_profiles().unwrap_or_default();
        let actives = TOOLS
            .iter()
            .map(|&t| {
                let id = db.get_active_profile(t).ok().flatten().map(|a| a.profile_id);
                (t, id)
            })
            .collect();
        (profiles, actives)
    };

    let mut submenus: Vec<Submenu<tauri::Wry>> = Vec::new();
    for &tool in TOOLS.iter() {
        let active_id = actives
            .iter()
            .find(|(t, _)| *t == tool)
            .and_then(|(_, id)| *id);

        // 该工具下的 profiles：target_app == tool 或 None(通用)
        let items: Vec<CheckMenuItem<tauri::Wry>> = profiles
            .iter()
            .filter(|p| p.target_app == Some(tool) || p.target_app.is_none())
            .map(|p| {
                let checked = is_active(p.id, active_id);
                CheckMenuItem::with_id(
                    app,
                    encode_switch_id(tool, &p.name),
                    &p.name,
                    true,
                    checked,
                    None::<&str>,
                )
            })
            .collect::<tauri::Result<Vec<_>>>()?;

        // 子菜单 items 需要 &dyn IsMenuItem
        let item_refs: Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> =
            items.iter().map(|i| i as &dyn tauri::menu::IsMenuItem<tauri::Wry>).collect();

        let submenu = if item_refs.is_empty() {
            // 没有 profile：放个禁用占位项，提示去 GUI 添加
            let placeholder = MenuItem::with_id(
                app,
                format!("empty::{}", tool.as_str()),
                "（无 profile，去 Helio 添加）",
                false,
                None::<&str>,
            )?;
            Submenu::with_id_and_items(
                app,
                format!("tool::{}", tool.as_str()),
                tool_display_name(tool),
                true,
                &[&placeholder],
            )?
        } else {
            Submenu::with_id_and_items(
                app,
                format!("tool::{}", tool.as_str()),
                tool_display_name(tool),
                true,
                &item_refs,
            )?
        };
        submenus.push(submenu);
    }

    let sep = PredefinedMenuItem::separator(app)?;
    let open = MenuItem::with_id(app, "open_window", "打开 Helio", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;

    let mut all: Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> = Vec::new();
    for s in &submenus {
        all.push(s as &dyn tauri::menu::IsMenuItem<tauri::Wry>);
    }
    all.push(&sep as &dyn tauri::menu::IsMenuItem<tauri::Wry>);
    all.push(&open as &dyn tauri::menu::IsMenuItem<tauri::Wry>);
    all.push(&quit as &dyn tauri::menu::IsMenuItem<tauri::Wry>);

    Menu::with_items(app, &all)
}

/// 降级菜单：只有「打开 Helio / 退出」。
fn fallback_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let open = MenuItem::with_id(app, "open_window", "打开 Helio", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    Menu::with_items(
        app,
        &[
            &open as &dyn tauri::menu::IsMenuItem<tauri::Wry>,
            &quit as &dyn tauri::menu::IsMenuItem<tauri::Wry>,
        ],
    )
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
