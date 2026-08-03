//! 从本机 cc-switch 数据库导入 provider → Helio ApiProfile。
use crate::commands::helpers::{claude_extract_models, str_field};
use crate::commands::AppState;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use switch_api::models::{ApiProfile, ClaudeProfileFields, CodexProfileFields, TargetApp};
use tauri::State;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CcSwitchProvider {
    pub name: String,
    pub app_type: String,
    pub api_url: String,
    pub api_key: String,
    pub provider: String,
    pub model: Option<String>,
    pub model_mapping: Option<HashMap<String, String>>,
    pub reasoning_effort: Option<String>,
    pub context_1m: bool,
    pub wire_api: Option<String>,
    pub env_key: Option<String>,
    pub requires_openai_auth: Option<bool>,
    pub experimental_bearer_token: Option<String>,
    pub service_tier: Option<String>,
    pub is_current: bool,
}

#[tauri::command]
pub async fn scan_cc_switch(target_app: String) -> Result<Vec<CcSwitchProvider>, String> {
    let home = dirs::home_dir().ok_or("无法获取主目录")?;
    let db_path = home.join(".cc-switch").join("cc-switch.db");
    if !db_path.exists() {
        return Err(format!("未找到 cc-switch 数据库: {}", db_path.display()));
    }
    let conn = rusqlite::Connection::open(&db_path)
        .map_err(|e| format!("打开 cc-switch 数据库失败: {}", e))?;
    let mut stmt = conn
        .prepare(
            "SELECT name, settings_config, is_current FROM providers WHERE app_type = ?1 ORDER BY sort_index",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([&target_app], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2).unwrap_or(0) != 0,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        let (name, settings, is_current) = r.map_err(|e| e.to_string())?;
        let mut parsed = parse_cc_provider(&target_app, &settings);
        parsed.name = name;
        parsed.app_type = target_app.clone();
        parsed.is_current = is_current;
        out.push(parsed);
    }
    Ok(out)
}

#[tauri::command]
pub async fn import_cc_switch(
    target_app: String,
    providers: Vec<CcSwitchProvider>,
    state: State<'_, AppState>,
) -> Result<usize, String> {
    let target = TargetApp::parse(&target_app)
        .ok_or_else(|| format!("Unknown target app: {}", target_app))?;
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let mut count = 0;
    for p in providers {
        let mut name = p.name.clone();
        if db.profile_name_exists(&name, target, None).unwrap_or(false) {
            name = format!("{}-{}", p.name, target.as_str());
        }
        let profile = ApiProfile {
            name,
            provider: p.provider,
            api_url: p.api_url,
            api_key: p.api_key,
            model: p.model,
            context_1m: Some(p.context_1m),
            target_app: Some(target),
            claude: ClaudeProfileFields {
                model_mapping: p.model_mapping,
            },
            codex: CodexProfileFields {
                reasoning_effort: p.reasoning_effort,
                wire_api: (target == TargetApp::Codex).then(|| "responses".to_string()),
                env_key: p.env_key,
                service_tier: p.service_tier,
                ..Default::default()
            },
            ..Default::default()
        };
        if db.add_profile(&profile).is_ok() {
            count += 1;
        }
    }
    Ok(count)
}

