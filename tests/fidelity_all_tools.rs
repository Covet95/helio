//! 保真写入路径对**全部 7 个工具**的端到端契约。
//!
//! 默认 trait 实现让每个适配器都走三路合并。这里逐工具验证：
//! 切换后用户手写的、与 API 无关的配置必须存活，且受管字段被正确更新。
//!
//! `#![cfg(unix)]`：`dirs` 在 Windows 不读 `$HOME`，测试会读写 runner 的
//! 真实用户目录。
//!
//! 并发：`$HOME` 是**进程级全局量**，多个测试并行会互相覆盖。仓库既有做法
//! 是「一个文件只放一个测试」（见 `credentials_switch_e2e.rs`），代价是
//! 场景全挤进一个巨型测试、失败难以定位。这里改用 [`HomeGuard`] 串行化——
//! 每个测试独占 `$HOME`，互不干扰，同时保持场景独立可单独重跑。

#![cfg(unix)]

use anyhow::{ensure, Context};
use std::sync::{Mutex, MutexGuard, OnceLock};
use switch_api::adapters::get_adapter;
use switch_api::models::{ApiProfile, TargetApp};

/// 串行化 `$HOME` 改写。`Mutex` 中毒不影响后续测试——我们只关心互斥，
/// 不关心前一个测试的断言结果。
static HOME_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// 测试期间独占 `$HOME`，析构时恢复原值。
///
/// 持有锁的生命周期覆盖整个测试体，因此同一时刻只有一个测试在改 `$HOME`。
struct HomeGuard {
    _lock: MutexGuard<'static, ()>,
    dir: tempfile::TempDir,
    previous: Option<std::ffi::OsString>,
}

impl HomeGuard {
    fn new() -> Self {
        let lock = HOME_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let dir = tempfile::tempdir().expect("create temp home");
        let previous = std::env::var_os("HOME");
        std::env::set_var("HOME", dir.path());

        Self {
            _lock: lock,
            dir,
            previous,
        }
    }

    fn path(&self) -> &std::path::Path {
        self.dir.path()
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
    }
}

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

/// 对单个工具跑一次**完整切换事务**（含凭据与辅助文件写入），
/// 断言主配置里的用户内容存活。
///
/// 走 `apply_profile_transaction_with_previous` 而非只调 `write_config_merged`：
/// 真实切换会依次写主配置、凭据、辅助文件，只在事务层面验证才能覆盖全链路。
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

    switch_api::adapters::apply_profile_transaction_with_previous(
        adapter.as_ref(),
        api_profile,
        &shared,
        None,
    )
    .with_context(|| format!("{tool:?} 切换事务失败"))?;

    let after = std::fs::read_to_string(&path).with_context(|| format!("{tool:?} 读回失败"))?;

    for fragment in must_survive {
        ensure!(
            after.contains(fragment),
            "{tool:?} 丢失了用户内容 {fragment:?}:\n{after}"
        );
    }

    Ok(())
}

/// 辅助文件（凭据 / MCP 等）在切换后必须保留其原有的非受管内容。
fn check_aux_file(
    tool: TargetApp,
    path: std::path::PathBuf,
    live: &str,
    must_survive: &[&str],
) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, live)?;

    let adapter = get_adapter(tool);
    let main_path = adapter.config_path();
    std::fs::create_dir_all(main_path.parent().context("config dir")?)?;
    if !main_path.exists() {
        std::fs::write(&main_path, "{}")?;
    }

    let live_value = adapter.read_config()?;
    let shared = adapter.extract_shared_config(&live_value);
    let api_profile = profile(tool, "custom", "https://new.example/v1", "m");

    switch_api::adapters::apply_profile_transaction_with_previous(
        adapter.as_ref(),
        &api_profile,
        &shared,
        None,
    )
    .with_context(|| format!("{tool:?} 切换事务失败"))?;

    let after =
        std::fs::read_to_string(&path).with_context(|| format!("{tool:?} 辅助文件读回失败"))?;

    for fragment in must_survive {
        ensure!(
            after.contains(fragment),
            "{tool:?} 辅助文件 {} 丢失了 {fragment:?}:\n{after}",
            path.display()
        );
    }

    Ok(())
}

