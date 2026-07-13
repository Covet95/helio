//! 模型可用性探活（与目标工具接入协议对齐）
//! 供 CLI 与 Tauri GUI 共用。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelTestResult {
    pub model: String,
    pub endpoint: String,
    /// chat_completions | responses | anthropic_messages | gemini
    pub protocol: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_label: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeProtocol {
    ChatCompletions,
    Responses,
    AnthropicMessages,
    GeminiGenerate,
}

impl ProbeProtocol {
    fn as_str(self) -> &'static str {
        match self {
            ProbeProtocol::ChatCompletions => "chat_completions",
            ProbeProtocol::Responses => "responses",
            ProbeProtocol::AnthropicMessages => "anthropic_messages",
            ProbeProtocol::GeminiGenerate => "gemini",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SuccessCheck {
    ChatChoices,
    ResponsesOutputOrStatus,
    AnthropicContent,
    GeminiCandidates,
}

#[derive(Debug, Clone)]
struct ProbePlan {
    protocol: ProbeProtocol,
    endpoint: String,
    headers: Vec<(String, String)>,
    body: serde_json::Value,
    success: SuccessCheck,
}

#[derive(Deserialize)]
struct ChatCompletionResponse {
    #[serde(default)]
    choices: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct ResponsesResponse {
    #[serde(default)]
    output: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Deserialize)]
struct AnthropicMessagesResponse {
    #[serde(default)]
    content: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    #[allow(dead_code)]
    r#type: Option<String>,
}

#[derive(Deserialize)]
struct GeminiGenerateResponse {
    #[serde(default)]
    candidates: Vec<serde_json::Value>,
}

/// Anthropic 协议兼容子路径；仅用于 models 列表与 OpenAI-compatible 探活
const COMPAT_SUFFIXES: &[&str] = &[
    "/api/claudecode",
    "/api/anthropic",
    "/apps/anthropic",
    "/api/coding",
    "/claudecode",
    "/anthropic",
    "/step_plan",
    "/coding",
    "/claude",
];

const PROVIDER_MODELS_URLS: &[(&str, &str)] = &[
    ("bigmodel.cn", "https://open.bigmodel.cn/api/paas/v4/models"),
    ("z.ai", "https://api.z.ai/api/paas/v4/models"),
    ("deepseek.com", "https://api.deepseek.com/models"),
    ("moonshot.cn", "https://api.moonshot.cn/v1/models"),
    ("openrouter.ai", "https://openrouter.ai/api/v1/models"),
    ("siliconflow.cn", "https://api.siliconflow.cn/v1/models"),
    (
        "dashscope.aliyuncs.com",
        "https://dashscope.aliyuncs.com/compatible-mode/v1/models",
    ),
];

fn provider_models_url(api_url: &str) -> Option<String> {
    let lower = api_url.to_lowercase();
    PROVIDER_MODELS_URLS
        .iter()
        .find(|(pat, _)| lower.contains(pat))
        .map(|(_, url)| url.to_string())
}

fn is_local_url(api_url: &str) -> bool {
    let lower = api_url.to_lowercase();
    lower.contains("127.0.0.1") || lower.contains("localhost") || lower.contains("0.0.0.0")
}

fn http_client(api_url: &str) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder().timeout(std::time::Duration::from_secs(15));
    if is_local_url(api_url) {
        builder = builder.no_proxy();
    }
    builder.build().map_err(|e| e.to_string())
}

fn trim_base(api_url: &str) -> String {
    api_url.trim().trim_end_matches('/').to_string()
}

/// OpenAI-compatible base：可剥 Anthropic 兼容后缀 + provider 特判（仅列表/兼容探活）
fn openai_compat_base(api_url: &str) -> String {
    if let Some(url) = provider_models_url(api_url) {
        return url
            .trim_end_matches("/models")
            .trim_end_matches('/')
            .to_string();
    }
    let base = trim_base(api_url);
    for suffix in COMPAT_SUFFIXES {
        if let Some(stripped) = base.strip_suffix(suffix) {
            return stripped.trim_end_matches('/').to_string();
        }
    }
    base
}

/// 原样 base（不剥 Anthropic 后缀）—— Claude / Hermes·OpenClaw anthropic 用
fn raw_base(api_url: &str) -> String {
    trim_base(api_url)
}

fn join_openai_path(base: &str, leaf: &str) -> String {
    if base.ends_with("/v1") || base.ends_with("/paas/v4") {
        format!("{}/{}", base, leaf)
    } else {
        format!("{}/v1/{}", base, leaf)
    }
}

fn chat_completions_url_compat(api_url: &str) -> String {
    join_openai_path(&openai_compat_base(api_url), "chat/completions")
}

#[cfg(test)]
fn responses_url_compat(api_url: &str) -> String {
    join_openai_path(&openai_compat_base(api_url), "responses")
}

/// 不剥后缀的 OpenAI 路径（Codex / Hermes chat|responses 在用户 base 上拼）
fn chat_completions_url_raw(api_url: &str) -> String {
    join_openai_path(&raw_base(api_url), "chat/completions")
}

fn responses_url_raw(api_url: &str) -> String {
    join_openai_path(&raw_base(api_url), "responses")
}

fn anthropic_messages_url(api_url: &str) -> String {
    let base = raw_base(api_url);
    if base.ends_with("/v1") {
        format!("{}/messages", base)
    } else {
        format!("{}/v1/messages", base)
    }
}

fn is_gemini_official(api_url: &str) -> bool {
    api_url
        .to_lowercase()
        .contains("generativelanguage.googleapis.com")
}

fn gemini_generate_url(api_url: &str, model: &str) -> String {
    let base = raw_base(api_url);
    // 预设常为 https://generativelanguage.googleapis.com
    if base.contains("/models/") && base.contains(":generateContent") {
        return base;
    }
    if base.contains("/v1beta") || base.contains("/v1/") {
        format!("{}/models/{}:generateContent", base, model)
    } else {
        format!("{}/v1beta/models/{}:generateContent", base, model)
    }
}

fn normalize_hermes_openclaw_mode(api_mode: Option<&str>) -> &'static str {
    match api_mode.map(str::trim).filter(|s| !s.is_empty()) {
        None | Some("chat_completions") | Some("openai-completions") | Some("chat") => {
            "chat_completions"
        }
        Some("anthropic_messages") | Some("anthropic-messages") => "anthropic_messages",
        Some("codex_responses")
        | Some("openai-responses")
        | Some("responses")
        | Some("openai_responses") => "responses",
        Some(_) => "chat_completions",
    }
}

