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

/// 配置文件格式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConfigFormat {
    /// JSON 格式（Claude Code, OpenCode）
    Json,
    /// TOML 格式（Codex）
    Toml,
    /// 基于环境变量的 API 存储（Gemini CLI，settings.json + .env）
    EnvBased,
}

/// 工具元数据 - 数据化每个工具的属性
#[derive(Debug, Clone)]
pub struct ToolMetadata {
    /// 工具 ID（kebab-case，如 "claude-code"）
    pub id: &'static str,
    /// 显示名称
    pub display_name: &'static str,
    /// 配置文件格式
    pub config_format: ConfigFormat,
}

/// 目标应用类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TargetApp {
    ClaudeCode,
    Codex,
    Gemini,
    OpenCode,
}

impl TargetApp {
    pub fn as_str(&self) -> &'static str {
        match self {
            TargetApp::ClaudeCode => "claude-code",
            TargetApp::Codex => "codex",
            TargetApp::Gemini => "gemini",
            TargetApp::OpenCode => "opencode",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "claude-code" => Some(TargetApp::ClaudeCode),
            "codex" => Some(TargetApp::Codex),
            "gemini" => Some(TargetApp::Gemini),
            "opencode" => Some(TargetApp::OpenCode),
            _ => None,
        }
    }

    pub fn all() -> Vec<Self> {
        vec![
            TargetApp::ClaudeCode,
            TargetApp::Codex,
            TargetApp::Gemini,
            TargetApp::OpenCode,
        ]
    }

    /// 工具元数据 - 数据驱动各工具的属性
    pub fn metadata(&self) -> ToolMetadata {
        match self {
            TargetApp::ClaudeCode => ToolMetadata {
                id: "claude-code",
                display_name: "Claude Code",
                config_format: ConfigFormat::Json,
            },
            TargetApp::Codex => ToolMetadata {
                id: "codex",
                display_name: "Codex",
                config_format: ConfigFormat::Toml,
            },
            TargetApp::Gemini => ToolMetadata {
                id: "gemini",
                display_name: "Gemini CLI",
                config_format: ConfigFormat::EnvBased,
            },
            TargetApp::OpenCode => ToolMetadata {
                id: "opencode",
                display_name: "OpenCode",
                config_format: ConfigFormat::Json,
            },
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_variants_have_metadata() {
        for app in TargetApp::all() {
            let meta = app.metadata();
            assert_eq!(meta.id, app.as_str(), "metadata id must match as_str");
            assert!(!meta.display_name.is_empty());
        }
    }

    #[test]
    fn test_from_str_roundtrip() {
        for app in TargetApp::all() {
            let s = app.as_str();
            assert_eq!(TargetApp::from_str(s), Some(app));
        }
        assert_eq!(TargetApp::from_str("unknown"), None);
    }

    #[test]
    fn test_new_tools_registered() {
        assert_eq!(TargetApp::from_str("gemini"), Some(TargetApp::Gemini));
        assert_eq!(TargetApp::from_str("opencode"), Some(TargetApp::OpenCode));
        assert_eq!(TargetApp::Gemini.metadata().config_format, ConfigFormat::EnvBased);
        assert_eq!(TargetApp::OpenCode.metadata().config_format, ConfigFormat::Json);
        assert_eq!(TargetApp::Codex.metadata().config_format, ConfigFormat::Toml);
    }
}

