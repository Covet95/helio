use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Claude Code 专用字段（JSON flatten → IPC 仍为顶层键）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClaudeProfileFields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_mapping: Option<HashMap<String, String>>,
}

/// Codex `/model` catalog 条目（精简；其余元数据由适配器模板填充）
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodexCatalogModel {
    pub slug: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<i64>,
    /// Documented Codex reasoning levels supported by this model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_levels: Option<Vec<String>>,
    /// Legacy import compatibility. New profiles use `reasoning_levels`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_reasoning: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_images: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_tool_calls: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_web_search: Option<bool>,
}

/// Codex 专用字段（JSON flatten → IPC 仍为顶层键）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CodexProfileFields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wire_api: Option<String>,
    /// Provider-scoped environment variable containing the API key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_openai_auth: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub experimental_bearer_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    /// Custom provider capability required for standalone web search.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_standalone_web_search: Option<bool>,
    /// Built-in Amazon Bedrock provider override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aws_profile: Option<String>,
    /// Built-in Amazon Bedrock provider override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aws_region: Option<String>,
    /// 写入 `model_catalog.json` 的模型表；空/缺省 = 切换时不改本机 catalog
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_models: Option<Vec<CodexCatalogModel>>,
}

/// OpenCode 专用字段（JSON flatten → IPC 仍为顶层键）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OpenCodeProfileFields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<String>>,
    /// OpenCode provider SDK mode: chat_completions or responses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opencode_api_mode: Option<String>,
    /// Per-model OpenCode config written beneath provider.<id>.models.<model>.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_configs: Option<HashMap<String, serde_json::Value>>,
}

/// Model IDs last written by Helio for each OpenCode provider.
///
/// This state is kept in the Helio database rather than opencode.json so manual
/// model entries can be distinguished from entries managed by a profile.
pub type OpenCodeManagedModelState = HashMap<String, Vec<String>>;

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

/// 同一 profile 下的一把 API Key（池 + 手动活跃）
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiKeyEntry {
    pub id: String,
    #[serde(default)]
    pub label: String,
    pub key: String,
    #[serde(default)]
    pub is_active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_probe_ok: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_probed_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
}