fn bearer_headers(token: &str) -> Vec<(String, String)> {
    vec![
        (
            "Authorization".into(),
            format!("Bearer {}", token),
        ),
        ("Content-Type".into(), "application/json".into()),
    ]
}

fn anthropic_headers(api_key: &str) -> Vec<(String, String)> {
    vec![
        ("x-api-key".into(), api_key.to_string()),
        ("anthropic-version".into(), "2023-06-01".into()),
        ("Content-Type".into(), "application/json".into()),
    ]
}

fn chat_body(model: &str) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": "ping"}],
        "max_tokens": 1,
        "temperature": 0,
        "stream": false
    })
}

fn responses_body(model: &str) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "input": "ping",
        "max_output_tokens": 16,
        "stream": false
    })
}

fn anthropic_body(model: &str) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "max_tokens": 1,
        "messages": [{"role": "user", "content": "ping"}]
    })
}

fn gemini_body() -> serde_json::Value {
    serde_json::json!({
        "contents": [{
            "role": "user",
            "parts": [{"text": "ping"}]
        }],
        "generationConfig": { "maxOutputTokens": 1 }
    })
}

/// 解析探活计划（纯函数，单测覆盖）
fn resolve_probe_plan(
    target_app: &str,
    api_url: &str,
    api_key: &str,
    model: &str,
    wire_api: Option<&str>,
    api_mode: Option<&str>,
    experimental_bearer_token: Option<&str>,
) -> Result<ProbePlan, String> {
    let app = target_app.trim().to_lowercase();
    let key = api_key.trim();
    let model = model.trim();
    if key.is_empty() {
        return Err("需要 API Key 才能测试模型".into());
    }
    if model.is_empty() {
        return Err("先选择或填写模型".into());
    }

    match app.as_str() {
        "claude-code" => Ok(ProbePlan {
            protocol: ProbeProtocol::AnthropicMessages,
            endpoint: anthropic_messages_url(api_url),
            headers: anthropic_headers(key),
            body: anthropic_body(model),
            success: SuccessCheck::AnthropicContent,
        }),
        "codex" => {
            let wire = wire_api.map(str::trim).filter(|s| !s.is_empty()).unwrap_or("responses");
            let token = experimental_bearer_token
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(key);
            if wire == "chat" {
                Ok(ProbePlan {
                    protocol: ProbeProtocol::ChatCompletions,
                    // Codex base 原样（不剥 Anthropic 后缀）
                    endpoint: chat_completions_url_raw(api_url),
                    headers: bearer_headers(token),
                    body: chat_body(model),
                    success: SuccessCheck::ChatChoices,
                })
            } else {
                // 默认 responses（与 adapter 新 provider 默认一致）
                Ok(ProbePlan {
                    protocol: ProbeProtocol::Responses,
                    endpoint: responses_url_raw(api_url),
                    headers: bearer_headers(token),
                    body: responses_body(model),
                    success: SuccessCheck::ResponsesOutputOrStatus,
                })
            }
        }
        "gemini" => {
            if is_gemini_official(api_url) {
                let mut endpoint = gemini_generate_url(api_url, model);
                // key 走 query，避免与部分代理 header 行为不一致
                let sep = if endpoint.contains('?') { "&" } else { "?" };
                endpoint = format!(
                    "{}{}key={}",
                    endpoint,
                    sep,
                    urlencoding_minimal(key)
                );
                Ok(ProbePlan {
                    protocol: ProbeProtocol::GeminiGenerate,
                    endpoint,
                    headers: vec![("Content-Type".into(), "application/json".into())],
                    body: gemini_body(),
                    success: SuccessCheck::GeminiCandidates,
                })
            } else {
                Ok(ProbePlan {
                    protocol: ProbeProtocol::ChatCompletions,
                    endpoint: chat_completions_url_compat(api_url),
                    headers: bearer_headers(key),
                    body: chat_body(model),
                    success: SuccessCheck::ChatChoices,
                })
            }
        }
        "opencode" => Ok(ProbePlan {
            protocol: ProbeProtocol::ChatCompletions,
            endpoint: chat_completions_url_compat(api_url),
            headers: bearer_headers(key),
            body: chat_body(model),
            success: SuccessCheck::ChatChoices,
        }),
        "hermes" | "openclaw" => match normalize_hermes_openclaw_mode(api_mode) {
            "anthropic_messages" => Ok(ProbePlan {
                protocol: ProbeProtocol::AnthropicMessages,
                endpoint: anthropic_messages_url(api_url),
                headers: anthropic_headers(key),
                body: anthropic_body(model),
                success: SuccessCheck::AnthropicContent,
            }),
            "responses" => Ok(ProbePlan {
                protocol: ProbeProtocol::Responses,
                endpoint: responses_url_raw(api_url),
                headers: bearer_headers(key),
                body: responses_body(model),
                success: SuccessCheck::ResponsesOutputOrStatus,
            }),
            _ => Ok(ProbePlan {
                protocol: ProbeProtocol::ChatCompletions,
                endpoint: chat_completions_url_raw(api_url),
                headers: bearer_headers(key),
                body: chat_body(model),
                success: SuccessCheck::ChatChoices,
            }),
        },
        other => Err(format!("未知 target_app: {}", other)),
    }
}

