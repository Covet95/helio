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

pub struct ProbeRequest<'a> {
    pub target_app: &'a str,
    pub api_url: &'a str,
    pub api_key: &'a str,
    pub model: &'a str,
    pub wire_api: Option<&'a str>,
    pub api_mode: Option<&'a str>,
    pub experimental_bearer_token: Option<&'a str>,
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

/// 复用连接池：本地地址用 no_proxy client（避免本地代理环路），其余用默认 client。
/// 探测/批量/并发场景共享，避免每请求新建 Client 丢失 TCP/TLS 连接复用。
fn http_client(api_url: &str) -> Result<&'static reqwest::Client, String> {
    static LOCAL: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    static DEFAULT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    let client = if is_local_url(api_url) {
        LOCAL.get_or_init(|| {
            reqwest::Client::builder()
                .no_proxy()
                .build()
                .expect("build local http client")
        })
    } else {
        DEFAULT.get_or_init(reqwest::Client::new)
    };
    Ok(client)
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
        ("Authorization".into(), format!("Bearer {}", token)),
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
            let _ = (wire_api, experimental_bearer_token);
            Ok(ProbePlan {
                protocol: ProbeProtocol::Responses,
                endpoint: responses_url_raw(api_url),
                headers: bearer_headers(key),
                body: responses_body(model),
                success: SuccessCheck::ResponsesOutputOrStatus,
            })
        }
        "pi" => {
            // Pi: protocol from api_mode/wire_api; official Google host → generateContent
            let mode = api_mode.unwrap_or("").trim().to_lowercase();
            let wire = wire_api.unwrap_or("").trim().to_lowercase();
            if is_gemini_official(api_url)
                && !mode.contains("anthropic")
                && !mode.contains("response")
                && wire != "responses"
            {
                // key 走 x-goog-api-key header，绝不进 URL query（避免错误消息/日志泄漏）
                let mut headers = vec![("Content-Type".into(), "application/json".into())];
                headers.push(("x-goog-api-key".into(), key.to_string()));
                Ok(ProbePlan {
                    protocol: ProbeProtocol::GeminiGenerate,
                    endpoint: gemini_generate_url(api_url, model),
                    headers,
                    body: gemini_body(),
                    success: SuccessCheck::GeminiCandidates,
                })
            } else if mode.contains("anthropic")
                || mode == "anthropic_messages"
                || mode == "anthropic-messages"
            {
                Ok(ProbePlan {
                    protocol: ProbeProtocol::AnthropicMessages,
                    endpoint: anthropic_messages_url(api_url),
                    headers: anthropic_headers(key),
                    body: anthropic_body(model),
                    success: SuccessCheck::AnthropicContent,
                })
            } else if mode.contains("response")
                || mode == "codex_responses"
                || mode == "openai-responses"
                || wire == "responses"
            {
                Ok(ProbePlan {
                    protocol: ProbeProtocol::Responses,
                    endpoint: responses_url_raw(api_url),
                    headers: bearer_headers(key),
                    body: responses_body(model),
                    success: SuccessCheck::ResponsesOutputOrStatus,
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
        "opencode" => match normalize_hermes_openclaw_mode(api_mode) {
            "responses" => Ok(ProbePlan {
                protocol: ProbeProtocol::Responses,
                endpoint: responses_url_compat(api_url),
                headers: bearer_headers(key),
                body: responses_body(model),
                success: SuccessCheck::ResponsesOutputOrStatus,
            }),
            _ => Ok(ProbePlan {
                protocol: ProbeProtocol::ChatCompletions,
                endpoint: chat_completions_url_compat(api_url),
                headers: bearer_headers(key),
                body: chat_body(model),
                success: SuccessCheck::ChatChoices,
            }),
        },
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

/// 脱敏 endpoint 中的凭据类 query 参数（防御性：万一凭据进 URL，错误消息不泄漏）。
/// 匹配 key / api_key / apikey 等常见键名，值替换为 ***。
fn sanitize_endpoint(endpoint: &str) -> String {
    let Some((base, query)) = endpoint.split_once('?') else {
        return endpoint.to_string();
    };
    let mut pairs: Vec<String> = Vec::new();
    for seg in query.split('&') {
        if let Some((k, _)) = seg.split_once('=') {
            let k = k.trim();
            if k.eq_ignore_ascii_case("key")
                || k.eq_ignore_ascii_case("api_key")
                || k.eq_ignore_ascii_case("apikey")
            {
                pairs.push(format!("{k}=***"));
                continue;
            }
        }
        pairs.push(seg.to_string());
    }
    format!("{base}?{}", pairs.join("&"))
}

fn validate_success(plan: &ProbePlan, text: &str) -> Result<(), String> {
    match plan.success {
        SuccessCheck::ChatChoices => {
            let parsed: ChatCompletionResponse = serde_json::from_str(text)
                .map_err(|e| format!("{} 解析失败: {}", sanitize_endpoint(&plan.endpoint), e))?;
            if parsed.choices.is_empty() {
                return Err(format!(
                    "{} 没有返回 completion choice",
                    sanitize_endpoint(&plan.endpoint)
                ));
            }
        }
        SuccessCheck::ResponsesOutputOrStatus => {
            let parsed: ResponsesResponse = serde_json::from_str(text)
                .map_err(|e| format!("{} 解析失败: {}", sanitize_endpoint(&plan.endpoint), e))?;
            if parsed.output.is_none() && parsed.status.is_none() {
                return Err(format!(
                    "{} 未返回有效 responses 结构",
                    sanitize_endpoint(&plan.endpoint)
                ));
            }
        }
        SuccessCheck::AnthropicContent => {
            let parsed: AnthropicMessagesResponse = serde_json::from_str(text)
                .map_err(|e| format!("{} 解析失败: {}", sanitize_endpoint(&plan.endpoint), e))?;
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
                    return Err(format!(
                        "{} 返回 error 结构",
                        sanitize_endpoint(&plan.endpoint)
                    ));
                }
                // 仍要求可解析为对象（上面已成功）
            }
        }
        SuccessCheck::GeminiCandidates => {
            let parsed: GeminiGenerateResponse = serde_json::from_str(text)
                .map_err(|e| format!("{} 解析失败: {}", sanitize_endpoint(&plan.endpoint), e))?;
            if parsed.candidates.is_empty() && text.contains("\"error\"") {
                return Err(format!(
                    "{} 返回 error 结构",
                    sanitize_endpoint(&plan.endpoint)
                ));
            }
            // 部分账号安全过滤可能空 candidates 但仍 2xx；有 error 才失败
            if parsed.candidates.is_empty() && !text.contains("candidates") {
                return Err(format!(
                    "{} 未返回 candidates",
                    sanitize_endpoint(&plan.endpoint)
                ));
            }
        }
    }
    Ok(())
}

/// 执行已解析的探活计划（供 test_model / failover / 批量探活复用）
async fn execute_probe_plan(api_url: &str, plan: &ProbePlan) -> Result<(), String> {
    let client = http_client(api_url)?;
    let mut req = client
        .post(sanitize_endpoint(&plan.endpoint))
        .timeout(std::time::Duration::from_secs(15))
        .json(&plan.body);
    for (k, v) in &plan.headers {
        req = req.header(k, v);
    }

    let response = req
        .send()
        .await
        .map_err(|e| format!("{} 请求失败: {}", sanitize_endpoint(&plan.endpoint), e))?;

    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|e| format!("{} 读取响应失败: {}", sanitize_endpoint(&plan.endpoint), e))?;
    if !status.is_success() {
        let detail = text.trim();
        // 截断响应体：远端/代理可能回显请求细节，避免超长或敏感内容进错误消息
        let detail = &detail[..detail.len().min(300)];
        if detail.is_empty() {
            return Err(format!(
                "{} 返回 {}",
                sanitize_endpoint(&plan.endpoint),
                status
            ));
        }
        return Err(format!(
            "{} 返回 {}: {}",
            sanitize_endpoint(&plan.endpoint),
            status,
            detail
        ));
    }

    validate_success(plan, &text)
}

/// 按参数探活；成功返回 ModelTestResult，失败返回 Err 文案。
pub async fn probe_with_params(request: ProbeRequest<'_>) -> Result<ModelTestResult, String> {
    let plan = resolve_probe_plan(
        request.target_app,
        request.api_url,
        request.api_key,
        request.model,
        request.wire_api,
        request.api_mode,
        request.experimental_bearer_token,
    )?;
    execute_probe_plan(request.api_url, &plan).await?;
    Ok(ModelTestResult {
        model: request.model.trim().to_string(),
        endpoint: sanitize_endpoint(&plan.endpoint),
        protocol: plan.protocol.as_str().to_string(),
        key_label: request.key_label,
    })
}

// ─── Reachability（对齐 CC Switch stream_check）────────────────────────────
//
// 仅探测 api_url / base_url 是否可达，**不发送真实大模型请求、不校验鉴权**：
// - 收到任意 HTTP 响应（200/4xx/5xx）即判定「可达」；
// - 仅 DNS / 连接被拒 / TLS / 超时等网络级错误判定「不可达」；
// - 延迟 = 收到响应头的耗时（TTFB）。
// 与 CC Switch `services/stream_check.rs` 同语义；熔断/鉴权仍由真实流量路径负责。

/// 可达性健康档位（对齐 CC Switch HealthStatus）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReachabilityStatus {
    Operational,
    Degraded,
    Failed,
}

