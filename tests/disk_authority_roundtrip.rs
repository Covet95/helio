//! 真实链路回归：用户在 Helio 之外手改工具配置后切换档案，
//! 磁盘的新内容必须赢，旧库不得复活已删条目；反之库更新时补缺保留。
//!
//! 全文件仅此一个测试：独占进程级 `$HOME`（各适配器经 `dirs` 实时读取），
//! 不与其它测试并行冲突。

use switch_api::adapters;
use switch_api::db::Database;
use switch_api::models::{ApiProfile, TargetApp};

const NEW_URL: &str = "https://new-endpoint.example/v1";
const NEW_KEY: &str = "sk-e2e-new-key-12345";

const DISK_FIXTURE: &str = r#"
model_provider = "custom"
model = "old-model"

[model_providers.custom]
base_url = "https://old-endpoint.example/v1"
env_key = "OLD_KEY_ENV"

[model_providers.other]
base_url = "https://other.example/v1"

[mcp_servers.my-mcp]
command = "npx"
args = ["-y", "my-mcp"]
"#;

fn stale_shared() -> serde_json::Value {
    serde_json::json!({
        "model_provider": "custom",
        "model": "old-model",
        "model_providers": {
            "custom": {},
            "other": { "base_url": "https://other.example/v1" }
        },
        "mcp_servers": {
            "my-mcp": { "command": "npx", "args": ["-y", "my-mcp"] },
            "old-server": { "command": "gone" }
        },
        "stale_top": 1
    })
}

fn new_profile() -> ApiProfile {
    let mut profile = ApiProfile {
        name: "e2e".to_string(),
        provider: "custom".to_string(),
        api_url: NEW_URL.to_string(),
        api_key: NEW_KEY.to_string(),
        model: Some("gpt-e2e".to_string()),
        target_app: Some(TargetApp::Codex),
        ..Default::default()
    };
    profile.normalize_keys();
    profile
}

#[test]
fn switch_preserves_hand_edits_and_applies_new_api() {
    let home = tempfile::tempdir().unwrap();
    let previous_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", home.path());

    let result = (|| -> anyhow::Result<()> {
        let codex_dir = home.path().join(".codex");
        std::fs::create_dir_all(&codex_dir)?;
        let config_path = codex_dir.join("config.toml");
        std::fs::write(&config_path, DISK_FIXTURE)?;

        let db = Database::open(":memory:")?;
        // 旧库：含用户已在磁盘删掉的 MCP 条目与顶层键。
        db.save_shared_config(TargetApp::Codex, stale_shared())?;
        // 落库后重写同内容磁盘文件：mtime 不早于库 updated_at（确定性，不靠 sleep）。
        std::fs::write(&config_path, DISK_FIXTURE)?;

        let profile_id = db.add_profile(&new_profile())?;
        let mut profile = db
            .get_profile_by_id(profile_id)?
            .expect("profile just inserted");
        profile.normalize_keys();

        // 场景一：磁盘更新 → 跳过补缺后切换。
        let persisted = db.get_shared_config(TargetApp::Codex)?;
        let resolved = adapters::resolve_shared_config(TargetApp::Codex, persisted)?;
        assert!(resolved.get("stale_top").is_none());
        assert!(resolved["mcp_servers"].get("old-server").is_none());
        adapters::apply_profile_switch(&db, TargetApp::Codex, &profile, &resolved, false)?;

        let written: toml::Value = toml::from_str(&std::fs::read_to_string(&config_path)?)?;
        assert_eq!(
            written["model_providers"]["custom"]["base_url"].as_str(),
            Some(NEW_URL)
        );
        assert!(written["mcp_servers"].get("my-mcp").is_some());
        assert!(written["mcp_servers"].get("old-server").is_none());
        assert!(written.get("stale_top").is_none());
        // 与本次切换无关的 provider 不动。
        assert_eq!(
            written["model_providers"]["other"]["base_url"].as_str(),
            Some("https://other.example/v1")
        );
        let auth: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(codex_dir.join("auth.json"))?)?;
        assert_eq!(auth["OPENAI_API_KEY"].as_str(), Some(NEW_KEY));

        // 场景二：库更新 → 补缺保留（迁移语义不断）。
        let future = switch_api::models::SharedConfig {
            target_app: TargetApp::Codex,
            config: stale_shared(),
            updated_at: Some(chrono::Utc::now().timestamp() + 3600),
        };
        let completed = adapters::resolve_shared_config(TargetApp::Codex, Some(future))?;
        assert_eq!(completed["stale_top"], 1);
        assert!(completed["mcp_servers"].get("old-server").is_some());
        Ok(())
    })();

    match previous_home {
        Some(home) => std::env::set_var("HOME", home),
        None => std::env::remove_var("HOME"),
    }
    result.unwrap();
}
