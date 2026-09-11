//! Tauri 包装：模型列表 + 探活（核心逻辑在 switch_api::probe）

pub use switch_api::probe::{
    probe_reachability, probe_with_params, FailoverResult, KeyProbeResult, ModelTestResult,
    ReachabilityConfig,
};

use serde::{Deserialize, Serialize};
use switch_api::error::AppError;
use switch_api::probe;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchedModel {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owned_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct ModelsResponse {
    data: Option<Vec<ModelEntry>>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
    #[serde(default)]
    owned_by: Option<String>,
}

#[derive(Deserialize)]
struct GeminiModelsResponse {
    models: Option<Vec<GeminiModelEntry>>,
}

#[derive(Deserialize)]
struct GeminiModelEntry {
    name: String,
    #[serde(default, rename = "displayName")]
    display_name: Option<String>,
    #[serde(default, rename = "inputTokenLimit")]
    input_token_limit: Option<i64>,
    #[serde(default, rename = "supportedGenerationMethods")]
    supported_generation_methods: Option<Vec<String>>,
}

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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestModelRequest {
    pub target_app: String,
    pub api_url: String,
    pub api_key: String,
    pub model: String,
    pub env_key: Option<String>,
    pub wire_api: Option<String>,
    pub api_mode: Option<String>,
    pub experimental_bearer_token: Option<String>,
    pub key_label: Option<String>,
    /// Codex auth 命令模式：Helio 不执行外部命令，探活/拉取时直接提示。
    #[serde(default)]
    pub has_command_auth: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchModelsRequest {
    pub target_app: String,
    pub provider: Option<String>,
    pub api_url: String,
    pub api_key: String,
    pub env_key: Option<String>,
    pub api_mode: Option<String>,
    pub experimental_bearer_token: Option<String>,
    pub aws_profile: Option<String>,
    pub aws_region: Option<String>,
    /// Codex auth 命令模式：Helio 不执行外部命令，直接提示。
    #[serde(default)]
    pub has_command_auth: bool,
}

fn provider_models_url(api_url: &str) -> Option<String> {
    let lower = api_url.to_lowercase();
    PROVIDER_MODELS_URLS
        .iter()
        .find(|(pat, _)| lower.contains(pat))
        .map(|(_, url)| url.to_string())
}

fn candidates(base_url: &str) -> Vec<String> {
    let base = base_url.trim_end_matches('/');
    let mut out = vec![format!("{}/v1/models", base), format!("{}/models", base)];
    for suf in COMPAT_SUFFIXES {
        if let Some(stripped) = base.strip_suffix(suf) {
            let s = stripped.trim_end_matches('/');
            out.push(format!("{}/v1/models", s));
            out.push(format!("{}/models", s));
        }
    }
    out
}

fn is_local_url(api_url: &str) -> bool {
    let lower = api_url.to_lowercase();
    lower.contains("127.0.0.1") || lower.contains("localhost") || lower.contains("0.0.0.0")
}

fn http_client(api_url: &str) -> Result<reqwest::Client, AppError> {
    let mut builder = reqwest::Client::builder().timeout(std::time::Duration::from_secs(15));
    if is_local_url(api_url) {
        builder = builder.no_proxy();
    }
    builder
        .build()
        .map_err(|e| AppError::from(e).with_context("创建 HTTP 客户端失败"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiscoveryProtocol {
    OpenAiCompatible,
    Anthropic,
    Gemini,
}

fn discovery_protocol(request: &FetchModelsRequest) -> DiscoveryProtocol {
    let app = request.target_app.trim().to_ascii_lowercase();
    let provider = request
        .provider
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let mode = request
        .api_mode
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    // Codex 只剩 Responses 一个协议（chat 已于 2026-02 删除），wire 取值不再影响发现方式。
    if app == "codex" {
        return DiscoveryProtocol::OpenAiCompatible;
    }
    if request
        .api_url
        .to_ascii_lowercase()
        .contains("generativelanguage.googleapis.com")
        || matches!(provider.as_str(), "google" | "gemini" | "gemini-api")
        || (app == "pi" && mode.contains("gemini"))
    {
        DiscoveryProtocol::Gemini
    } else if matches!(app.as_str(), "claude-code" | "zcode")
        || matches!(provider.as_str(), "anthropic" | "claude")
        || mode.contains("anthropic")
        || mode == "anthropic-messages"
    {
        DiscoveryProtocol::Anthropic
    } else {
        DiscoveryProtocol::OpenAiCompatible
    }
}

fn is_bedrock_request(request: &FetchModelsRequest) -> bool {
    request.target_app.eq_ignore_ascii_case("codex")
        && request
            .provider
            .as_deref()
            .map(str::trim)
            .is_some_and(|provider| provider.eq_ignore_ascii_case("amazon-bedrock"))
}

fn choose_non_empty_env_value(env_value: Option<String>, fallback: String) -> String {
    env_value
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(fallback)
}

fn bedrock_auth_context(request: &FetchModelsRequest) -> String {
    let mut fields = Vec::new();
    if let Some(profile) = request
        .aws_profile
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        fields.push(format!("AWS Profile: {profile}"));
    }
    if let Some(region) = request
        .aws_region
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        fields.push(format!("AWS Region: {region}"));
    }
    if fields.is_empty() {
        String::new()
    } else {
        format!("（{}）", fields.join("，"))
    }
}

fn resolved_api_key(request: &FetchModelsRequest) -> Result<String, AppError> {
    let env_key = request
        .env_key
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .and_then(|name| std::env::var(name).ok())
        .unwrap_or_default();
    let key = if request.target_app.eq_ignore_ascii_case("codex") {
        request
            .experimental_bearer_token
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| {
                if env_key.trim().is_empty() {
                    request.api_key.trim()
                } else {
                    env_key.trim()
                }
            })
    } else if env_key.trim().is_empty() {
        request.api_key.trim()
    } else {
        env_key.trim()
    };
    if key.is_empty() {
        if request.has_command_auth {
            Err(AppError::invalid_input(
                "该档案使用 auth 命令获取 token，Helio 不执行外部命令，无法加载模型列表",
            ))
        } else {
            Err(AppError::invalid_input(
                "需要 API Key、Bearer Token 或环境变量才能加载模型",
            ))
        }
    } else {
        Ok(key.to_string())
    }
}

fn discovery_headers(protocol: DiscoveryProtocol, api_key: &str) -> Vec<(&'static str, String)> {
    match protocol {
        DiscoveryProtocol::OpenAiCompatible => vec![("Authorization", format!("Bearer {api_key}"))],
        DiscoveryProtocol::Anthropic => vec![
            ("x-api-key", api_key.to_string()),
            ("anthropic-version", "2023-06-01".to_string()),
        ],
        DiscoveryProtocol::Gemini => vec![("x-goog-api-key", api_key.to_string())],
    }
}

fn discovery_urls(api_url: &str, protocol: DiscoveryProtocol) -> Vec<String> {
    if protocol == DiscoveryProtocol::Gemini {
        let base = api_url.trim().trim_end_matches('/');
        if base.contains("/models/") {
            return vec![base.to_string()];
        }
        return vec![if base.ends_with("/v1beta") || base.ends_with("/v1") {
            format!("{base}/models")
        } else {
            format!("{base}/v1beta/models")
        }];
    }

    let mut urls = Vec::new();
    if let Some(u) = provider_models_url(api_url) {
        urls.push(u);
    }
    urls.extend(candidates(api_url));
    urls
}

fn parse_models_response(
    body: &str,
    protocol: DiscoveryProtocol,
) -> Result<Vec<FetchedModel>, String> {
    match protocol {
        DiscoveryProtocol::Gemini => {
            let parsed: GeminiModelsResponse =
                serde_json::from_str(body).map_err(|error| error.to_string())?;
            Ok(parsed
                .models
                .unwrap_or_default()
                .into_iter()
                .map(|model| FetchedModel {
                    id: model
                        .name
                        .strip_prefix("models/")
                        .unwrap_or(&model.name)
                        .to_string(),
                    owned_by: Some("google".to_string()),
                    display_name: model.display_name,
                    context_window: model.input_token_limit,
                    capabilities: model.supported_generation_methods,
                })
                .collect())
        }
        DiscoveryProtocol::OpenAiCompatible | DiscoveryProtocol::Anthropic => {
            let parsed: ModelsResponse =
                serde_json::from_str(body).map_err(|error| error.to_string())?;
            Ok(parsed
                .data
                .unwrap_or_default()
                .into_iter()
                .map(|model| FetchedModel {
                    id: model.id,
                    owned_by: model.owned_by,
                    display_name: None,
                    context_window: None,
                    capabilities: None,
                })
                .collect())
        }
    }
}

/// 拉取供应商可用模型列表
#[tauri::command]
pub async fn fetch_models(request: FetchModelsRequest) -> Result<Vec<FetchedModel>, AppError> {
    if is_bedrock_request(&request) {
        return Err(AppError::invalid_input(format!(
            "Amazon Bedrock 使用 Codex 内置 AWS 认证，模型列表由 Codex 管理{}",
            bedrock_auth_context(&request)
        )));
    }
    if request.api_url.trim().is_empty() {
        return Err(AppError::invalid_input("需要 API URL 才能加载模型"));
    }
    let protocol = discovery_protocol(&request);
    let api_key = resolved_api_key(&request)?;
    let urls = discovery_urls(&request.api_url, protocol);
    let client = http_client(&request.api_url)?;
    let mut last_err = String::from("无候选端点");
    for url in &urls {
        let mut builder = client.get(url);
        for (name, value) in discovery_headers(protocol, &api_key) {
            builder = builder.header(name, value);
        }
        let res = builder.send().await;
        match res {
            Ok(r) if r.status().is_success() => match r.text().await {
                Ok(body) => match parse_models_response(&body, protocol) {
                    Ok(mut models) => {
                        let mut seen = std::collections::HashSet::new();
                        models.retain(|m| seen.insert(m.id.clone()));
                        models.sort_by(|a, b| a.id.cmp(&b.id));
                        return Ok(models);
                    }
                    Err(error) => {
                        last_err = format!("{} 解析失败：{}", probe::sanitize_endpoint(url), error)
                    }
                },
                Err(error) => {
                    last_err = format!("{} 读取响应失败：{}", probe::sanitize_endpoint(url), error)
                }
            },
            Ok(r) => last_err = format!("{} 返回 {}", probe::sanitize_endpoint(url), r.status()),
            Err(e) => last_err = format!("{} 请求失败：{}", probe::sanitize_endpoint(url), e),
        }
    }
    // message 只说「失败了、试了几个端点」，逐个端点的具体原因进 detail：
    // 前者是用户需要知道的，后者是排查时需要看的，混在一行里两头都读不清。
    Err(AppError::io(format!("加载模型失败（试了 {} 个端点）", urls.len())).with_detail(last_err))
}

/// 按目标工具协议探活
#[tauri::command]
pub async fn test_model(request: TestModelRequest) -> Result<ModelTestResult, AppError> {
    let resolved_api_key = choose_non_empty_env_value(
        request
            .env_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .and_then(|name| std::env::var(name).ok()),
        request.api_key,
    );
    if resolved_api_key.trim().is_empty() && request.has_command_auth {
        return Err(AppError::invalid_input(
            "该档案使用 auth 命令获取 token，Helio 不执行外部命令，无法探活",
        ));
    }
    probe::probe_with_params(probe::ProbeRequest {
        target_app: &request.target_app,
        api_url: &request.api_url,
        api_key: &resolved_api_key,
        model: &request.model,
        wire_api: request.wire_api.as_deref(),
        api_mode: request.api_mode.as_deref(),
        experimental_bearer_token: request.experimental_bearer_token.as_deref(),
        key_label: request.key_label,
    })
    .await
    // 探活失败的具体原因（超时 / 401 / 模型不存在）由 probe 层给出，已经是中文，
    // 这里只负责归类。归 Io：探活本质是一次网络 I/O，失败即端点不可用。
    .map_err(AppError::io)
}

#[cfg(test)]
mod tests {
    use super::{
        discovery_headers, discovery_protocol, is_bedrock_request, parse_models_response,
        DiscoveryProtocol, FetchModelsRequest, TestModelRequest,
    };

    #[test]
    fn test_model_request_deserializes_camel_case() {
        let request: TestModelRequest = serde_json::from_value(serde_json::json!({
            "targetApp": "codex",
            "apiUrl": "https://api.example.test",
            "apiKey": "key",
            "model": "gpt-test",
            "envKey": "CODEX_API_KEY",
            "wireApi": "responses",
            "apiMode": "openai",
            "experimentalBearerToken": "token",
            "keyLabel": "Primary",
        }))
        .unwrap();

        assert_eq!(request.target_app, "codex");
        assert_eq!(request.env_key.as_deref(), Some("CODEX_API_KEY"));
        assert_eq!(request.key_label.as_deref(), Some("Primary"));
    }

    #[test]
    fn discovery_uses_protocol_specific_auth_and_metadata() {
        let request: FetchModelsRequest = serde_json::from_value(serde_json::json!({
            "targetApp": "claude-code",
            "provider": "anthropic",
            "apiUrl": "https://api.example.test",
            "apiKey": "key",
        }))
        .unwrap();
        assert_eq!(discovery_protocol(&request), DiscoveryProtocol::Anthropic);
        assert_eq!(
            discovery_headers(DiscoveryProtocol::Anthropic, "key")[0].0,
            "x-api-key"
        );

        let gemini = parse_models_response(
            r#"{"models":[{"name":"models/gemini-2.5-pro","displayName":"Gemini Pro","inputTokenLimit":1048576,"supportedGenerationMethods":["generateContent"]}]}"#,
            DiscoveryProtocol::Gemini,
        )
        .unwrap();
        assert_eq!(gemini[0].id, "gemini-2.5-pro");
        assert_eq!(gemini[0].display_name.as_deref(), Some("Gemini Pro"));
        assert_eq!(gemini[0].context_window, Some(1_048_576));
    }

    #[test]
    fn bedrock_detection_requires_explicit_provider() {
        let inferred: FetchModelsRequest = serde_json::from_value(serde_json::json!({
            "targetApp": "codex",
            "apiUrl": "",
            "apiKey": "",
        }))
        .unwrap();
        assert!(!is_bedrock_request(&inferred));

        let explicit: FetchModelsRequest = serde_json::from_value(serde_json::json!({
            "targetApp": "codex",
            "provider": "amazon-bedrock",
            "apiUrl": "",
            "apiKey": "",
            "awsProfile": "default",
            "awsRegion": "us-east-1",
        }))
        .unwrap();
        assert!(is_bedrock_request(&explicit));
        assert_eq!(
            discovery_protocol(&explicit),
            DiscoveryProtocol::OpenAiCompatible
        );
    }

    #[test]
    fn query_key_is_masked_in_error_detail() {
        // 回归：fetch_models 曾把原始 ?key= 明文写进 AppError detail（前端“详情”展示）。
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(4) {
                let mut s = stream.unwrap();
                let mut buf = [0u8; 4096];
                use std::io::Read;
                s.set_read_timeout(Some(std::time::Duration::from_secs(3)))
                    .ok();
                let _ = s.read(&mut buf);
                let body = "boom";
                use std::io::Write;
                let resp = format!(
                    "HTTP/1.1 500 Boom\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = s.write_all(resp.as_bytes());
            }
        });
        let req: super::FetchModelsRequest = serde_json::from_value(serde_json::json!({
            "targetApp": "opencode",
            "apiUrl": format!("http://{addr}/v1?key=SECRET999"),
            "apiKey": "k",
        }))
        .unwrap();
        let err = tauri::async_runtime::block_on(super::fetch_models(req)).unwrap_err();
        let v = serde_json::to_value(&err).unwrap();
        let detail = v.get("detail").and_then(|d| d.as_str()).unwrap_or("");
        assert!(
            !detail.contains("SECRET999"),
            "detail 不得含明文凭据，实得：{detail}"
        );
        assert!(
            detail.contains("***"),
            "detail 应保留掩码后的端点以便排查，实得：{detail}"
        );
    }

    #[test]
    fn empty_environment_value_does_not_replace_explicit_key() {
        assert_eq!(
            super::choose_non_empty_env_value(Some("   ".into()), "explicit".into()),
            "explicit"
        );
        assert_eq!(
            super::choose_non_empty_env_value(Some("from-env".into()), "explicit".into()),
            "from-env"
        );
    }
}
