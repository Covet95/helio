//! 系统托盘 / 状态栏：动态菜单 + 一键切换 profile（macOS / Windows / Linux）。
use crate::commands::AppState;
use serde_json::json;
use switch_api::models::ApiProfile;
use switch_api::models::TargetApp;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::Emitter;
use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;

/// 固定菜单项 id（事件处理器按字符串匹配，集中定义避免拼写漂移）。
const OPEN_WINDOW_ID: &str = "open_window";
const QUIT_ID: &str = "quit";
/// 状态栏图标 id（注册与查找须一致；与 tauri.conf.json 的 trayIcon.id 对应）。
const TRAY_ID: &str = "helio-tray";

/// 切换菜单项 id 格式：switch::<tool>::<profile_name>
/// profile 名可能含 "::"，解析时从左切 2 段，名字取剩余全部。
fn encode_switch_id(tool: TargetApp, profile_name: &str) -> String {
    format!("switch::{}::{}", tool.as_str(), profile_name)
}

/// 解析切换菜单项 id。非切换 id（open_window/quit/非法）返回 None。
fn parse_switch_id(id: &str) -> Option<(TargetApp, String)> {
    let rest = id.strip_prefix("switch::")?;
    let (tool_str, name) = rest.split_once("::")?;
    if name.is_empty() {
        return None;
    }
    let tool = TargetApp::parse(tool_str)?;
    Some((tool, name.to_string()))
}

/// 工具在状态栏菜单里的显示名。
fn tool_display_name(tool: TargetApp) -> &'static str {
    match tool {
        TargetApp::ClaudeCode => "Claude Code",
        TargetApp::Codex => "Codex",
        TargetApp::Pi => "Pi",
        TargetApp::OpenCode => "OpenCode",
        TargetApp::Hermes => "Hermes",
        TargetApp::OpenClaw => "OpenClaw",
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
const TOOLS: [TargetApp; 6] = [
    TargetApp::ClaudeCode,
    TargetApp::Codex,
    TargetApp::Pi,
    TargetApp::OpenCode,
    TargetApp::Hermes,
    TargetApp::OpenClaw,
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
                let id = db
                    .get_active_profile(t)
                    .ok()
                    .flatten()
                    .map(|a| a.profile_id);
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

        // 该工具下的 profiles（每条 profile 必须明确归属某工具，无"通用"）
        let items: Vec<CheckMenuItem<tauri::Wry>> = profiles
            .iter()
            .filter(|p| p.target_app == Some(tool))
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
        let item_refs: Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> = items
            .iter()
            .map(|i| i as &dyn tauri::menu::IsMenuItem<tauri::Wry>)
            .collect();

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
    let open = MenuItem::with_id(app, OPEN_WINDOW_ID, "打开 Helio", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, QUIT_ID, "退出", true, None::<&str>)?;

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
    let open = MenuItem::with_id(app, OPEN_WINDOW_ID, "打开 Helio", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, QUIT_ID, "退出", true, None::<&str>)?;
    Menu::with_items(
        app,
        &[
            &open as &dyn tauri::menu::IsMenuItem<tauri::Wry>,
            &quit as &dyn tauri::menu::IsMenuItem<tauri::Wry>,
        ],
    )
}

/// 显示并聚焦主窗口（托盘图标 / 「打开 Helio」/ macOS Dock 点击时用）。
pub(crate) fn show_main_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
    }
}

