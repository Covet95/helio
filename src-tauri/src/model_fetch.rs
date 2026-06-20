//! 模型列表加载与模型可用性测试（参考 cc-switch）
//!
//! 用 OpenAI 兼容的 GET /v1/models 端点拉供应商可用模型；对 Anthropic 协议
//! 挂在兼容子路径上的供应商（如 .../api/anthropic），会剥掉已知后缀再试。
//! 测试模型时发送一次极小的 OpenAI-compatible chat completion。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchedModel {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owned_by: Option<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelTestResult {
    pub model: String,
    pub endpoint: String,
}

#[derive(Deserialize)]
struct ChatCompletionResponse {
    #[serde(default)]
    choices: Vec<serde_json::Value>,
}

/// OpenAI Responses API 的成功响应（探活只看是否含 output/status 字段）。
#[derive(Deserialize)]
struct ResponsesResponse {
    #[serde(default)]
    output: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    status: Option<String>,
}

/// Anthropic 协议兼容子路径；命中则追加「剥后缀再拼 /v1/models」的候选
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

/// 已知 provider 的 models 端点（按 api_url 子串匹配；参考 cc-switch 的 provider 预设）。
/// 命中则优先用这个端点（这些 provider 的 Anthropic 路径下没有 /v1/models，得用它们的 OpenAI 兼容端点）。
const PROVIDER_MODELS_URLS: &[(&str, &str)] = &[
    // 智谱 GLM：Anthropic 在 /api/anthropic，OpenAI 模型列表在 /api/paas/v4/models
    ("bigmodel.cn", "https://open.bigmodel.cn/api/paas/v4/models"),
    ("z.ai", "https://api.z.ai/api/paas/v4/models"),
    // DeepSeek
    ("deepseek.com", "https://api.deepseek.com/models"),
    // Kimi / Moonshot
    ("moonshot.cn", "https://api.moonshot.cn/v1/models"),
    // OpenRouter
    ("openrouter.ai", "https://openrouter.ai/api/v1/models"),
    // 硅基流动
    ("siliconflow.cn", "https://api.siliconflow.cn/v1/models"),
    // 阿里百炼
    ("dashscope.aliyuncs.com", "https://dashscope.aliyuncs.com/compatible-mode/v1/models"),
];

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

fn http_client(api_url: &str) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder().timeout(std::time::Duration::from_secs(15));
    if is_local_url(api_url) {
        builder = builder.no_proxy();
    }
    builder.build().map_err(|e| e.to_string())
}

fn openai_base_url(api_url: &str) -> String {
    if let Some(url) = provider_models_url(api_url) {
        return url.trim_end_matches("/models").trim_end_matches('/').to_string();
    }

    let base = api_url.trim_end_matches('/');
    for suffix in COMPAT_SUFFIXES {
        if let Some(stripped) = base.strip_suffix(suffix) {
            return stripped.trim_end_matches('/').to_string();
        }
    }
    base.to_string()
}

fn chat_completions_url(api_url: &str) -> String {
    let base = openai_base_url(api_url);
    if base.ends_with("/v1") || base.ends_with("/paas/v4") {
        format!("{}/chat/completions", base)
    } else {
        format!("{}/v1/chat/completions", base)
    }
}

/// Responses API 端点（wire_api = "responses" 时使用）。
fn responses_url(api_url: &str) -> String {
    let base = openai_base_url(api_url);
    if base.ends_with("/v1") || base.ends_with("/paas/v4") {
        format!("{}/responses", base)
    } else {
        format!("{}/v1/responses", base)
    }
}

/// 拉取供应商可用模型列表：按候选 URL 顺序试，第一个成功（{data:[{id}]}}）的返回。
#[tauri::command]
pub async fn fetch_models(api_url: String, api_key: String) -> Result<Vec<FetchedModel>, String> {
    if api_key.trim().is_empty() {
        return Err("需要 API Key 才能加载模型".to_string());
    }
    let mut urls: Vec<String> = Vec::new();
    if let Some(u) = provider_models_url(&api_url) {
        urls.push(u);
    }
    urls.extend(candidates(&api_url));
    // 用户常开 macOS 系统代理（HTTP/SOCKS 指向 127.0.0.1）。reqwest 默认读系统
    // 代理，会把本地服务（如 127.0.0.1:8317 的代理网关）也转发到系统代理导致
    // 连接失败。这里对本地地址绕过代理，其余仍走系统代理（境外 provider 可能需要）。
    let client = http_client(&api_url)?;
    let mut last_err = String::from("无候选端点");
    for url in &urls {
        let res = client
            .get(url)
            .header("Authorization", format!("Bearer {}", api_key))
            .send()
            .await;
        match res {
            Ok(r) if r.status().is_success() => match r.json::<ModelsResponse>().await {
                Ok(parsed) => {
                    let mut models: Vec<FetchedModel> = parsed
                        .data
                        .unwrap_or_default()
                        .into_iter()
                        .map(|m| FetchedModel {
                            id: m.id,
                            owned_by: m.owned_by,
                        })
                        .collect();
                    // 按 id 去重（保留首次出现）+ 字母序排序
                    let mut seen = std::collections::HashSet::new();
                    models.retain(|m| seen.insert(m.id.clone()));
                    models.sort_by(|a, b| a.id.cmp(&b.id));
                    return Ok(models);
                }
                Err(e) => last_err = format!("{} 解析失败: {}", url, e),
            },
            Ok(r) => last_err = format!("{} 返回 {}", url, r.status()),
            Err(e) => last_err = format!("{} 请求失败: {}", url, e),
        }
    }
    Err(format!(
        "加载模型失败（试了 {} 个端点）: {}",
        urls.len(),
        last_err
    ))
}

