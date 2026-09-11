//! 真实链路回归（凭据共享模块重构后）。
//!
//! 用真实临时 `$HOME` + 真实 `apply_profile_switch` 跑完整切换链路，验证凭据
//! 剥离/回填重构后仍满足两条契约：
//!
//! 1. 目标 provider 拿到 profile 的 key；
//! 2. 其他 provider 的 key 原样保留，且不与目标 key 串味。
//!
//! 覆盖 opencode / zcode / openclaw / hermes 四个被改动的适配器，并额外覆盖：
//!
//! - 凭据不得泄漏进 `shared_configs` 表（`strip_credentials` 的核心契约）；
//! - 畸形配置（非字符串 key、非对象条目）不 panic、不丢数据；
//! - 连续切换 alpha → beta → 重切 beta（文件型库，走 journal 与
//!   `already_active` 分支），验证「切走再切回」不丢凭据。
//!
//! 适配器经 `dirs` 实时读取 `$HOME`，故本文件独占进程级 `$HOME`，
//! 全文件仅此一个测试，避免与其它测试并行冲突。

use serde_json::json;
use std::fs;
use std::path::Path;
use switch_api::adapters::{self, get_adapter};
use switch_api::db::Database;
use switch_api::models::{ApiProfile, TargetApp};

const NEW_KEY: &str = "sk-target-new-111";
const OTHER_KEY: &str = "sk-other-keep-222";
const OLD_TARGET_KEY: &str = "sk-target-old-000";

fn profile(target: TargetApp, provider: &str, url: &str) -> ApiProfile {
    profile_with(target, provider, url, NEW_KEY)
}

fn profile_with(target: TargetApp, provider: &str, url: &str, key: &str) -> ApiProfile {
    let mut p = ApiProfile {
        name: format!("e2e-{provider}"),
        provider: provider.into(),
        api_url: url.into(),
        api_key: key.into(),
        model: Some("m1".into()),
        target_app: Some(target),
        ..Default::default()
    };
    p.normalize_keys();
    p
}

/// 在既有库上执行一次真实切换（走 journal + already_active 分支）。
fn switch_once(db: &Database, profile_id: i64) -> anyhow::Result<()> {
    let mut stored = db.get_profile_by_id(profile_id)?.expect("profile exists");
    stored.normalize_keys();
    let target = stored.target_app.expect("profile 需带 target_app");
    let shared = adapters::resolve_shared_config(target, db.get_shared_config(target)?)?;
    adapters::apply_profile_switch(db, target, &stored, &shared, false)?;
    Ok(())
}

/// 走真实切换链路（落库 → 解析共享配置 → apply_profile_switch）。
///
/// 返回 `(切换后磁盘配置, 切换后库内 shared_config)`，后者用于确认凭据没有
/// 被写进 `shared_configs` 表。
fn run_switch(
    target: TargetApp,
    provider: &str,
    url: &str,
) -> anyhow::Result<(serde_json::Value, serde_json::Value)> {
    let db = Database::open(":memory:")?;
    let id = db.add_profile(&profile(target, provider, url))?;
    let mut stored = db.get_profile_by_id(id)?.expect("profile just inserted");
    stored.normalize_keys();
    let shared = adapters::resolve_shared_config(target, db.get_shared_config(target)?)?;
    adapters::apply_profile_switch(&db, target, &stored, &shared, false)?;
    let disk = get_adapter(target).read_config()?;
    let persisted = db
        .get_shared_config(target)?
        .map(|s| s.config)
        .unwrap_or(serde_json::Value::Null);
    Ok((disk, persisted))
}

/// `strip_credentials` 的核心契约：任何 key 都不得落入 `shared_configs` 表。
fn assert_no_key_leak(label: &str, persisted: &serde_json::Value) {
    let text = persisted.to_string();
    for secret in [NEW_KEY, OLD_TARGET_KEY, OTHER_KEY] {
        assert!(
            !text.contains(secret),
            "{label}: 凭据泄漏进 shared_configs 表 → {secret}\n库内容：{text}"
        );
    }
}

/// 按适配器自己的序列化方式写盘（JSON / YAML 由适配器决定），并确认可读回。
fn write_fixture(target: TargetApp, dir: &Path, fixture: &serde_json::Value) -> anyhow::Result<()> {
    fs::create_dir_all(dir)?;
    get_adapter(target).write_config(fixture)?;
    let readback = get_adapter(target).read_config()?;
    anyhow::ensure!(
        readback.is_object(),
        "{target:?} 夹具写盘后读回不是对象：{readback}"
    );
    Ok(())
}

