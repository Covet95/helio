//! 文档编辑层：把「配置文件」抽象成可保真编辑的文档。
//!
//! 本模块只做三件事，且都是纯函数（除显式标注的 IO 入口）：
//!
//! 1. **解析**：文本 → 文档树（保留注释、键序、空白）；
//! 2. **三路合并**：`(live, previous_managed, next_managed) → rendered`；
//! 3. **渲染**：文档树 → 文本。
//!
//! ## 为什么是「三路」而不是「声明 owned 路径」
//!
//! 朴素做法是给每个工具声明「这些路径归我管」，写回时只替换它们。但路径声明
//! 表达不了条件逻辑（Bedrock 分支、保留字后缀、auth 模式互斥），最后仍要退化成
//! 一堆规则。
//!
//! 改用三路合并后，**「我管哪些字段」由 `previous_managed` 自己界定**——
//! 上次写进去的东西就是我的管理范围，无需另写一份声明：
//!
//! - `live`：用户当前的真实文件（含手写注释、未受管字段）；
//! - `previous_managed`：上次切换时写入的受管片段（`None` = 首次切换）；
//! - `next_managed`：本次要写入的受管片段。
//!
//! 算法：从 `live` 里**摘掉** `previous_managed` 覆盖的叶子，再把 `next_managed`
//! 合并上去。于是：
//!
//! - 用户手写的字段（不在 previous_managed 里）→ 原样保留；
//! - 上次受管、本次不再需要的字段 → 被摘掉（不会残留）；
//! - 本次受管的字段 → 写入。
//!
//! 这条不变量可被属性测试钉死（见 `tests` 模块）。

use anyhow::{Context, Result};
use serde_json::Value;

pub mod json;
pub mod toml;
pub mod yaml;

/// 文档格式。新增工具时在此登记，并在 [`parse`] / [`render`] 中分派。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocFormat {
    /// JSON（`.json`）
    Json,
    /// TOML（`.toml`）
    Toml,
    /// YAML（`.yaml` / `.yml`）
    Yaml,
}

impl DocFormat {
    /// 按文件扩展名推断格式。未知扩展名返回 `None`，由调用方决定如何处理。
    pub fn from_path(path: &std::path::Path) -> Option<Self> {
        match path
            .extension()
            .and_then(|e| e.to_str())?
            .to_ascii_lowercase()
            .as_str()
        {
            "json" => Some(Self::Json),
            "toml" => Some(Self::Toml),
            "yaml" | "yml" => Some(Self::Yaml),
            _ => None,
        }
    }
}

/// 解析文本为文档树。
///
/// 空文本解析为 `Value::Null`，表示「文件不存在或为空」——合并时视作空文档，
/// 与「文件存在但内容为空对象」区分开（后者是 `Value::Object({})`）。
pub fn parse(format: DocFormat, text: &str) -> Result<Value> {
    if text.trim().is_empty() {
        return Ok(Value::Null);
    }
    match format {
        DocFormat::Json => json::parse(text),
        DocFormat::Toml => toml::parse(text),
        DocFormat::Yaml => yaml_parse(text),
    }
}

/// 渲染文档树为文本。`Value::Null` 渲染为空字符串。
pub fn render(format: DocFormat, value: &Value) -> Result<String> {
    if value.is_null() {
        return Ok(String::new());
    }
    match format {
        DocFormat::Json => json::render(value),
        DocFormat::Toml => toml::render(value),
        DocFormat::Yaml => yaml_render(value),
    }
}

fn yaml_parse(text: &str) -> Result<Value> {
    serde_yaml::from_str(text).context("Failed to parse YAML document")
}

fn yaml_render(value: &Value) -> Result<String> {
    serde_yaml::to_string(value).context("Failed to serialize YAML document")
}

/// 三路合并：以 `live` 为基底，摘掉 `previous_managed` 的覆盖，再叠加 `next_managed`。
///
/// 这是本模块的核心不变量所在。见模块文档。
pub fn merge_three_way(
    live: &Value,
    previous_managed: Option<&Value>,
    next_managed: &Value,
) -> Value {
    let mut base = normalize_document(live);
    if let Some(previous) = previous_managed {
        remove_covered(&mut base, &normalize_document(previous));
    }
    overlay(&mut base, &normalize_document(next_managed));
    base
}

