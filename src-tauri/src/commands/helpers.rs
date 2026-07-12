//! 扫描/导入共用小工具
use switch_api::models::TargetApp;

pub(crate) fn str_field(v: &serde_json::Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

pub(crate) fn codex_string_field(
    target: TargetApp,
    cfg: &serde_json::Value,
    key: &str,
) -> Option<String> {
    if target != TargetApp::Codex {
        return None;
    }

    let value = str_field(cfg, key);
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

pub(crate) fn codex_context_1m(target: TargetApp, cfg: &serde_json::Value) -> Option<bool> {
    if target != TargetApp::Codex {
        return None;
    }

    cfg.get("model_context_window")
        .and_then(|w| w.as_i64())
        .map(|w| w >= 1_000_000)
}

pub(crate) fn codex_bool_field(target: TargetApp, cfg: &serde_json::Value, key: &str) -> Option<bool> {
    if target != TargetApp::Codex {
        return None;
    }
    cfg.get(key).and_then(|v| v.as_bool())
}

/// 从 Claude Code 的 env 对象反向提取默认模型与角色映射，与 `ClaudeCodeAdapter::merge_config`
/// 的写入格式对称：
/// - `ANTHROPIC_MODEL` → 默认模型
/// - `ANTHROPIC_DEFAULT_{ROLE}_MODEL`（可能带 `[1M]` 后缀）→ mapping 的 `{role}_model` / `{role}_one_m`
/// - `ANTHROPIC_DEFAULT_{ROLE}_MODEL_NAME` → mapping 的 `{role}_name`
///
/// 用 `&mut Option` 累加：已有值不覆盖，仅补空（便于多个 env 来源依次补齐）。
pub(crate) fn claude_extract_models(
    env: &serde_json::Value,
    model: &mut Option<String>,
    mapping: &mut Option<std::collections::HashMap<String, String>>,
) {
    if model.is_none() {
        let m = str_field(env, "ANTHROPIC_MODEL");
        if !m.trim().is_empty() {
            *model = Some(m);
        }
    }

    let mut found = std::collections::HashMap::new();
    for role in ["sonnet", "opus", "fable", "haiku"] {
        let upper = role.to_uppercase();
        let raw = str_field(env, &format!("ANTHROPIC_DEFAULT_{upper}_MODEL"));
        if raw.trim().is_empty() {
            continue;
        }
        // [1M] 后缀 = 1M 上下文标记，剥离后才是真实模型 id
        let (base, one_m) = match raw.strip_suffix("[1M]") {
            Some(b) => (b.to_string(), true),
            None => (raw.clone(), false),
        };
        found.insert(format!("{role}_model"), base);
        if one_m {
            found.insert(format!("{role}_one_m"), "true".to_string());
        }
        let name = str_field(env, &format!("ANTHROPIC_DEFAULT_{upper}_MODEL_NAME"));
        if !name.trim().is_empty() {
            found.insert(format!("{role}_name"), name);
        }
    }

    if found.is_empty() {
        return;
    }
    match mapping {
        // 已有更高优先级的映射，只补尚未出现的键
        Some(existing) => {
            for (k, v) in found {
                existing.entry(k).or_insert(v);
            }
        }
        None => *mapping = Some(found),
    }
}

pub(crate) fn default_provider(target: TargetApp) -> String {
    match target {
        TargetApp::ClaudeCode => "anthropic",
        TargetApp::Codex => "openai",
        TargetApp::Gemini => "google",
        TargetApp::OpenCode => "anthropic",
        TargetApp::Hermes => "custom",
        TargetApp::OpenClaw => "custom",
    }
    .to_string()
}