/// 执行一次切换：复用 apply_profile_switch（含崩溃一致性 journal）+ emit 事件 + 弹通知。
/// 出错时只弹错误通知，不 panic、不动菜单。
fn do_switch(app: &AppHandle, tool: TargetApp, profile_name: &str) {
    let result: Result<(), String> = (|| {
        let state = app.state::<AppState>();
        let (profile, persisted_shared_config) = {
            let db = state.db.lock().map_err(|e| e.to_string())?;
            let profile = db
                .get_profile_by_name_and_target(profile_name, tool)
                .map_err(|e| e.to_string())?;
            let persisted_shared_config = db
                .get_shared_config(tool)
                .map_err(|e| e.to_string())?
                .map(|config| config.config);
            (profile, persisted_shared_config)
        };
        // 全局写锁：与 GUI 切换等写盘命令互斥
        let _write_guard = state.config_lock.lock().map_err(|e| e.to_string())?;
        let shared_config =
            switch_api::adapters::resolve_shared_config(tool, persisted_shared_config)
                .map_err(|e| e.to_string())?;
        let db = state.db.lock().map_err(|e| e.to_string())?;
        switch_api::adapters::apply_profile_switch(&db, tool, &profile, &shared_config, true)
            .map_err(|e| format!("切换失败: {e}"))?;
        Ok(())
    })();

    match result {
        Ok(()) => {
            // 通知前端刷新当前状态
            let _ = app.emit(
                "profile-switched",
                json!({ "tool": tool.as_str(), "profile_name": profile_name }),
            );
            // 原生通知
            let _ = app
                .notification()
                .builder()
                .title("Helio")
                .body(format!(
                    "已切换 {} → {}",
                    tool_display_name(tool),
                    profile_name
                ))
                .show();
        }
        Err(e) => {
            let _ = app
                .notification()
                .builder()
                .title("Helio 切换失败")
                .body(format!(
                    "{} → {}：{}",
                    tool_display_name(tool),
                    profile_name,
                    e
                ))
                .show();
        }
    }
}

/// 生成托盘用的「太阳」图标（实心圆芯 + 8 道细长光芒）。
/// - macOS：纯黑 + 透明，配合 icon_as_template(true) 按主题自动反色。
/// - Windows / Linux：金橙色实心，任务栏/系统托盘上可辨识（模板图标在这些平台无效）。
///
/// 代码生成，不依赖图片文件。参数与设计原型一致（变体 B）。
fn sun_icon() -> tauri::image::Image<'static> {
    const S: u32 = 44;
    const CORE_R: f32 = 7.5;
    const RAY_INNER: f32 = 12.0;
    const RAY_OUTER: f32 = 20.5;
    const RAY_HALF: f32 = 1.6;
    const N_RAYS: u32 = 8;

    // Windows / Linux 托盘用暖色太阳；macOS 模板图标保持纯黑。
    #[cfg(target_os = "macos")]
    const RGB: (u8, u8, u8) = (0, 0, 0);
    #[cfg(not(target_os = "macos"))]
    const RGB: (u8, u8, u8) = (245, 166, 35); // #F5A623

    let cx = (S as f32 - 1.0) / 2.0;
    let cy = (S as f32 - 1.0) / 2.0;

    // 线性渐变：x 从 e0 过渡到 e1 时，返回 0→1（用于 1px 抗锯齿边）
    let ramp = |e0: f32, e1: f32, x: f32| -> f32 { ((x - e0) / (e1 - e0)).clamp(0.0, 1.0) };

    let mut rgba = vec![0u8; (S * S * 4) as usize];
    for y in 0..S {
        for x in 0..S {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let dist = dx.hypot(dy);

            // 圆芯
            let mut a = ramp(CORE_R + 0.6, CORE_R - 0.6, dist);

            // 8 道光芒
            let ang = dy.atan2(dx);
            for k in 0..N_RAYS {
                let ra = (k as f32 / N_RAYS as f32) * std::f32::consts::TAU;
                let mut da = ang - ra;
                // 归一到 [-PI, PI]
                da = da.sin().atan2(da.cos());
                let perp = da.sin().abs() * dist; // 到光芒中心线的垂直距离
                let along = da.cos() * dist; // 沿光芒方向的投影
                if along > 0.0 {
                    let band = ramp(RAY_INNER - 0.6, RAY_INNER + 0.6, along)
                        * ramp(RAY_OUTER + 0.6, RAY_OUTER - 0.6, along);
                    let wid = ramp(RAY_HALF + 0.6, RAY_HALF - 0.6, perp);
                    a = a.max(band * wid);
                }
            }

            let i = ((y * S + x) * 4) as usize;
            rgba[i] = RGB.0;
            rgba[i + 1] = RGB.1;
            rgba[i + 2] = RGB.2;
            rgba[i + 3] = (a * 255.0).round() as u8;
        }
    }

    tauri::image::Image::new_owned(rgba, S, S)
}

