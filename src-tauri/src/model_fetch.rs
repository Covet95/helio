//! Tauri 包装：模型列表 + 探活（核心逻辑在 switch_api::probe）

pub use switch_api::probe::{
    probe_reachability, probe_with_params, FailoverResult, KeyProbeResult, ModelTestResult,
    ReachabilityConfig, ReachabilityResult, ReachabilityStatus,
};

use serde::{Deserialize, Serialize};
use switch_api::probe;

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

/// 拉取供应商可用模型列表
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

/// 按目标工具协议探活
#[tauri::command]
pub async fn test_model(
    target_app: String,
    api_url: String,
    api_key: String,
    model: String,
    wire_api: Option<String>,
    api_mode: Option<String>,
    experimental_bearer_token: Option<String>,
    key_label: Option<String>,
) -> Result<ModelTestResult, String> {
    probe::probe_with_params(
        &target_app,
        &api_url,
        &api_key,
        &model,
        wire_api.as_deref(),
        api_mode.as_deref(),
        experimental_bearer_token.as_deref(),
        key_label,
    )
    .await
}

#[cfg(test)]
mod tests {
    // resolve matrix lives in switch_api::probe tests
}