pub(crate) fn parse_cc_provider(app_type: &str, settings: &str) -> CcSwitchProvider {
    let v: serde_json::Value = serde_json::from_str(settings).unwrap_or(serde_json::json!({}));
    let mut out = CcSwitchProvider {
        name: String::new(),
        app_type: app_type.to_string(),
        api_url: String::new(),
        api_key: String::new(),
        provider: "custom".to_string(),
        model: None,
        model_mapping: None,
        reasoning_effort: None,
        context_1m: false,
        wire_api: None,
        env_key: None,
        requires_openai_auth: None,
        experimental_bearer_token: None,
        service_tier: None,
        is_current: false,
    };
    match app_type {
        "codex" => {
            out.provider = "openai".to_string();
            out.api_key = v
                .get("auth")
                .and_then(|a| a.get("OPENAI_API_KEY"))
                .and_then(|k| k.as_str())
                .unwrap_or("")
                .to_string();
            let config_str = v.get("config").and_then(|c| c.as_str()).unwrap_or("");
            let toml_v: toml::Value =
                toml::from_str(config_str).unwrap_or(toml::Value::Table(Default::default()));
            let cfg: serde_json::Value =
                serde_json::to_value(&toml_v).unwrap_or(serde_json::json!({}));
            let pid = str_field(&cfg, "model_provider");
            let providers = cfg.get("model_providers").and_then(|p| p.as_object());
            let block = providers.and_then(|p| {
                (if !pid.is_empty() { p.get(&pid) } else { None })
                    .or_else(|| p.get("custom"))
                    .or_else(|| p.values().next())
            });
            if let Some(b) = block {
                out.api_url = str_field(b, "base_url");
                let w = str_field(b, "wire_api");
                if !w.trim().is_empty() {
                    out.wire_api = Some(w);
                }
                out.requires_openai_auth = b.get("requires_openai_auth").and_then(|x| x.as_bool());
                let env_key = str_field(b, "env_key");
                if !env_key.trim().is_empty() {
                    out.env_key = Some(env_key);
                }
                let bearer = str_field(b, "experimental_bearer_token");
                if !bearer.trim().is_empty() {
                    out.experimental_bearer_token = Some(bearer);
                }
            }
            if out.api_url.is_empty() {
                out.api_url = str_field(&cfg, "base_url");
            }
            out.model = cfg
                .get("model")
                .and_then(|m| m.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(String::from);
            out.reasoning_effort = cfg
                .get("model_reasoning_effort")
                .and_then(|r| r.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(String::from);
            out.context_1m = cfg
                .get("model_context_window")
                .and_then(|w| w.as_i64())
                .map(|w| w >= 1_000_000)
                .unwrap_or(false);
            let tier = str_field(&cfg, "service_tier");
            if !tier.trim().is_empty() {
                out.service_tier = Some(tier);
            }
        }
        "claude" | "claude-code" => {
            out.provider = "anthropic".to_string();
            let env = v
                .get("settingsConfig")
                .and_then(|s| s.get("env"))
                .or_else(|| v.get("env"));
            if let Some(env) = env {
                out.api_url = str_field(env, "ANTHROPIC_BASE_URL");
                let k = str_field(env, "ANTHROPIC_AUTH_TOKEN");
                out.api_key = if k.is_empty() {
                    str_field(env, "ANTHROPIC_API_KEY")
                } else {
                    k
                };
                let mut model = None;
                let mut mapping = None;
                claude_extract_models(env, &mut model, &mut mapping);
                out.model = model;
                out.model_mapping = mapping;
                if let Some(ref mm) = out.model_mapping {
                    out.context_1m = mm.iter().any(|(k, v)| k.ends_with("_one_m") && v == "true");
                }
            }
        }
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::parse_cc_provider;
    use serde_json::json;

    #[test]
    fn test_parse_claude_role_mapping_and_one_m() {
        let settings = json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://example.com",
                "ANTHROPIC_AUTH_TOKEN": "sk-x",
                "ANTHROPIC_MODEL": "glm-5.1",
                "ANTHROPIC_DEFAULT_SONNET_MODEL": "glm-5.2[1M]",
                "ANTHROPIC_DEFAULT_FABLE_MODEL": "claude-fable-5"
            }
        })
        .to_string();
        let p = parse_cc_provider("claude", &settings);
        assert_eq!(p.model.as_deref(), Some("glm-5.1"));
        let mm = p.model_mapping.unwrap();
        assert_eq!(mm.get("sonnet_model").map(String::as_str), Some("glm-5.2"));
        assert!(p.context_1m);
    }

    #[test]
    fn test_parse_codex_provider_block() {
        let config = r#"
model_provider = "openai-custom"
model = "gpt-5.6-sol"
model_reasoning_effort = "xhigh"
service_tier = "fast"
model_context_window = 1000000
[model_providers.openai-custom]
base_url = "https://welfare.example/v1"
wire_api = "responses"
requires_openai_auth = true
experimental_bearer_token = "sk-bearer"
"#;
        let settings = json!({"auth":{"OPENAI_API_KEY":"sk-auth"},"config":config}).to_string();
        let p = parse_cc_provider("codex", &settings);
        assert_eq!(p.api_url, "https://welfare.example/v1");
        assert_eq!(p.reasoning_effort.as_deref(), Some("xhigh"));
        assert_eq!(p.experimental_bearer_token.as_deref(), Some("sk-bearer"));
        assert!(p.context_1m);
    }
}