/// 按格式做三路合并，返回渲染好的文本。
///
/// 各适配器的统一入口：TOML 走保格式路径（`toml_edit`），其余格式走
/// 值级合并后重新渲染。语义完全一致，差别只在格式保真能力。
pub fn merge_document(
    format: DocFormat,
    live_text: &str,
    previous_managed: Option<&Value>,
    next_managed: &Value,
) -> Result<String> {
    match format {
        DocFormat::Toml => toml::merge_json_into_toml(live_text, previous_managed, next_managed),
        DocFormat::Yaml => yaml::merge_documents(live_text, previous_managed, next_managed),
        DocFormat::Json => {
            let live = parse(DocFormat::Json, live_text)?;
            json::render(&merge_three_way(&live, previous_managed, next_managed))
        }
    }
}

/// 非对象文档归一为空对象——顶层必须是 map 才有「按路径摘除」的语义。
fn normalize_document(value: &Value) -> Value {
    if value.is_object() {
        value.clone()
    } else {
        Value::Object(serde_json::Map::new())
    }
}

/// 从 `base` 中摘除 `covered` 所覆盖的叶子路径。
///
/// 递归语义：
/// - `covered` 的子节点是对象 → 递归下探；子表被摘空后，该子表整体移除；
/// - `covered` 的子节点是标量/数组 → 该叶子在 `base` 中直接移除。
///
/// 关键点：**只摘「覆盖到」的路径**，`base` 中不在 `covered` 里的字段一律不动。
/// 这正是「用户手写内容被保留」的实现。
fn remove_covered(base: &mut Value, covered: &Value) {
    let (Some(base_map), Some(covered_map)) = (base.as_object_mut(), covered.as_object()) else {
        return;
    };

    let mut emptied = Vec::new();
    for (key, covered_value) in covered_map {
        let Some(base_value) = base_map.get_mut(key) else {
            continue;
        };
        if covered_value.is_object() && base_value.is_object() {
            remove_covered(base_value, covered_value);
            if base_value.as_object().is_some_and(|m| m.is_empty()) {
                emptied.push(key.clone());
            }
        } else {
            // 标量 / 数组 / 类型不匹配 → 整个叶子由受管片段拥有，摘除。
            emptied.push(key.clone());
        }
    }
    for key in emptied {
        base_map.remove(&key);
    }
}