/// 极简 query escape（key 通常为 sk- 字符集）
fn urlencoding_minimal(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn validate_success(plan: &ProbePlan, text: &str) -> Result<(), String> {
    match plan.success {
        SuccessCheck::ChatChoices => {
            let parsed: ChatCompletionResponse = serde_json::from_str(text)
                .map_err(|e| format!("{} 解析失败: {}", plan.endpoint, e))?;
            if parsed.choices.is_empty() {
                return Err(format!("{} 没有返回 completion choice", plan.endpoint));
            }
        }
        SuccessCheck::ResponsesOutputOrStatus => {
            let parsed: ResponsesResponse = serde_json::from_str(text)
                .map_err(|e| format!("{} 解析失败: {}", plan.endpoint, e))?;
            if parsed.output.is_none() && parsed.status.is_none() {
                return Err(format!("{} 未返回有效 responses 结构", plan.endpoint));
            }
        }
        SuccessCheck::AnthropicContent => {
            let parsed: AnthropicMessagesResponse = serde_json::from_str(text)
                .map_err(|e| format!("{} 解析失败: {}", plan.endpoint, e))?;
            // 成功响应通常有 content 数组或 type=message
            let has_content = parsed
                .content
                .as_ref()
                .map(|c| !c.is_empty())
                .unwrap_or(false);
            let has_type = parsed.r#type.as_deref().is_some();
            if !has_content && !has_type {
                // 部分中转只回 id/model；若 JSON 对象且 2xx，放宽：只要不是 error 形
                if text.contains("\"error\"") {
                    return Err(format!("{} 返回 error 结构", plan.endpoint));
                }
                // 仍要求可解析为对象（上面已成功）
            }
        }
        SuccessCheck::GeminiCandidates => {
            let parsed: GeminiGenerateResponse = serde_json::from_str(text)
                .map_err(|e| format!("{} 解析失败: {}", plan.endpoint, e))?;
            if parsed.candidates.is_empty() && text.contains("\"error\"") {
                return Err(format!("{} 返回 error 结构", plan.endpoint));
            }
            // 部分账号安全过滤可能空 candidates 但仍 2xx；有 error 才失败
            if parsed.candidates.is_empty() && !text.contains("candidates") {
                return Err(format!("{} 未返回 candidates", plan.endpoint));
            }
        }
    }
    Ok(())
}

/// 执行已解析的探活计划（供 test_model / failover / 批量探活复用）
async fn execute_probe_plan(api_url: &str, plan: &ProbePlan) -> Result<(), String> {
    let client = http_client(api_url)?;
    let mut req = client.post(&plan.endpoint).json(&plan.body);
    for (k, v) in &plan.headers {
        req = req.header(k, v);
    }

    let response = req
        .send()
        .await
        .map_err(|e| format!("{} 请求失败: {}", plan.endpoint, e))?;

    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|e| format!("{} 读取响应失败: {}", plan.endpoint, e))?;
    if !status.is_success() {
        let detail = text.trim();
        if detail.is_empty() {
            return Err(format!("{} 返回 {}", plan.endpoint, status));
        }
        return Err(format!("{} 返回 {}: {}", plan.endpoint, status, detail));
    }

    validate_success(plan, &text)
}