#[test]
fn every_tool_keeps_user_content_on_switch() {
    let home = HomeGuard::new();

    (|| -> anyhow::Result<()> {
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
            &profile(
                TargetApp::ClaudeCode,
                "anthropic",
                "https://new.example",
                "m",
            ),
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

        // ---------------- Hermes：YAML（保格式合并，注释也保真） ----------------
        check_tool(
            TargetApp::Hermes,
            "# 手写注释\nuser_setting: keep-me\nproviders: {}\n",
            &["# 手写注释", "user_setting", "keep-me"],
            &profile(TargetApp::Hermes, "custom", "https://new.example/v1", "m"),
        )?;

        // ---------------- Codex：TOML（文本级合并，注释也保真） ----------------
        check_tool(
            TargetApp::Codex,
            "# 手写注释\nuser_key = \"keep-me\"\nmodel_provider = \"custom\"\n",
            &["# 手写注释", "user_key", "keep-me"],
            &profile(TargetApp::Codex, "custom", "https://new.example/v1", "m"),
        )?;

        // ---------------- 辅助文件：切换不得抹掉其中的非受管内容 ----------------
        // Codex 的 auth.json：运行时 OAuth 字段必须存活（只该改 Helio 管的键）。
        check_aux_file(
            TargetApp::Codex,
            home.path().join(".codex/auth.json"),
            r#"{"tokens":{"access_token":"runtime-oauth"},"user_setting":"keep-me"}"#,
            &["runtime-oauth", "user_setting", "keep-me"],
        )?;

        // Pi 的 auth.json：其他 provider 的凭据必须存活。
        check_aux_file(
            TargetApp::Pi,
            home.path().join(".pi/agent/auth.json"),
            r#"{"other_provider":{"type":"api_key","key":"other-secret"},"user_note":"keep-me"}"#,
            &["other-secret", "user_note", "keep-me"],
        )?;

        Ok(())
    })()
    .expect("跨工具保真契约失败");
}

/// 场景：用户切到 A → 手动编辑 config → 切到 B。
///
/// 验证「用户手改的**非受管**内容」在第二次切换后仍然存活。这是
/// 「Helio 之外手改配置」这一真实用法的核心契约。
#[test]
fn user_edits_between_switches_survive() {
    let _home = HomeGuard::new();

    (|| -> anyhow::Result<()> {
        let adapter = get_adapter(TargetApp::Codex);
        let path = adapter.config_path();
        std::fs::create_dir_all(path.parent().context("config dir")?)?;
        std::fs::write(&path, "# 初始\nmodel_provider = \"custom\"\n")?;

        // 第一次切换：A。
        let a = profile(TargetApp::Codex, "custom", "https://a.example/v1", "ma");
        let live = adapter.read_config()?;
        let shared = adapter.extract_shared_config(&live);
        let merged_a = adapter.merge_config(&a, &shared);
        switch_api::adapters::apply_profile_transaction_with_previous(
            adapter.as_ref(),
            &a,
            &shared,
            None,
        )?;

        // 用户在 Helio 之外手改：加了自己的键与注释。
        let text = std::fs::read_to_string(&path)?;
        let edited = format!("{text}\n# 用户手写的段落\nuser_custom_key = \"keep-me\"\n");
        std::fs::write(&path, &edited)?;

        // 第二次切换：B。previous_managed 来自 A。
        let b = profile(TargetApp::Codex, "custom", "https://b.example/v1", "mb");
        let live2 = adapter.read_config()?;
        let shared2 = adapter.extract_shared_config(&live2);
        switch_api::adapters::apply_profile_transaction_with_previous(
            adapter.as_ref(),
            &b,
            &shared2,
            Some(&merged_a),
        )?;

        let after = std::fs::read_to_string(&path)?;

        ensure!(
            after.contains("user_custom_key = \"keep-me\""),
            "用户手改的键丢失:\n{after}"
        );
        ensure!(
            after.contains("# 用户手写的段落"),
            "用户手写的注释丢失:\n{after}"
        );
        ensure!(
            after.contains("https://b.example/v1"),
            "新受管值未写入:\n{after}"
        );
        ensure!(
            !after.contains("https://a.example/v1"),
            "旧受管值残留:\n{after}"
        );

        Ok(())
    })()
    .expect("用户手改内容存活契约失败");
}