/// 用指定模型发起一次极小的探活请求，请求成功才表示模型可用。
/// wire_api 决定端点与请求/响应格式：
/// - "responses" → POST /v1/responses（body 用 input/max_output_tokens，响应看 output/status）
/// - 其他（含 None/"chat"/"") → POST /v1/chat/completions（body 用 messages，响应看 choices）
#[tauri::command]
pub async fn test_model(
    api_url: String,
    api_key: String,
    model: String,
    wire_api: Option<String>,
) -> Result<ModelTestResult, String> {
    if api_key.trim().is_empty() {
        return Err("需要 API Key 才能测试模型".to_string());
    }
    let model = model.trim();
    if model.is_empty() {
        return Err("先选择或填写模型".to_string());
    }

    let use_responses = wire_api.as_deref() == Some("responses");
    let client = http_client(&api_url)?;

    let (endpoint, body) = if use_responses {
        (
            responses_url(&api_url),
            serde_json::json!({
                "model": model,
                "input": "ping",
                "max_output_tokens": 16,
                "stream": false
            }),
        )
    } else {
        (
            chat_completions_url(&api_url),
            serde_json::json!({
                "model": model,
                "messages": [{"role": "user", "content": "ping"}],
                "max_tokens": 1,
                "temperature": 0,
                "stream": false
            }),
        )
    };

    let response = client
        .post(&endpoint)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("{} 请求失败: {}", endpoint, e))?;

    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|e| format!("{} 读取响应失败: {}", endpoint, e))?;
    if !status.is_success() {
        let detail = text.trim();
        if detail.is_empty() {
            return Err(format!("{} 返回 {}", endpoint, status));
        }
        return Err(format!("{} 返回 {}: {}", endpoint, status, detail));
    }

    // 探活只需确认 HTTP 2xx 且返回体能解析为对应协议的成功结构。
    if use_responses {
        let parsed: ResponsesResponse = serde_json::from_str(&text)
            .map_err(|e| format!("{} 解析失败: {}", endpoint, e))?;
        // responses 返回应含 output 数组或 status 字段；两者皆空视为异常。
        if parsed.output.is_none() && parsed.status.is_none() {
            return Err(format!("{} 未返回有效 responses 结构", endpoint));
        }
    } else {
        let parsed: ChatCompletionResponse = serde_json::from_str(&text)
            .map_err(|e| format!("{} 解析失败: {}", endpoint, e))?;
        if parsed.choices.is_empty() {
            return Err(format!("{} 没有返回 completion choice", endpoint));
        }
    }

    Ok(ModelTestResult {
        model: model.to_string(),
        endpoint,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_completions_url_keeps_openai_v1() {
        assert_eq!(
            chat_completions_url("https://api.openai.com/v1"),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn test_responses_url_keeps_v1() {
        assert_eq!(
            responses_url("https://api.openai.com/v1"),
            "https://api.openai.com/v1/responses"
        );
    }

    #[test]
    fn test_responses_url_adds_v1_for_plain_base() {
        assert_eq!(
            responses_url("https://chybenzun.top"),
            "https://chybenzun.top/v1/responses"
        );
    }

    #[test]
    fn test_chat_completions_url_adds_v1_for_plain_base() {
        assert_eq!(
            chat_completions_url("https://example.com"),
            "https://example.com/v1/chat/completions"
        );
    }

    #[test]
    fn test_chat_completions_url_strips_anthropic_compat_suffix() {
        assert_eq!(
            chat_completions_url("https://api.deepseek.com/anthropic"),
            "https://api.deepseek.com/v1/chat/completions"
        );
    }

    #[test]
    fn test_chat_completions_url_uses_provider_override() {
        assert_eq!(
            chat_completions_url("https://open.bigmodel.cn/api/anthropic"),
            "https://open.bigmodel.cn/api/paas/v4/chat/completions"
        );
    }
}
