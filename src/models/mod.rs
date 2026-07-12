use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Claude Code 专用字段（JSON flatten → IPC 仍为顶层键）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClaudeProfileFields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_mapping: Option<HashMap<String, String>>,
}

/// Codex 专用字段（JSON flatten → IPC 仍为顶层键）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CodexProfileFields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wire_api: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_openai_auth: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub experimental_bearer_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_thinking_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
}

/// OpenCode 专用字段（JSON flatten → IPC 仍为顶层键）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OpenCodeProfileFields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<String>>,
}

/// Hermes 专用字段（JSON flatten → IPC 仍为顶层键）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HermesProfileFields {
    /// chat_completions / anthropic_messages / codex_responses 等
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_mode: Option<String>,
}

/// OpenClaw 专用字段（JSON flatten → IPC 仍为顶层键）
/// 与 Hermes 独立：不共用结构体；同名键 api_mode 因 profile 归属单一 target_app 不冲突。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OpenClawProfileFields {
    /// openai-completions / anthropic-messages / openai-responses 等
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_mode: Option<String>,
    /// models[].maxTokens
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<i64>,
}

/// API Profile - 只存储 API 相关信息
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApiProfile {
    pub id: Option<i64>,
    pub name: String,
    pub provider: String,
    pub api_url: String,
    pub api_key: String,
    /// 默认模型（跨工具）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// 是否启用 1M 上下文窗口（各适配器自行解释，不互相调用）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_1m: Option<bool>,
    /// 归属工具
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_app: Option<TargetApp>,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,

    #[serde(flatten)]
    pub claude: ClaudeProfileFields,
    #[serde(flatten)]
    pub codex: CodexProfileFields,
    #[serde(flatten)]
    pub opencode: OpenCodeProfileFields,
    #[serde(flatten)]
    pub hermes: HermesProfileFields,
    #[serde(flatten)]
    pub openclaw: OpenClawProfileFields,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TargetApp {
    ClaudeCode,
    Codex,
    Gemini,
    #[serde(rename = "opencode")]
    OpenCode,
    Hermes,
    #[serde(rename = "openclaw")]
    OpenClaw,
}

impl TargetApp {
    pub fn as_str(&self) -> &'static str {
        match self {
            TargetApp::ClaudeCode => "claude-code",
            TargetApp::Codex => "codex",
            TargetApp::Gemini => "gemini",
            TargetApp::OpenCode => "opencode",
            TargetApp::Hermes => "hermes",
            TargetApp::OpenClaw => "openclaw",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "claude-code" => Some(TargetApp::ClaudeCode),
            "codex" => Some(TargetApp::Codex),
            "gemini" => Some(TargetApp::Gemini),
            "opencode" => Some(TargetApp::OpenCode),
            "hermes" => Some(TargetApp::Hermes),
            "openclaw" => Some(TargetApp::OpenClaw),
            _ => None,
        }
    }

    #[cfg(not(feature = "tauri-gui"))]
    pub fn all() -> Vec<Self> {
        vec![
            TargetApp::ClaudeCode,
            TargetApp::Codex,
            TargetApp::Gemini,
            TargetApp::OpenCode,
            TargetApp::Hermes,
            TargetApp::OpenClaw,
        ]
    }
}

impl std::fmt::Display for TargetApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedConfig {
    pub target_app: TargetApp,
    pub config: serde_json::Value,
    pub updated_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct ActiveProfile {
    pub profile_id: i64,
}

#[cfg(not(feature = "tauri-gui"))]
impl ApiProfile {
    pub fn new(
        name: String,
        provider: String,
        api_url: String,
        api_key: String,
        model_mapping: Option<HashMap<String, String>>,
    ) -> Self {
        Self {
            id: None,
            name,
            provider,
            api_url,
            api_key,
            model: None,
            context_1m: None,
            target_app: None,
            created_at: None,
            updated_at: None,
            claude: ClaudeProfileFields { model_mapping },
            codex: CodexProfileFields::default(),
            opencode: OpenCodeProfileFields::default(),
            hermes: HermesProfileFields::default(),
            openclaw: OpenClawProfileFields::default(),
        }
    }