/// OpenCode 的多轮切换：验证 provider 增删与用户内容留存。
///
/// OpenCode 的 shared_config 会被 `prepare_shared_config_for_switch` 变换，
/// 是唯一在切换路径上改动共享配置的适配器，因此单独钉一条。
#[test]
fn opencode_multi_switch_keeps_user_content() {
    let _home = HomeGuard::new();

    (|| -> anyhow::Result<()> {
        let adapter = get_adapter(TargetApp::OpenCode);
        let path = adapter.config_path();
        std::fs::create_dir_all(path.parent().context("config dir")?)?;
        std::fs::write(
            &path,
            r#"{
  "$schema": "https://opencode.ai/config.json",
  "theme": "my-theme",
  "username": "keep-me",
  "provider": {}
}
"#,
        )?;

        let a = profile(TargetApp::OpenCode, "provA", "https://a.example/v1", "ma");
        let live = adapter.read_config()?;
        let shared = adapter.extract_shared_config(&live);
        let merged_a = adapter.merge_config(&a, &shared);
        switch_api::adapters::apply_profile_transaction_with_previous(
            adapter.as_ref(),
            &a,
            &shared,
            None,
        )?;

        let b = profile(TargetApp::OpenCode, "provB", "https://b.example/v1", "mb");
        let live2 = adapter.read_config()?;
        let shared2 = adapter.extract_shared_config(&live2);
        switch_api::adapters::apply_profile_transaction_with_previous(
            adapter.as_ref(),
            &b,
            &shared2,
            Some(&merged_a),
        )?;

        let after = std::fs::read_to_string(&path)?;

        // 用户内容必须存活。
        ensure!(after.contains("my-theme"), "用户 theme 丢失:\n{after}");
        ensure!(after.contains("keep-me"), "用户 username 丢失:\n{after}");
        ensure!(after.contains("$schema"), "用户 $schema 丢失:\n{after}");
        // 新 provider 写入。
        ensure!(
            after.contains("https://b.example/v1"),
            "新 provider 未写入:\n{after}"
        );
        // 旧 provider **应当保留**：OpenCode 是多 provider 共存语义
        // （见 `merge_config` 的「从磁盘补回其他 provider 的 key」注释），
        // 切换到 B 不应删掉 A。这条与其余单 provider 工具不同，勿照搬断言。
        ensure!(
            after.contains("https://a.example/v1"),
            "旧 provider 被误删（OpenCode 应共存）:\n{after}"
        );

        Ok(())
    })()
    .expect("OpenCode 多轮切换契约失败");
}

/// 边界：live 文件是**合法但非对象**的 JSON（数组 / 标量）。
///
/// 这类文件通常不是工具的真实配置（用户误放、或工具改了格式）。行为必须是
/// **整体覆盖为受管内容**，而不是崩溃、也不是产出损坏文件。
#[test]
fn non_object_live_file_is_replaced_not_crashed() {
    let _home = HomeGuard::new();

    (|| -> anyhow::Result<()> {
        let adapter = get_adapter(TargetApp::ZCode);
        let path = adapter.config_path();
        std::fs::create_dir_all(path.parent().context("config dir")?)?;

        // 顶层是数组的「合法 JSON」。
        std::fs::write(&path, "[1, 2, 3]")?;

        let api_profile = profile(TargetApp::ZCode, "custom", "https://new.example/v1", "m");
        switch_api::adapters::apply_profile_transaction_with_previous(
            adapter.as_ref(),
            &api_profile,
            &serde_json::json!({}),
            None,
        )?;

        let after = std::fs::read_to_string(&path)?;
        // 关键：结果必须是合法 JSON，且写入成功。
        let parsed: serde_json::Value = serde_json::from_str(&after)
            .unwrap_or_else(|e| panic!("产出不是合法 JSON：{e}\n{after}"));
        ensure!(parsed.is_object(), "结果应是对象:\n{after}");

        Ok(())
    })()
    .expect("非对象 live 文件处理失败");
}