/// 把 `patch` 合并进 `base`。对象递归合并，其余类型直接覆盖。
fn overlay(base: &mut Value, patch: &Value) {
    match (base, patch) {
        (Value::Object(base_map), Value::Object(patch_map)) => {
            for (key, patch_value) in patch_map {
                match base_map.get_mut(key) {
                    Some(base_value) => overlay(base_value, patch_value),
                    None => {
                        base_map.insert(key.clone(), patch_value.clone());
                    }
                }
            }
        }
        (base_value, patch_value) => {
            *base_value = patch_value.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn format_from_extension() {
        use std::path::Path;
        assert_eq!(DocFormat::from_path(Path::new("a.json")), Some(DocFormat::Json));
        assert_eq!(DocFormat::from_path(Path::new("a.toml")), Some(DocFormat::Toml));
        assert_eq!(DocFormat::from_path(Path::new("a.yaml")), Some(DocFormat::Yaml));
        assert_eq!(DocFormat::from_path(Path::new("a.YML")), Some(DocFormat::Yaml));
        assert_eq!(DocFormat::from_path(Path::new("a.txt")), None);
        assert_eq!(DocFormat::from_path(Path::new("noext")), None);
    }

    #[test]
    fn empty_text_parses_as_null() {
        assert_eq!(parse(DocFormat::Json, "").unwrap(), Value::Null);
        assert_eq!(parse(DocFormat::Json, "   \n ").unwrap(), Value::Null);
        assert_eq!(render(DocFormat::Json, &Value::Null).unwrap(), "");
    }

    // ---- 三路合并：本模块的核心契约 ----

    /// 用户手写的、不在受管片段里的字段必须原样保留。
    #[test]
    fn merge_preserves_untouched_user_fields() {
        let live = json!({
            "user_note": "keep me",
            "model": "old-model",
            "nested": { "user_key": 1, "model": "old" }
        });
        let previous = json!({ "model": "old-model", "nested": { "model": "old" } });
        let next = json!({ "model": "new-model", "nested": { "model": "new" } });

        let merged = merge_three_way(&live, Some(&previous), &next);

        assert_eq!(merged["user_note"], json!("keep me"));
        assert_eq!(merged["nested"]["user_key"], json!(1));
        assert_eq!(merged["model"], json!("new-model"));
        assert_eq!(merged["nested"]["model"], json!("new"));
    }

    /// 上次受管、本次不再写入的字段必须被摘掉，不能残留。
    #[test]
    fn merge_removes_stale_managed_fields() {
        let live = json!({ "managed_old": "stale", "user": "keep" });
        let previous = json!({ "managed_old": "stale" });
        let next = json!({ "managed_new": "fresh" });

        let merged = merge_three_way(&live, Some(&previous), &next);

        assert!(merged.get("managed_old").is_none(), "受管字段应被摘除");
        assert_eq!(merged["user"], json!("keep"));
        assert_eq!(merged["managed_new"], json!("fresh"));
    }

    /// 首次切换（无 previous）时，不应摘除 live 中的任何内容——只做叠加。
    #[test]
    fn merge_without_previous_only_overlays() {
        let live = json!({ "a": 1, "b": 2 });
        let next = json!({ "b": 3, "c": 4 });

        let merged = merge_three_way(&live, None, &next);

        assert_eq!(merged, json!({ "a": 1, "b": 3, "c": 4 }));
    }

    /// 摘除后变空的子表应整体移除，不留 `{}` 空壳。
    #[test]
    fn merge_drops_emptied_subtables() {
        let live = json!({ "providers": { "p1": { "url": "x" } }, "keep": true });
        let previous = json!({ "providers": { "p1": { "url": "x" } } });
        let next = json!({});

        let merged = merge_three_way(&live, Some(&previous), &next);

        assert!(
            merged.get("providers").is_none(),
            "摘空后的子表应整体消失，实际: {merged}"
        );
        assert_eq!(merged["keep"], json!(true));
    }

    /// 用户手动改过受管字段的值时，受管片段仍应覆盖它（受管字段归 Helio 所有）。
    #[test]
    fn merge_overrides_user_edit_of_managed_field() {
        let live = json!({ "model": "user-typed-this" });
        let previous = json!({ "model": "what-we-wrote" });
        let next = json!({ "model": "new-from-profile" });

        let merged = merge_three_way(&live, Some(&previous), &next);

        assert_eq!(merged["model"], json!("new-from-profile"));
    }

    /// 幂等：用同一份 next 连续合并两次，结果不变。
    #[test]
    fn merge_is_idempotent() {
        let live = json!({ "user": 1, "model": "a" });
        let previous = json!({ "model": "a" });
        let next = json!({ "model": "b", "extra": true });

        let once = merge_three_way(&live, Some(&previous), &next);
        let twice = merge_three_way(&once, Some(&next), &next);

        assert_eq!(once, twice);
    }

    /// 非对象文档（如顶层是数组）归一为空对象，不 panic。
    #[test]
    fn merge_normalizes_non_object_documents() {
        let merged = merge_three_way(&json!([1, 2, 3]), None, &json!({ "a": 1 }));
        assert_eq!(merged, json!({ "a": 1 }));
    }

    /// 类型不匹配（live 是标量、受管是对象）时，受管片段获胜。
    #[test]
    fn merge_handles_type_mismatch() {
        let live = json!({ "section": "a string" });
        let previous = json!({ "section": { "nested": 1 } });
        let next = json!({ "section": { "nested": 2 } });

        let merged = merge_three_way(&live, Some(&previous), &next);

        assert_eq!(merged["section"]["nested"], json!(2));
    }
}