/// API Profile - 只存储 API 相关信息
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApiProfile {
    pub id: Option<i64>,
    pub name: String,
    pub provider: String,
    pub api_url: String,
    /// 活跃 key 冗余列：始终等于 api_keys 中 is_active 的那把（adapters 只读此字段）
    pub api_key: String,
    /// 多 key 池；空/缺省时仅用 api_key
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_keys: Option<Vec<ApiKeyEntry>>,
    /// 默认模型（跨工具）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// 是否启用 1M 上下文窗口（各适配器自行解释，不互相调用）
    ///
    /// - `Some(true)` → 1_000_000
    /// - `Some(false)` → 标准上下文：Grok 默认 500_000，其它 200_000
    /// - `None` → 多数适配器不覆写已有配置
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

/// 1M context window tokens.
pub const CONTEXT_LENGTH_1M: i64 = 1_000_000;
/// Grok-family standard context (when 1M toggle is off).
pub const CONTEXT_LENGTH_GROK: i64 = 500_000;
/// Non-Grok standard context (when 1M toggle is off).
pub const CONTEXT_LENGTH_STANDARD: i64 = 200_000;

impl ApiProfile {
    /// Whether this profile's model id looks like a Grok model.
    ///
    /// Uses the profile `model` field only (not provider name), so custom
    /// OpenAI-compatible proxies serving Grok still match.
    pub fn is_grok_model(&self) -> bool {
        Self::model_is_grok(self.model.as_deref())
    }

    pub fn model_is_grok(model: Option<&str>) -> bool {
        model
            .map(|m| {
                let lower = m.trim().to_ascii_lowercase();
                // xai/grok-*, openai/grok-*, grok-4.5, grok4, etc.
                lower.contains("grok")
            })
            .unwrap_or(false)
    }

    /// Standard (non-1M) context length for this profile.
    /// Grok defaults to 500k; everything else defaults to 200k.
    pub fn standard_context_length(&self) -> i64 {
        if self.is_grok_model() {
            CONTEXT_LENGTH_GROK
        } else {
            CONTEXT_LENGTH_STANDARD
        }
    }

    /// 生成简单唯一 id（无需外部依赖）
    pub fn new_key_id() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("k{nanos}")
    }

    /// 将 legacy 单 api_key 与 api_keys 归一：非空池恰好一把 active，api_key 镜像 active。
    pub fn normalize_keys(&mut self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let mut keys = self.api_keys.take().unwrap_or_default();
        // 丢掉空 secret
        keys.retain(|e| !e.key.trim().is_empty());

        if keys.is_empty() {
            let k = self.api_key.trim();
            if !k.is_empty() {
                keys.push(ApiKeyEntry {
                    id: Self::new_key_id(),
                    label: "default".into(),
                    key: k.to_string(),
                    is_active: true,
                    last_probe_ok: None,
                    last_probed_at: None,
                    created_at: Some(now),
                });
            }
        } else {
            // 若 api_key 有值但不在池中，当作额外一把（不强制 active）
            let k = self.api_key.trim();
            if !k.is_empty() && !keys.iter().any(|e| e.key == k) {
                keys.push(ApiKeyEntry {
                    id: Self::new_key_id(),
                    label: "legacy".into(),
                    key: k.to_string(),
                    is_active: false,
                    last_probe_ok: None,
                    last_probed_at: None,
                    created_at: Some(now),
                });
            }
            let active_count = keys.iter().filter(|e| e.is_active).count();
            if active_count == 0 {
                keys[0].is_active = true;
            } else if active_count > 1 {
                let mut seen = false;
                for e in keys.iter_mut() {
                    if e.is_active {
                        if seen {
                            e.is_active = false;
                        } else {
                            seen = true;
                        }
                    }
                }
            }
        }

        if let Some(active) = keys.iter().find(|e| e.is_active) {
            self.api_key = active.key.clone();
        } else if keys.is_empty() {
            // keep api_key as-is (可能为空，保存时由表单校验)
        }

        self.api_keys = if keys.is_empty() { None } else { Some(keys) };
    }

    pub fn active_key(&self) -> &str {
        if let Some(keys) = self.api_keys.as_ref() {
            if let Some(a) = keys.iter().find(|e| e.is_active) {
                return a.key.as_str();
            }
        }
        self.api_key.as_str()
    }

    /// 将指定 id 设为唯一活跃 key，并同步 api_key。找不到则 false。
    pub fn set_active_key_id(&mut self, key_id: &str) -> bool {
        self.normalize_keys();
        let Some(keys) = self.api_keys.as_mut() else {
            return false;
        };
        if !keys.iter().any(|e| e.id == key_id) {
            return false;
        }
        for e in keys.iter_mut() {
            e.is_active = e.id == key_id;
        }
        if let Some(a) = keys.iter().find(|e| e.is_active) {
            self.api_key = a.key.clone();
        }
        true
    }

    /// 按 label（大小写不敏感）或 id 设活跃
    pub fn set_active_key_ref(&mut self, id_or_label: &str) -> bool {
        self.normalize_keys();
        let needle = id_or_label.trim();
        let Some(keys) = self.api_keys.as_ref() else {
            return false;
        };
        let id = keys
            .iter()
            .find(|e| e.id == needle || e.label.eq_ignore_ascii_case(needle))
            .map(|e| e.id.clone());
        match id {
            Some(id) => self.set_active_key_id(&id),
            None => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TargetApp {
    ClaudeCode,
    Codex,
    Pi,
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
            TargetApp::Pi => "pi",
            TargetApp::OpenCode => "opencode",
            TargetApp::Hermes => "hermes",
            TargetApp::OpenClaw => "openclaw",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "claude-code" => Some(TargetApp::ClaudeCode),
            "codex" => Some(TargetApp::Codex),
            "pi" => Some(TargetApp::Pi),
            "opencode" => Some(TargetApp::OpenCode),
            "hermes" => Some(TargetApp::Hermes),
            "openclaw" => Some(TargetApp::OpenClaw),
            _ => None,
        }
    }

    pub fn all() -> Vec<Self> {
        vec![
            TargetApp::ClaudeCode,
            TargetApp::Codex,
            TargetApp::Pi,
            TargetApp::OpenCode,
            TargetApp::Hermes,
            TargetApp::OpenClaw,
        ]
    }
}

impl std::str::FromStr for TargetApp {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        TargetApp::parse(value).ok_or(())
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
            api_keys: None,
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
        // 按字符切片（key 可能是多字节 UTF-8），字节切片会越界 panic
        let chars: Vec<char> = key.chars().collect();
        if chars.len() > 15 {
            let head: String = chars[..10].iter().collect();
            let tail: String = chars[chars.len() - 5..].iter().collect();
            format!("{head}...{tail}")
        } else {
            "***".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_legacy_single_key() {
        let mut p = ApiProfile {
            api_key: "sk-main".into(),
            api_keys: None,
            ..Default::default()
        };
        p.normalize_keys();
        let keys = p.api_keys.as_ref().unwrap();
        assert_eq!(keys.len(), 1);
        assert!(keys[0].is_active);
        assert_eq!(keys[0].key, "sk-main");
        assert_eq!(p.api_key, "sk-main");
        assert_eq!(p.active_key(), "sk-main");
    }

    #[test]
    fn normalize_two_keys_exactly_one_active() {
        let mut p = ApiProfile {
            api_key: "sk-a".into(),
            api_keys: Some(vec![
                ApiKeyEntry {
                    id: "1".into(),
                    label: "a".into(),
                    key: "sk-a".into(),
                    is_active: false,
                    ..Default::default()
                },
                ApiKeyEntry {
                    id: "2".into(),
                    label: "b".into(),
                    key: "sk-b".into(),
                    is_active: true,
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };
        p.normalize_keys();
        assert_eq!(p.api_key, "sk-b");
        assert_eq!(p.active_key(), "sk-b");
        assert_eq!(
            p.api_keys
                .as_ref()
                .unwrap()
                .iter()
                .filter(|e| e.is_active)
                .count(),
            1
        );
    }

    #[test]
    fn set_active_key_id_switches_api_key() {
        let mut p = ApiProfile {
            api_keys: Some(vec![
                ApiKeyEntry {
                    id: "1".into(),
                    label: "a".into(),
                    key: "sk-a".into(),
                    is_active: true,
                    ..Default::default()
                },
                ApiKeyEntry {
                    id: "2".into(),
                    label: "b".into(),
                    key: "sk-b".into(),
                    is_active: false,
                    ..Default::default()
                },
            ]),
            api_key: "sk-a".into(),
            ..Default::default()
        };
        assert!(p.set_active_key_id("2"));
        assert_eq!(p.api_key, "sk-b");
        assert!(!p.set_active_key_id("missing"));
    }

    #[test]
    fn test_from_str_roundtrip() {
        for app in [
            TargetApp::ClaudeCode,
            TargetApp::Codex,
            TargetApp::Pi,
            TargetApp::OpenCode,
            TargetApp::Hermes,
            TargetApp::OpenClaw,
        ] {
            assert_eq!(TargetApp::parse(app.as_str()), Some(app));
        }
        assert_eq!(TargetApp::parse("unknown"), None);
        assert_eq!(TargetApp::parse("gemini"), None);
    }

    #[test]
    fn test_new_tools_registered() {
        assert_eq!(TargetApp::parse("pi"), Some(TargetApp::Pi));
        assert_eq!(TargetApp::parse("opencode"), Some(TargetApp::OpenCode));
        assert_eq!(TargetApp::parse("hermes"), Some(TargetApp::Hermes));
        assert_eq!(TargetApp::parse("openclaw"), Some(TargetApp::OpenClaw));
    }

    #[test]
    fn test_serde_matches_as_str() {
        for app in [
            TargetApp::ClaudeCode,
            TargetApp::Codex,
            TargetApp::Pi,
            TargetApp::OpenCode,
            TargetApp::Hermes,
            TargetApp::OpenClaw,
        ] {
            let json = serde_json::to_string(&app).unwrap();
            assert_eq!(json, format!("\"{}\"", app.as_str()));
            let back: TargetApp = serde_json::from_str(&format!("\"{}\"", app.as_str())).unwrap();
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
                catalog_models: Some(vec![CodexCatalogModel {
                    slug: "gpt-5.6-sol".into(),
                    display_name: Some("GPT-5.6 Sol".into()),
                    ..Default::default()
                }]),
                ..Default::default()
            },
            opencode: OpenCodeProfileFields {
                models: Some(vec!["a".into()]),
                ..Default::default()
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
        assert_eq!(v["catalog_models"][0]["slug"], "gpt-5.6-sol");
        assert_eq!(v["catalog_models"][0]["display_name"], "GPT-5.6 Sol");
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
        assert_eq!(
            back.codex.catalog_models.as_ref().unwrap()[0].slug,
            "gpt-5.6-sol"
        );
        assert_eq!(back.opencode.models.as_ref().unwrap()[0], "a");
        // Flatten: last writer of same key wins on serialize; deserialize fills both
        // groups from top-level keys. Prefer tool-owned fields when target_app set.
        assert_eq!(back.openclaw.max_tokens, Some(128000));
    }

    #[cfg(not(feature = "tauri-gui"))]
    #[test]
    fn test_all_variants_roundtrip() {
        for app in TargetApp::all() {
            assert_eq!(TargetApp::parse(app.as_str()), Some(app));
        }
    }
}