/// 回归：`merge_config` 的无条件删除必须真的落到文件上。
///
/// Codex 的 `merge_config` 会清掉历史遗留的顶层 `api_key`/`aws_profile`。
/// 这类删除无法被三路合并表达（`previous_managed` 由同一个函数推导，同样
/// 不含这些键），必须靠 `merge_removal_intent` 注入删除意图。
///
/// 这条走**真实事务路径**——单测 helper 通过不代表接线正确（踩过）。
#[test]
fn legacy_keys_are_actually_removed_from_disk() {
    let _home = HomeGuard::new();

    (|| -> anyhow::Result<()> {
        let adapter = get_adapter(TargetApp::Codex);
        let path = adapter.config_path();
        std::fs::create_dir_all(path.parent().context("config dir")?)?;

        // 用户文件里有历史版本误写入的遗留键。
        std::fs::write(
            &path,
            "model = \"seed\"\napi_key = \"legacy-key\"\naws_profile = \"legacy\"\naws_region = \"us-east-1\"\n",
        )?;

        let api_profile = profile(TargetApp::Codex, "custom", "https://new.example/v1", "m");
        let live = adapter.read_config()?;
        let shared = adapter.extract_shared_config(&live);

        switch_api::adapters::apply_profile_transaction_with_previous(
            adapter.as_ref(),
            &api_profile,
            &shared,
            None,
        )?;

        let after = std::fs::read_to_string(&path)?;

        ensure!(!after.contains("legacy-key"), "遗留 api_key 未被清理:\n{after}");
        ensure!(!after.contains("aws_profile"), "遗留 aws_profile 未被清理:\n{after}");
        ensure!(!after.contains("aws_region"), "遗留 aws_region 未被清理:\n{after}");
        ensure!(
            after.contains("https://new.example/v1"),
            "受管字段应写入:\n{after}"
        );

        Ok(())
    })()
    .expect("遗留键清理契约失败");
}

/// 回归：`~/.claude.json` 是 JSONC 且含 Claude Code 运行时状态，
/// 早期实现会把它整体替换成 `{"mcpServers": ...}`。
///
/// 根因：解析失败就从 `{}` 起步 + 全量写回。`serde_json` 严格解析必然
/// 拒绝 JSONC，于是用户的键与运行时状态全丢。
#[test]
fn claude_aux_jsonc_keeps_runtime_state() {
    let home = HomeGuard::new();

    (|| -> anyhow::Result<()> {
        let adapter = get_adapter(TargetApp::ClaudeCode);

        let claude_json = home.path().join(".claude.json");
        std::fs::write(
            &claude_json,
            r#"{
  // Claude Code 的运行时状态
  "numStartups": 42,
  "user_own_key": "keep-me",
  "projects": { "/my/proj": { "history": [] } }
}"#,
        )?;

        let settings = adapter.config_path();
        std::fs::create_dir_all(settings.parent().context("config dir")?)?;
        std::fs::write(&settings, "{}")?;

        let shared = serde_json::json!({ "mcpServers": { "fs": { "command": "npx" } } });
        adapter.apply_auxiliary_config(&shared)?;

        let after = std::fs::read_to_string(&claude_json)?;

        ensure!(after.contains("numStartups"), "运行时状态丢失:\n{after}");
        ensure!(after.contains("keep-me"), "用户键丢失:\n{after}");
        ensure!(after.contains("my/proj"), "项目历史丢失:\n{after}");
        ensure!(after.contains("mcpServers"), "MCP 未写入:\n{after}");
        ensure!(after.contains("npx"), "MCP 内容未写入:\n{after}");

        Ok(())
    })()
    .expect("claude.json 保真契约失败");
}

/// 回归：`opencode.json` 常是 JSONC，早期实现因解析失败走 fallback，
/// 导致保真合并在该文件上完全失效。
#[test]
fn opencode_jsonc_still_merges() {
    let _home = HomeGuard::new();

    (|| -> anyhow::Result<()> {
        let adapter = get_adapter(TargetApp::OpenCode);
        let path = adapter.config_path();
        std::fs::create_dir_all(path.parent().context("config dir")?)?;
        std::fs::write(
            &path,
            r#"{
  // 我的 opencode 配置
  "$schema": "https://opencode.ai/config.json",
  "theme": "my-theme",
  "provider": {}
}"#,
        )?;

        let api_profile = profile(TargetApp::OpenCode, "cpa", "https://new.example/v1", "m");
        let live = adapter.read_config()?;
        let shared = adapter.extract_shared_config(&live);

        switch_api::adapters::apply_profile_transaction_with_previous(
            adapter.as_ref(),
            &api_profile,
            &shared,
            None,
        )?;

        let after = std::fs::read_to_string(&path)?;

        ensure!(after.contains("my-theme"), "用户 theme 丢失:\n{after}");
        ensure!(after.contains("$schema"), "用户 $schema 丢失:\n{after}");
        ensure!(
            after.contains("https://new.example/v1"),
            "受管 provider 未写入:\n{after}"
        );

        Ok(())
    })()
    .expect("opencode JSONC 合并契约失败");
}
