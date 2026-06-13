use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// API Profile - 只存储 API 相关信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiProfile {
    pub id: Option<i64>,
    pub name: String,
    pub provider: String,
    pub api_url: String,
    pub api_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_mapping: Option<HashMap<String, String>>,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
}

/// 目标应用类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TargetApp {
    ClaudeCode,
    Codex,
}

impl TargetApp {
    pub fn as_str(&self) -> &'static str {
        match self {
            TargetApp::ClaudeCode => "claude-code",
            TargetApp::Codex => "codex",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "claude-code" => Some(TargetApp::ClaudeCode),
            "codex" => Some(TargetApp::Codex),
            _ => None,
        }
    }

    pub fn all() -> Vec<Self> {
        vec![TargetApp::ClaudeCode, TargetApp::Codex]
    }
}

impl std::fmt::Display for TargetApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// 共享配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedConfig {
    pub target_app: TargetApp,
    pub config: serde_json::Value,
    pub updated_at: Option<i64>,
}

/// 当前活动的 Profile
#[derive(Debug, Clone)]
pub struct ActiveProfile {
    pub target_app: TargetApp,
    pub profile_id: i64,
}

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
            model_mapping,
            created_at: None,
            updated_at: None,
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
