//! 保真写入路径的端到端契约。
//!
//! 单元测试证明合并算法正确；这里证明**它真的被切换路径用上了**——
//! 用户手写的注释在真实切换后必须存活，上次受管而本次不再受管的字段必须被
//! 摘除。这是本次重构的核心验收标准。
//!
//! 沿用仓库既有做法（见 `credentials_switch_e2e.rs`）：`#![cfg(unix)]`
//! （`dirs` 在 Windows 不读 `$HOME`），且**整个文件只放一个测试**——
//! `$HOME` 是进程级全局量，多个测试并行会互相覆盖。场景用顺序执行的分节
//! 表达，失败时靠 `anyhow::ensure!` 的消息定位。

#![cfg(unix)]

use anyhow::ensure;
use switch_api::adapters::get_adapter;
use switch_api::models::{ApiProfile, TargetApp};

fn codex_profile(provider: &str, url: &str, model: &str) -> ApiProfile {
    ApiProfile {
        name: "test".to_string(),
        provider: provider.to_string(),
        api_url: url.to_string(),
        api_key: "sk-test".to_string(),
        model: Some(model.to_string()),
        ..Default::default()
    }
}

#[test]
fn fidelity_switch_preserves_user_content_and_drops_stale_fields() {
    let home = tempfile::tempdir().expect("create temp home");
    let previous_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", home.path());

    let result = (|| -> anyhow::Result<()> {
        let adapter = get_adapter(TargetApp::Codex);
        let config_path = adapter.config_path();
        std::fs::create_dir_all(config_path.parent().expect("config dir"))?;

        // ---------------- 场景一：注释与未受管字段存活 ----------------
        std::fs::write(
            &config_path,
            "\
#:schema none
# 我的 Codex 配置 —— 手写注释
model = \"old-model\"
approval_policy = \"never\"     # 我习惯永不确认

[model_providers.custom]
name = \"我的中转\"
base_url = \"https://old.example/v1\"

[mcp_servers.filesystem]
command = \"npx\"
",
        )?;

        let profile = codex_profile("custom", "https://new.example/v1", "new-model");
        let live = adapter.read_config()?;
        let shared = adapter.extract_shared_config(&live);
        let merged = adapter.merge_config(&profile, &shared);
        adapter.write_config_merged(&merged, None)?;

        let after = std::fs::read_to_string(&config_path)?;

        ensure!(
            after.contains("# 我的 Codex 配置"),
            "顶层注释丢失:\n{after}"
        );
        ensure!(after.contains("# 我习惯永不确认"), "行尾注释丢失:\n{after}");
        ensure!(
            after.contains("[mcp_servers.filesystem]"),
            "未受管子表丢失:\n{after}"
        );
        ensure!(after.contains("npx"), "未受管子表内容丢失:\n{after}");
        ensure!(
            after.contains("https://new.example/v1"),
            "受管字段未更新:\n{after}"
        );
        ensure!(
            !after.contains("https://old.example/v1"),
            "旧值残留:\n{after}"
        );

        // ---------------- 场景二：陈旧受管字段被摘除 ----------------
        // 第一次切换带 context_1m（会写入上下文窗口相关键）。
        let mut first = codex_profile("custom", "https://first.example/v1", "m1");
        first.context_1m = Some(true);
        let first_merged = adapter.merge_config(&first, &serde_json::json!({}));
        adapter.write_config_merged(&first_merged, None)?;

        let after_first = std::fs::read_to_string(&config_path)?;
        ensure!(
            after_first.contains("model_context_window"),
            "首次切换应写入上下文窗口:\n{after_first}"
        );

        // 第二次切换不再管理该字段（None），陈旧键必须消失。
        let second = codex_profile("custom", "https://second.example/v1", "m2");
        let second_merged = adapter.merge_config(&second, &serde_json::json!({}));
        adapter.write_config_merged(&second_merged, Some(&first_merged))?;

        let after_second = std::fs::read_to_string(&config_path)?;
        ensure!(
            after_second.contains("https://second.example/v1"),
            "新值应写入:\n{after_second}"
        );
        ensure!(
            !after_second.contains("https://first.example/v1"),
            "旧值应被替换:\n{after_second}"
        );
        ensure!(
            !after_second.contains("model_context_window"),
            "陈旧的受管字段未被摘除（会无限累积）:\n{after_second}"
        );

        // ---------------- 场景三：首次切换不删除用户内容 ----------------
        let custom_path = home.path().join("custom_config.toml");
        std::fs::write(
            &custom_path,
            "\
# 用户完全自定义的配置
my_own_key = \"do not delete\"
model = \"user-model\"
",
        )?;
        let custom_text = std::fs::read_to_string(&custom_path)?;
        let custom_merged = adapter.merge_config(
            &codex_profile("custom", "https://third.example/v1", "m3"),
            &serde_json::json!({}),
        );
        let rendered =
            switch_api::doc::toml::merge_json_into_toml(&custom_text, None, &custom_merged)?;

        ensure!(
            rendered.contains("my_own_key = \"do not delete\""),
            "首次切换删除了用户自定义键:\n{rendered}"
        );
        ensure!(
            rendered.contains("# 用户完全自定义的配置"),
            "首次切换删除了用户注释:\n{rendered}"
        );

        Ok(())
    })();

    match previous_home {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }
    result.expect("保真切换契约失败");
}