/// 应用启动时建托盘图标。挂菜单 + 事件处理器。
pub fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let menu = build_menu(app)?;

    let builder = TrayIconBuilder::with_id(TRAY_ID)
        .icon(sun_icon())
        .tooltip("Helio")
        .menu(&menu)
        // Windows：左键默认也弹菜单；关掉后由下方 Click 处理器负责显示主窗口。
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            let id = event.id.as_ref();
            match id {
                OPEN_WINDOW_ID => {
                    show_main_window(app);
                    // 点开时刷新菜单（满足需求：GUI 改动后下次点开刷新）
                    rebuild_tray_menu(app);
                }
                QUIT_ID => {
                    app.exit(0);
                }
                other => {
                    if let Some((tool, name)) = parse_switch_id(other) {
                        // 切到后台线程执行：写盘可能在窗口/菜单事件线程上阻塞 UI
                        let app = app.clone();
                        let name = name.to_string();
                        tauri::async_runtime::spawn(async move {
                            do_switch(&app, tool, &name);
                            // 切换后重建菜单：勾选移到新 profile
                            rebuild_tray_menu(&app);
                        });
                    }
                    // 其它(占位项 empty::* / 子菜单容器 tool::* 等)忽略
                }
            }
        })
        .on_tray_icon_event(|tray, event| {
            // 左键单击图标：显示窗口（Windows 任务栏托盘 / macOS 状态栏通用）
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                show_main_window(app);
                rebuild_tray_menu(app);
            }
        });

    // 仅 macOS 使用模板图标（系统按浅色/深色菜单栏反色）。
    #[cfg(target_os = "macos")]
    let builder = builder.icon_as_template(true);

    builder.build(app)?;

    Ok(())
}

/// 重建 tray 菜单（切换后 / 点开窗口时）。失败则忽略，保持旧菜单。
fn rebuild_tray_menu(app: &AppHandle) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        if let Ok(menu) = build_menu(app) {
            let _ = tray.set_menu(Some(menu));
        }
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
        let id = encode_switch_id(TargetApp::Pi, "weird::name");
        assert_eq!(
            parse_switch_id(&id),
            Some((TargetApp::Pi, "weird::name".to_string()))
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

    #[test]
    fn test_sun_icon_valid() {
        let img = sun_icon();
        // 44x44 RGBA
        assert_eq!(img.width(), 44);
        assert_eq!(img.height(), 44);
        let rgba = img.rgba();
        assert_eq!(rgba.len(), 44 * 44 * 4);
        // 颜色通道在整幅图中恒定（仅 alpha 变化）
        #[cfg(target_os = "macos")]
        let expected_rgb = (0u8, 0u8, 0u8);
        #[cfg(not(target_os = "macos"))]
        let expected_rgb = (245u8, 166u8, 35u8);
        assert!(rgba
            .chunks(4)
            .all(|p| p[0] == expected_rgb.0 && p[1] == expected_rgb.1 && p[2] == expected_rgb.2));
        // 既有不透明像素（图形），也有透明像素（背景）——不是全黑块、也不是全空
        let opaque = rgba.chunks(4).filter(|p| p[3] == 255).count();
        let transparent = rgba.chunks(4).filter(|p| p[3] == 0).count();
        assert!(opaque > 50, "应有足够的实心像素，实得 {opaque}");
        assert!(transparent > 200, "应有大片透明背景，实得 {transparent}");
        // 正中心（圆芯）必须不透明
        let center = ((22 * 44 + 22) * 4) as usize;
        assert_eq!(rgba[center + 3], 255, "圆芯中心应为实心");
        // 四角必须透明（光芒不会到角）
        assert_eq!(rgba[3], 0, "左上角应透明");
    }
}