/// 按参数探活；成功返回 ModelTestResult，失败返回 Err 文案。
pub async fn probe_with_params(
    target_app: &str,
    api_url: &str,
    api_key: &str,
    model: &str,
    wire_api: Option<&str>,
    api_mode: Option<&str>,
    experimental_bearer_token: Option<&str>,
    key_label: Option<String>,
) -> Result<ModelTestResult, String> {
    let plan = resolve_probe_plan(
        target_app,
        api_url,
        api_key,
        model,
        wire_api,
        api_mode,
        experimental_bearer_token,
    )?;
    execute_probe_plan(api_url, &plan).await?;
    Ok(ModelTestResult {
        model: model.trim().to_string(),
        endpoint: plan.endpoint,
        protocol: plan.protocol.as_str().to_string(),
        key_label,
    })
}

/// 单把 key 的探活结果（failover / 批量）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyProbeResult {
    pub key_id: String,
    pub label: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
}

/// failover 结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailoverResult {
    pub success: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_key_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_label: Option<String>,
    pub tried: Vec<KeyProbeResult>,
    pub re_switched: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_keeps_anthropic_suffix_and_x_api_key() {
        let plan = resolve_probe_plan(
            "claude-code",
            "https://api.deepseek.com/anthropic",
            "sk-test",
            "deepseek-chat",
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(plan.protocol, ProbeProtocol::AnthropicMessages);
        assert_eq!(
            plan.endpoint,
            "https://api.deepseek.com/anthropic/v1/messages"
        );
        assert!(plan
            .headers
            .iter()
            .any(|(k, v)| k == "x-api-key" && v == "sk-test"));
        assert!(!plan.endpoint.contains("chat/completions"));
    }

    #[test]
    fn codex_empty_wire_defaults_to_responses() {
        let plan = resolve_probe_plan(
            "codex",
            "https://api.openai.com/v1",
            "sk",
            "gpt-5.5",
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(plan.protocol, ProbeProtocol::Responses);
        assert_eq!(plan.endpoint, "https://api.openai.com/v1/responses");
    }

    #[test]
    fn codex_chat_wire() {
        let plan = resolve_probe_plan(
            "codex",
            "https://proxy.example/v1",
            "sk",
            "gpt",
            Some("chat"),
            None,
            None,
        )
        .unwrap();
        assert_eq!(plan.protocol, ProbeProtocol::ChatCompletions);
        assert!(plan.endpoint.ends_with("/chat/completions"));
    }

    #[test]
    fn codex_experimental_bearer_overrides_key() {
        let plan = resolve_probe_plan(
            "codex",
            "https://api.openai.com/v1",
            "sk-main",
            "gpt",
            Some("responses"),
            None,
            Some("sk-exp-token"),
        )
        .unwrap();
        let auth = plan
            .headers
            .iter()
            .find(|(k, _)| k == "Authorization")
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert_eq!(auth, "Bearer sk-exp-token");
    }

    #[test]
    fn hermes_anthropic_mode_no_openai_chat() {
        let plan = resolve_probe_plan(
            "hermes",
            "http://127.0.0.1:8317/v1",
            "sk",
            "claude-opus-4-8",
            None,
            Some("anthropic_messages"),
            None,
        )
        .unwrap();
        assert_eq!(plan.protocol, ProbeProtocol::AnthropicMessages);
        assert_eq!(plan.endpoint, "http://127.0.0.1:8317/v1/messages");
        assert!(plan.headers.iter().any(|(k, _)| k == "x-api-key"));
    }

    #[test]
    fn openclaw_responses_mode() {
        let plan = resolve_probe_plan(
            "openclaw",
            "http://127.0.0.1:8317/v1",
            "sk",
            "m",
            None,
            Some("codex_responses"),
            None,
        )
        .unwrap();
        assert_eq!(plan.protocol, ProbeProtocol::Responses);
        assert_eq!(plan.endpoint, "http://127.0.0.1:8317/v1/responses");
    }

    #[test]
    fn hermes_default_chat_raw_base() {
        let plan = resolve_probe_plan(
            "hermes",
            "https://api.freemodel.dev/v1",
            "sk",
            "gpt-5.5",
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(plan.protocol, ProbeProtocol::ChatCompletions);
        assert_eq!(
            plan.endpoint,
            "https://api.freemodel.dev/v1/chat/completions"
        );
    }

    #[test]
    fn opencode_uses_compat_chat() {
        let plan = resolve_probe_plan(
            "opencode",
            "https://api.deepseek.com/anthropic",
            "sk",
            "deepseek-chat",
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(plan.protocol, ProbeProtocol::ChatCompletions);
        assert_eq!(
            plan.endpoint,
            "https://api.deepseek.com/v1/chat/completions"
        );
    }

    #[test]
    fn gemini_official_generate() {
        let plan = resolve_probe_plan(
            "gemini",
            "https://generativelanguage.googleapis.com",
            "AIzaSyTest",
            "gemini-2.0-flash",
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(plan.protocol, ProbeProtocol::GeminiGenerate);
        assert!(plan.endpoint.contains("/v1beta/models/gemini-2.0-flash:generateContent"));
        assert!(plan.endpoint.contains("key=AIzaSyTest"));
    }

    #[test]
    fn missing_model_errors() {
        let err = resolve_probe_plan("codex", "https://x", "sk", "  ", None, None, None)
            .unwrap_err();
        assert!(err.contains("模型"));
    }

    // 兼容旧测试命名
    #[test]
    fn test_chat_completions_url_keeps_openai_v1() {
        assert_eq!(
            chat_completions_url_compat("https://api.openai.com/v1"),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn test_responses_url_keeps_v1() {
        assert_eq!(
            responses_url_compat("https://api.openai.com/v1"),
            "https://api.openai.com/v1/responses"
        );
    }

    #[test]
    fn test_chat_completions_url_strips_anthropic_compat_suffix() {
        assert_eq!(
            chat_completions_url_compat("https://api.deepseek.com/anthropic"),
            "https://api.deepseek.com/v1/chat/completions"
        );
    }

    #[test]
    fn test_chat_completions_url_uses_provider_override() {
        assert_eq!(
            chat_completions_url_compat("https://open.bigmodel.cn/api/anthropic"),
            "https://open.bigmodel.cn/api/paas/v4/chat/completions"
        );
    }
}