/// 可达性探测结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReachabilityResult {
    pub status: ReachabilityStatus,
    pub success: bool,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_time_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    pub tested_at: i64,
    pub retry_count: u32,
    /// 实际探测的 URL
    pub endpoint: String,
}

/// 可达性探测配置（对齐 CC Switch StreamCheckConfig 默认）
#[derive(Debug, Clone)]
pub struct ReachabilityConfig {
    pub timeout_secs: u64,
    pub max_retries: u32,
    /// TTFB 超过此毫秒标 degraded（默认 6000，与 CC Switch 一致）
    pub degraded_threshold_ms: u64,
}

impl Default for ReachabilityConfig {
    fn default() -> Self {
        Self {
            timeout_secs: 8,
            max_retries: 1,
            degraded_threshold_ms: 6000,
        }
    }
}

fn should_retry_reachability(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    lower.contains("timeout") || lower.contains("abort") || lower.contains("timed out")
}

fn map_reachability_error(e: reqwest::Error) -> String {
    if e.is_timeout() {
        "Request timeout".into()
    } else if e.is_connect() {
        format!("Connection failed: {e}")
    } else {
        e.to_string()
    }
}

/// GET `api_url`：任意 HTTP 响应 = 可达；仅网络错误 = 失败。
/// 超时类失败最多重试 `max_retries` 次。
pub async fn probe_reachability(api_url: &str, config: &ReachabilityConfig) -> ReachabilityResult {
    let endpoint = api_url.trim().trim_end_matches('/').to_string();
    let tested_at = chrono::Utc::now().timestamp();
    if endpoint.is_empty() {
        return ReachabilityResult {
            status: ReachabilityStatus::Failed,
            success: false,
            message: "base_url 为空".into(),
            response_time_ms: None,
            http_status: None,
            tested_at,
            retry_count: 0,
            endpoint,
        };
    }

    let timeout = std::time::Duration::from_secs(config.timeout_secs);
    let mut last: Option<ReachabilityResult> = None;

    for attempt in 0..=config.max_retries {
        let start = std::time::Instant::now();
        // 复用共享连接池（http_client 内部按 本地/远程 分流 no_proxy）
        let client = match http_client(&endpoint) {
            Ok(c) => c,
            Err(e) => {
                return ReachabilityResult {
                    status: ReachabilityStatus::Failed,
                    success: false,
                    message: e,
                    response_time_ms: None,
                    http_status: None,
                    tested_at,
                    retry_count: attempt,
                    endpoint,
                };
            }
        };

        let result = client
            .get(&endpoint)
            .header("accept", "*/*")
            .header("accept-encoding", "identity")
            .timeout(timeout)
            .send()
            .await;

        let response_time = start.elapsed().as_millis() as u64;
        match result {
            Ok(resp) => {
                let status_code = resp.status().as_u16();
                let status = if response_time <= config.degraded_threshold_ms {
                    ReachabilityStatus::Operational
                } else {
                    ReachabilityStatus::Degraded
                };
                return ReachabilityResult {
                    status,
                    success: true,
                    message: "Reachable".into(),
                    response_time_ms: Some(response_time),
                    http_status: Some(status_code),
                    tested_at,
                    retry_count: attempt,
                    endpoint,
                };
            }
            Err(e) => {
                let msg = map_reachability_error(e);
                let r = ReachabilityResult {
                    status: ReachabilityStatus::Failed,
                    success: false,
                    message: msg.clone(),
                    response_time_ms: Some(response_time),
                    http_status: None,
                    tested_at,
                    retry_count: attempt,
                    endpoint: endpoint.clone(),
                };
                if should_retry_reachability(&msg) && attempt < config.max_retries {
                    last = Some(r);
                    continue;
                }
                return r;
            }
        }
    }

    last.unwrap_or(ReachabilityResult {
        status: ReachabilityStatus::Failed,
        success: false,
        message: "Check failed".into(),
        response_time_ms: None,
        http_status: None,
        tested_at,
        retry_count: config.max_retries,
        endpoint,
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
    fn codex_legacy_chat_wire_is_normalized_to_responses() {
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
        assert_eq!(plan.protocol, ProbeProtocol::Responses);
        assert!(plan.endpoint.ends_with("/responses"));
    }

    #[test]
    fn codex_legacy_bearer_is_ignored() {
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
        assert_eq!(auth, "Bearer sk-main");
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
    fn opencode_responses_mode_uses_responses_endpoint() {
        let plan = resolve_probe_plan(
            "opencode",
            "https://api.deepseek.com/anthropic",
            "sk",
            "deepseek-chat",
            None,
            Some("responses"),
            None,
        )
        .unwrap();
        assert_eq!(plan.protocol, ProbeProtocol::Responses);
        assert_eq!(plan.endpoint, "https://api.deepseek.com/v1/responses");
    }

    #[test]
    fn pi_official_google_generate() {
        let plan = resolve_probe_plan(
            "pi",
            "https://generativelanguage.googleapis.com",
            "AIzaSyTest",
            "gemini-2.0-flash",
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(plan.protocol, ProbeProtocol::GeminiGenerate);
        assert!(plan
            .endpoint
            .contains("/v1beta/models/gemini-2.0-flash:generateContent"));
        // key 走 header，不进 URL
        assert!(!plan.endpoint.contains("key="), "key 不得进 URL query");
        assert!(plan
            .headers
            .iter()
            .any(|(k, v)| k == "x-goog-api-key" && v == "AIzaSyTest"));
    }

    #[test]
    fn pi_custom_defaults_to_chat() {
        let plan = resolve_probe_plan(
            "pi",
            "https://proxy.example/v1",
            "sk",
            "foo",
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(plan.protocol, ProbeProtocol::ChatCompletions);
    }

    #[test]
    fn missing_model_errors() {
        let err =
            resolve_probe_plan("codex", "https://x", "sk", "  ", None, None, None).unwrap_err();
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

    // ── reachability (CC Switch stream_check semantics) ──

    #[test]
    fn reachability_default_config_matches_cc_switch() {
        let c = ReachabilityConfig::default();
        assert_eq!(c.timeout_secs, 8);
        assert_eq!(c.max_retries, 1);
        assert_eq!(c.degraded_threshold_ms, 6000);
    }

    #[test]
    fn reachability_should_retry_only_timeouts() {
        assert!(should_retry_reachability("Request timeout"));
        assert!(should_retry_reachability("request timed out"));
        assert!(should_retry_reachability("connection abort"));
        assert!(!should_retry_reachability("Connection failed: dns error"));
        assert!(!should_retry_reachability("Reachable"));
    }

    #[test]
    fn reachability_empty_url_fails() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let r = rt.block_on(probe_reachability("  ", &ReachabilityConfig::default()));
        assert!(!r.success);
        assert_eq!(r.status, ReachabilityStatus::Failed);
        assert!(r.message.contains("空") || r.message.to_lowercase().contains("empty"));
    }
}
