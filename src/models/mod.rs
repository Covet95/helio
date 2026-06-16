use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// API Profile - 只存储 API 相关信息
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApiProfile {
    pub id: Option<i64>,
    pub name: String,
    pub provider: String,
    pub api_url: String,
    pub api_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_mapping: Option<HashMap<String, String>>,
    /// 默认模型（如 gpt-5.5 / claude-opus-4）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// OpenCode 专用：provider 下挂载的模型 id 列表（多选）。
    /// 写入 opencode.json 的 provider.<id>.models，让 OpenCode 能列出/切换这些模型。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<String>>,
    /// 推理强度（Codex: low/medium/high/xhigh）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    /// 是否启用 1M 上下文窗口
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_1m: Option<bool>,
    /// 归属工具（per-tool profile；None = 通用，所有工具下都显示）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_app: Option<TargetApp>,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
}

/// 目标应用类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TargetApp {
    ClaudeCode,
    Codex,
    Gemini,
    // kebab 规则会得到 "open-code"，但全局(as_str/from_str/前端/DB)统一用 "opencode"
    #[serde(rename = "opencode")]
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

    #[cfg(not(feature = "tauri-gui"))]
    pub fn all() -> Vec<Self> {
        vec![
            TargetApp::ClaudeCode,
            TargetApp::Codex,
            TargetApp::Gemini,
            TargetApp::OpenCode,
        ]
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
            model_mapping,
            model: None,
            models: None,
            reasoning_effort: None,
            context_1m: None,
            target_app: None,
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
    fn test_from_str_roundtrip() {
        for app in [
            TargetApp::ClaudeCode,
            TargetApp::Codex,
            TargetApp::Gemini,
            TargetApp::OpenCode,
        ] {
            let s = app.as_str();
            assert_eq!(TargetApp::from_str(s), Some(app));
        }
        assert_eq!(TargetApp::from_str("unknown"), None);
    }

    #[test]
    fn test_new_tools_registered() {
        assert_eq!(TargetApp::from_str("gemini"), Some(TargetApp::Gemini));
        assert_eq!(TargetApp::from_str("opencode"), Some(TargetApp::OpenCode));
    }

    #[test]
    fn test_serde_matches_as_str() {
        // serde 序列化必须与 as_str 一致（前端/DB/IPC 全用同一套字符串）
        for app in [
            TargetApp::ClaudeCode,
            TargetApp::Codex,
            TargetApp::Gemini,
            TargetApp::OpenCode,
        ] {
            let json = serde_json::to_string(&app).unwrap();
            assert_eq!(
                json,
                format!("\"{}\"", app.as_str()),
                "{app:?} 的 serde 输出应等于 as_str"
            );
            // 反序列化：前端传的字符串（= as_str）必须能解析回来
            let back: TargetApp =
                serde_json::from_str(&format!("\"{}\"", app.as_str())).unwrap();
            assert_eq!(back, app);
        }
        // 关键回归：OpenCode 必须是 "opencode"（不是 kebab 的 "open-code"）
        assert_eq!(
            serde_json::to_string(&TargetApp::OpenCode).unwrap(),
            "\"opencode\""
        );
        serde_json::from_str::<TargetApp>("\"opencode\"").unwrap();
    }

    #[cfg(not(feature = "tauri-gui"))]
    #[test]
    fn test_all_variants_roundtrip() {
        for app in TargetApp::all() {
            assert_eq!(TargetApp::from_str(app.as_str()), Some(app));
        }
    }
}