    pub fn masked_key(&self) -> String {
        let key = &self.api_key;
        if key.len() > 15 {
            format!("{}...{}", &key[..10], &key[key.len() - 5..])
        } else {
            "***".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_str_roundtrip() {
        for app in [
            TargetApp::ClaudeCode,
            TargetApp::Codex,
            TargetApp::Gemini,
            TargetApp::OpenCode,
            TargetApp::Hermes,
            TargetApp::OpenClaw,
        ] {
            assert_eq!(TargetApp::from_str(app.as_str()), Some(app));
        }
        assert_eq!(TargetApp::from_str("unknown"), None);
    }

    #[test]
    fn test_new_tools_registered() {
        assert_eq!(TargetApp::from_str("gemini"), Some(TargetApp::Gemini));
        assert_eq!(TargetApp::from_str("opencode"), Some(TargetApp::OpenCode));
        assert_eq!(TargetApp::from_str("hermes"), Some(TargetApp::Hermes));
        assert_eq!(TargetApp::from_str("openclaw"), Some(TargetApp::OpenClaw));
    }

    #[test]
    fn test_serde_matches_as_str() {
        for app in [
            TargetApp::ClaudeCode,
            TargetApp::Codex,
            TargetApp::Gemini,
            TargetApp::OpenCode,
            TargetApp::Hermes,
            TargetApp::OpenClaw,
        ] {
            let json = serde_json::to_string(&app).unwrap();
            assert_eq!(json, format!("\"{}\"", app.as_str()));
            let back: TargetApp =
                serde_json::from_str(&format!("\"{}\"", app.as_str())).unwrap();
            assert_eq!(back, app);
        }
        assert_eq!(
            serde_json::to_string(&TargetApp::OpenCode).unwrap(),
            "\"opencode\""
        );
        assert_eq!(
            serde_json::to_string(&TargetApp::Hermes).unwrap(),
            "\"hermes\""
        );
        assert_eq!(
            serde_json::to_string(&TargetApp::OpenClaw).unwrap(),
            "\"openclaw\""
        );
    }

    #[test]
    fn test_profile_tool_groups_flatten_to_top_level_json() {
        let mut mm = HashMap::new();
        mm.insert("sonnet_model".into(), "x".into());
        let p = ApiProfile {
            name: "n".into(),
            provider: "anthropic".into(),
            api_url: "u".into(),
            api_key: "k".into(),
            model: Some("m".into()),
            claude: ClaudeProfileFields {
                model_mapping: Some(mm),
            },
            codex: CodexProfileFields {
                reasoning_effort: Some("xhigh".into()),
                wire_api: Some("responses".into()),
                experimental_bearer_token: Some("sk-b".into()),
                ..Default::default()
            },
            opencode: OpenCodeProfileFields {
                models: Some(vec!["a".into()]),
            },
            hermes: HermesProfileFields {
                api_mode: Some("chat_completions".into()),
            },
            openclaw: OpenClawProfileFields {
                api_mode: None,
                max_tokens: Some(128000),
            },
            ..Default::default()
        };
        let v = serde_json::to_value(&p).unwrap();
        assert!(v.get("claude").is_none());
        assert!(v.get("codex").is_none());
        assert!(v.get("openclaw").is_none());
        assert_eq!(v["model_mapping"]["sonnet_model"], "x");
        assert_eq!(v["reasoning_effort"], "xhigh");
        assert_eq!(v["models"][0], "a");
        assert_eq!(v["api_mode"], "chat_completions");
        assert_eq!(v["max_tokens"], 128000);
        let back: ApiProfile = serde_json::from_value(v).unwrap();
        assert_eq!(
            back.claude
                .model_mapping
                .as_ref()
                .unwrap()
                .get("sonnet_model")
                .map(String::as_str),
            Some("x")
        );
        assert_eq!(back.codex.reasoning_effort.as_deref(), Some("xhigh"));
        assert_eq!(back.opencode.models.as_ref().unwrap()[0], "a");
        // Flatten: last writer of same key wins on serialize; deserialize fills both
        // groups from top-level keys. Prefer tool-owned fields when target_app set.
        assert_eq!(back.openclaw.max_tokens, Some(128000));
    }

    #[cfg(not(feature = "tauri-gui"))]
    #[test]
    fn test_all_variants_roundtrip() {
        for app in TargetApp::all() {
            assert_eq!(TargetApp::from_str(app.as_str()), Some(app));
        }
    }
}