#[test]
fn credentials_survive_real_switch_across_adapters() {
    let home = tempfile::tempdir().unwrap();
    let previous_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", home.path());

    let result = (|| -> anyhow::Result<()> {
        // ---------------- opencode ----------------
        let opencode = json!({
            "provider": {
                "target": {
                    "npm": "@ai-sdk/openai-compatible",
                    "name": "target",
                    "options": { "baseURL": "https://old.example/v1", "apiKey": OLD_TARGET_KEY }
                },
                "other": {
                    "npm": "@ai-sdk/openai-compatible",
                    "name": "other",
                    "options": { "baseURL": "https://other.example/v1", "apiKey": OTHER_KEY }
                }
            }
        });
        write_fixture(
            TargetApp::OpenCode,
            &home.path().join(".config/opencode"),
            &opencode,
        )?;
        let (after, persisted) =
            run_switch(TargetApp::OpenCode, "target", "https://new.example/v1")?;
        assert_no_key_leak("opencode", &persisted);
        assert_eq!(
            after["provider"]["target"]["options"]["apiKey"], NEW_KEY,
            "opencode 目标 provider 未拿到新 key"
        );
        assert_eq!(
            after["provider"]["other"]["options"]["apiKey"], OTHER_KEY,
            "opencode 其他 provider 的 key 被洗掉"
        );
        assert_eq!(
            after["provider"]["target"]["options"]["baseURL"], "https://new.example/v1",
            "opencode 目标 baseURL 未更新"
        );

        // ---------------- zcode ----------------
        let zcode = json!({
            "provider": {
                "target": {
                    "name": "target",
                    "options": { "baseURL": "https://old.example/v1", "apiKey": OLD_TARGET_KEY }
                },
                "other": {
                    "name": "other",
                    "options": { "baseURL": "https://other.example/v1", "apiKey": OTHER_KEY }
                }
            }
        });
        write_fixture(TargetApp::ZCode, &home.path().join(".zcode/v2"), &zcode)?;
        let (after, persisted) = run_switch(TargetApp::ZCode, "target", "https://new.example/v1")?;
        assert_no_key_leak("zcode", &persisted);
        assert_eq!(
            after["provider"]["target"]["options"]["apiKey"], NEW_KEY,
            "zcode 目标 provider 未拿到新 key"
        );
        assert_eq!(
            after["provider"]["other"]["options"]["apiKey"], OTHER_KEY,
            "zcode 其他 provider 的 key 被洗掉"
        );

        // ---------------- openclaw ----------------
        let openclaw = json!({
            "models": {
                "mode": "replace",
                "providers": {
                    "target": {
                        "baseUrl": "https://old.example/v1", "apiKey": OLD_TARGET_KEY,
                        "api": "openai-completions", "models": [{ "id": "m1", "name": "m1" }]
                    },
                    "other": {
                        "baseUrl": "https://other.example/v1", "apiKey": OTHER_KEY,
                        "api": "openai-completions", "models": [{ "id": "m1", "name": "m1" }]
                    }
                }
            },
            "mcp": { "servers": { "keep": { "command": "uvx" } } }
        });
        write_fixture(
            TargetApp::OpenClaw,
            &home.path().join(".openclaw"),
            &openclaw,
        )?;
        let (after, persisted) =
            run_switch(TargetApp::OpenClaw, "target", "https://new.example/v1")?;
        assert_no_key_leak("openclaw", &persisted);
        assert_eq!(
            after["models"]["providers"]["target"]["apiKey"], NEW_KEY,
            "openclaw 目标 provider 未拿到新 key"
        );
        assert_eq!(
            after["models"]["providers"]["other"]["apiKey"], OTHER_KEY,
            "openclaw 其他 provider 的 key 被洗掉"
        );
        assert_eq!(
            after["mcp"]["servers"]["keep"]["command"], "uvx",
            "openclaw 切换破坏了 MCP 配置"
        );

        // ---------------- hermes ----------------
        let hermes = json!({
            "model": { "default": "old", "provider": "custom:target" },
            "custom_providers": [
                {
                    "name": "target", "base_url": "https://old.example/v1",
                    "api_key": OLD_TARGET_KEY, "api_mode": "chat_completions"
                },
                {
                    "name": "other", "base_url": "https://other.example/v1",
                    "api_key": OTHER_KEY, "api_mode": "chat_completions"
                }
            ],
            "mcp_servers": { "keep": { "command": "uvx" } }
        });
        write_fixture(TargetApp::Hermes, &home.path().join(".hermes"), &hermes)?;
        let (after, persisted) = run_switch(TargetApp::Hermes, "target", "https://new.example/v1")?;
        assert_no_key_leak("hermes", &persisted);
        let entries = after["custom_providers"]
            .as_array()
            .expect("hermes custom_providers 应为数组")
            .clone();
        let pick = |name: &str| -> serde_json::Value {
            entries
                .iter()
                .find(|e| e["name"] == name)
                .unwrap_or_else(|| panic!("hermes 丢失 provider {name}：{entries:?}"))
                .clone()
        };
        assert_eq!(
            pick("target")["api_key"],
            NEW_KEY,
            "hermes 目标 provider 未拿到新 key"
        );
        assert_eq!(
            pick("other")["api_key"],
            OTHER_KEY,
            "hermes 其他 provider 的 key 被洗掉"
        );
        assert_eq!(
            after["mcp_servers"]["keep"]["command"], "uvx",
            "hermes 切换破坏了 MCP 配置"
        );

        // -------- 畸形配置：非字符串 apiKey 不得被丢弃 --------
        // openclaw 旧实现用 `.as_str()` 取值，非字符串会被静默跳过 → 切换后丢 key。
        // 重构改为按原样回填，这里锁定「不再丢数据」。
        let weird = json!({
            "models": {
                "mode": "replace",
                "providers": {
                    "target": {
                        "baseUrl": "https://old.example/v1", "apiKey": OLD_TARGET_KEY,
                        "api": "openai-completions", "models": [{ "id": "m1", "name": "m1" }]
                    },
                    "numeric": {
                        "baseUrl": "https://n.example/v1", "apiKey": 12345,
                        "api": "openai-completions", "models": [{ "id": "m1", "name": "m1" }]
                    },
                    "notobject": "just-a-string"
                }
            },
            "mcp": { "servers": {} }
        });
        write_fixture(TargetApp::OpenClaw, &home.path().join(".openclaw"), &weird)?;
        let (after, persisted) =
            run_switch(TargetApp::OpenClaw, "target", "https://new.example/v1")?;
        assert_no_key_leak("openclaw/畸形配置", &persisted);
        assert_eq!(
            after["models"]["providers"]["numeric"]["apiKey"], 12345,
            "非字符串 apiKey 被丢弃（数据丢失）"
        );
        assert_eq!(
            after["models"]["providers"]["target"]["apiKey"], NEW_KEY,
            "畸形配置下目标 key 仍未写入"
        );

        // -------- 连续切换（真实文件库，走 journal + already_active 分支）--------
        // 场景：alpha → beta → 再切回 beta（幂等）。每次切换后，未参与本次切换的
        // provider 的 key 必须存活，否则「切走再切回」就会丢凭据。
        {
            let dir = home.path().join(".config/opencode");
            write_fixture(
                TargetApp::OpenCode,
                &dir,
                &json!({
                    "provider": {
                        "alpha": { "name": "alpha",
                                   "options": { "baseURL": "https://a.example/v1", "apiKey": "sk-alpha-orig" } },
                        "beta":  { "name": "beta",
                                   "options": { "baseURL": "https://b.example/v1", "apiKey": "sk-beta-orig" } },
                        "gamma": { "name": "gamma",
                                   "options": { "baseURL": "https://g.example/v1", "apiKey": "sk-gamma-orig" } }
                    }
                }),
            )?;

            // 文件型数据库：journal 落在同目录，真实覆盖崩溃恢复相关路径。
            let db = Database::open(home.path().join("helio-e2e.db"))?;
            let alpha_id = db.add_profile(&profile_with(
                TargetApp::OpenCode,
                "alpha",
                "https://a-new.example/v1",
                "sk-alpha-new",
            ))?;
            let beta_id = db.add_profile(&profile_with(
                TargetApp::OpenCode,
                "beta",
                "https://b-new.example/v1",
                "sk-beta-new",
            ))?;

            switch_once(&db, alpha_id)?;
            let disk = get_adapter(TargetApp::OpenCode).read_config()?;
            assert_eq!(
                disk["provider"]["alpha"]["options"]["apiKey"],
                "sk-alpha-new"
            );
            assert_eq!(
                disk["provider"]["beta"]["options"]["apiKey"], "sk-beta-orig",
                "切到 alpha 时 beta 的 key 被洗掉"
            );
            assert_eq!(
                disk["provider"]["gamma"]["options"]["apiKey"], "sk-gamma-orig",
                "切到 alpha 时 gamma 的 key 被洗掉"
            );

            switch_once(&db, beta_id)?;
            let disk = get_adapter(TargetApp::OpenCode).read_config()?;
            assert_eq!(disk["provider"]["beta"]["options"]["apiKey"], "sk-beta-new");
            assert_eq!(
                disk["provider"]["alpha"]["options"]["apiKey"], "sk-alpha-new",
                "切到 beta 时 alpha 的 key 被洗掉（切走再切回会丢凭据）"
            );
            assert_eq!(
                disk["provider"]["gamma"]["options"]["apiKey"], "sk-gamma-orig",
                "切到 beta 时 gamma 的 key 被洗掉"
            );

            // 重复切换同一 profile：命中 already_active 清理分支，结果必须稳定。
            switch_once(&db, beta_id)?;
            let disk = get_adapter(TargetApp::OpenCode).read_config()?;
            assert_eq!(disk["provider"]["beta"]["options"]["apiKey"], "sk-beta-new");
            assert_eq!(
                disk["provider"]["alpha"]["options"]["apiKey"],
                "sk-alpha-new"
            );
            assert_eq!(
                disk["provider"]["gamma"]["options"]["apiKey"],
                "sk-gamma-orig"
            );

            assert_no_key_leak(
                "连续切换",
                &db.get_shared_config(TargetApp::OpenCode)?
                    .map(|s| s.config)
                    .unwrap_or(serde_json::Value::Null),
            );
        }

        Ok(())
    })();

    match previous_home {
        Some(h) => std::env::set_var("HOME", h),
        None => std::env::remove_var("HOME"),
    }
    result.unwrap();
}
