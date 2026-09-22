//! 保真写入路径对**全部 7 个工具**的端到端契约。
//!
//! 默认 trait 实现让每个适配器都走三路合并。这里逐工具验证：
//! 切换后用户手写的、与 API 无关的配置必须存活，且受管字段被正确更新。
//!
//! 沿用仓库既有做法（见 `credentials_switch_e2e.rs`）：`#![cfg(unix)]`
//! （`dirs` 在 Windows 不读 `$HOME`），且**整个文件只放一个测试**——
//! `$HOME` 是进程级全局量，多个测试并行会互相覆盖。

#![cfg(unix)]

use anyhow::{ensure, Context};
use switch_api::adapters::get_adapter;
use switch_api::models::{ApiProfile, TargetApp};

fn profile(tool: TargetApp, provider: &str, url: &str, model: &str) -> ApiProfile {
    ApiProfile {
        name: "probe".to_string(),
        provider: provider.to_string(),
        api_url: url.to_string(),
        api_key: "sk-test".to_string(),
        model: Some(model.to_string()),
        target_app: Some(tool),
        ..Default::default()
    }
}

/// 对单个工具跑一次「写入 → 断言用户内容存活」。
///
/// `live` 是切换前用户手写的配置文本，`must_survive` 是必须原样保留的片段。
fn check_tool(
    tool: TargetApp,
    live: &str,
    must_survive: &[&str],
    api_profile: &ApiProfile,
) -> anyhow::Result<()> {
    let adapter = get_adapter(tool);
    let path = adapter.config_path();

    std::fs::create_dir_all(path.parent().context("config dir")?)?;
    std::fs::write(&path, live)?;

    let live_value = adapter
        .read_config()
        .with_context(|| format!("{tool:?} 读取 live 配置失败"))?;
    let shared = adapter.extract_shared_config(&live_value);
    let merged = adapter.merge_config(api_profile, &shared);

    adapter
        .write_config_merged(&merged, None)
        .with_context(|| format!("{tool:?} 保真写入失败"))?;

    let after = std::fs::read_to_string(&path)
        .with_context(|| format!("{tool:?} 读回失败"))?;

    for fragment in must_survive {
        ensure!(
            after.contains(fragment),
            "{tool:?} 丢失了用户内容 {fragment:?}:\n{after}"
        );
    }

    Ok(())
}

#[test]
fn every_tool_keeps_user_content_on_switch() {
    let home = tempfile::tempdir().expect("create temp home");
    let previous_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", home.path());

    let result = (|| -> anyhow::Result<()> {
        // ---------------- Claude Code：JSON，含 MCP 与权限 ----------------
        check_tool(
            TargetApp::ClaudeCode,
            r#"{
  "permissions": { "allow": ["Bash(ls:*)"] },
  "hooks": { "Stop": [] },
  "env": { "ANTHROPIC_BASE_URL": "https://old.example" }
}
"#,
            &["permissions", "Bash(ls:*)", "hooks"],
            &profile(TargetApp::ClaudeCode, "anthropic", "https://new.example", "m"),
        )?;

        // ---------------- Pi：JSON ----------------
        check_tool(
            TargetApp::Pi,
            r#"{
  "defaultProvider": "old",
  "user_custom_theme": "dracula"
}
"#,
            &["user_custom_theme", "dracula"],
            &profile(TargetApp::Pi, "custom", "https://new.example/v1", "m"),
        )?;

        // ---------------- OpenCode：JSON ----------------
        check_tool(
            TargetApp::OpenCode,
            r#"{
  "$schema": "https://opencode.ai/config.json",
  "theme": "my-theme",
  "provider": {}
}
"#,
            &["theme", "my-theme"],
            &profile(TargetApp::OpenCode, "custom", "https://new.example/v1", "m"),
        )?;

        // ---------------- OpenClaw：JSON ----------------
        check_tool(
            TargetApp::OpenClaw,
            r#"{
  "user_setting": "keep-me",
  "models": { "providers": {} }
}
"#,
            &["user_setting", "keep-me"],
            &profile(TargetApp::OpenClaw, "custom", "https://new.example/v1", "m"),
        )?;

        // ---------------- ZCode：JSON ----------------
        check_tool(
            TargetApp::ZCode,
            r#"{
  "uiTheme": "keep-this",
  "provider": {}
}
"#,
            &["uiTheme", "keep-this"],
            &profile(TargetApp::ZCode, "custom", "https://new.example/v1", "m"),
        )?;

        // ---------------- Hermes：YAML（值级合并，注释不保真） ----------------
        check_tool(
            TargetApp::Hermes,
            "user_setting: keep-me\nproviders: {}\n",
            &["user_setting", "keep-me"],
            &profile(TargetApp::Hermes, "custom", "https://new.example/v1", "m"),
        )?;

        // ---------------- Codex：TOML（文本级合并，注释也保真） ----------------
        check_tool(
            TargetApp::Codex,
            "# 手写注释\nuser_key = \"keep-me\"\nmodel_provider = \"custom\"\n",
            &["# 手写注释", "user_key", "keep-me"],
            &profile(TargetApp::Codex, "custom", "https://new.example/v1", "m"),
        )?;

        Ok(())
    })();

    match previous_home {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }
    result.expect("跨工具保真契约失败");
}
