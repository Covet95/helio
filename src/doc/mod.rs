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
/// ## 安全网：先算「正确结果」，再验「保真结果」
///
/// 保真合并依赖第三方库（`toml_edit` / `yaml_edit`）在保留格式的前提下做
/// 结构修改，而这类库的边角行为很难穷举验证——实测已踩到多个：嵌套值被
/// 提到顶层、序列缩进错乱、新键写入被静默丢弃。
///
/// 因此本函数采取 **verify-then-commit**：
///
/// 1. 先用纯值级合并算出 `expected`（正确性基准，不依赖任何库的格式能力）；
/// 2. 再跑保真合并得到 `candidate`（可能保住注释）；
/// 3. 把 `candidate` 解析回来与 `expected` 比对——**只有语义完全一致才采用**；
/// 4. 解析失败或语义不符 → 退回值级结果重新渲染。
///
/// 这样最坏情况退化为「旧的、正确但丢注释」的行为，**永远不会**产出损坏
/// 文件或写错语义。保真是优化，正确性是底线。
pub fn merge_document(
    format: DocFormat,
    live_text: &str,
    previous_managed: Option<&Value>,
    next_managed: &Value,
) -> Result<String> {
    let live = parse(format, live_text)?;
    let expected = merge_three_way(&live, previous_managed, next_managed);

    // JSON 无注释，值级合并后直接渲染即是最终形态，无需保真路径。
    if format == DocFormat::Json {
        return json::render(&expected);
    }

    let candidate = match format {
        DocFormat::Toml => toml::merge_json_into_toml(live_text, previous_managed, next_managed),
        DocFormat::Yaml => yaml::merge_documents(live_text, previous_managed, next_managed),
        DocFormat::Json => unreachable!("JSON 已在上方提前返回"),
    };

    match candidate {
        Ok(text) if renders_to(&text, format, &expected) => Ok(text),
        Ok(_) => {
            tracing::warn!("保真合并结果与预期语义不符，退回整体重写（{format:?}）");
            render(format, &expected)
        }
        Err(error) => {
            tracing::warn!("保真合并失败，退回整体重写（{format:?}）：{error:#}");
            render(format, &expected)
        }
    }
}

/// 校验候选文本解析后是否与期望值语义一致。
///
/// 用「解析 + 比对」而非字符串比较：保真路径会改变空白与格式，语义才是
/// 判定依据。解析失败即视为不一致。
fn renders_to(text: &str, format: DocFormat, expected: &Value) -> bool {
    match parse(format, text) {
        Ok(actual) => actual == *expected,
        Err(_) => false,
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
        // 用 `shift_remove` 而非 `remove`：启用 `preserve_order` 后，
        // `serde_json::Map` 的 `remove` 实际是 `swap_remove`——它把**最后一个
        // 元素搬到被删位置**，导致剩余键序被打乱。本模块的承诺是保留键序，
        // 因此必须用保序的 `shift_remove`（代价是 O(n)，但配置文件规模很小）。
        base_map.shift_remove(&key);
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
        assert_eq!(
            DocFormat::from_path(Path::new("a.json")),
            Some(DocFormat::Json)
        );
        assert_eq!(
            DocFormat::from_path(Path::new("a.toml")),
            Some(DocFormat::Toml)
        );
        assert_eq!(
            DocFormat::from_path(Path::new("a.yaml")),
            Some(DocFormat::Yaml)
        );
        assert_eq!(
            DocFormat::from_path(Path::new("a.YML")),
            Some(DocFormat::Yaml)
        );
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

#[cfg(test)]
mod safety_net_tests {
    use super::*;
    use serde_json::json;

    /// 跑一次合并，断言结果**解析回来**与值级基准一致。
    ///
    /// 这是本模块最重要的契约：保真路径可以丢注释（格式优化），但**绝不能**
    /// 改语义或产出无法解析的文件。下面每个用例都对应一个实测踩到过的坑。
    fn assert_semantics(format: DocFormat, live: &str, previous: Option<&Value>, next: &Value) {
        let live_value = parse(format, live).expect("live 应可解析");
        let expected = merge_three_way(&live_value, previous, next);

        let out = merge_document(format, live, previous, next)
            .unwrap_or_else(|e| panic!("{format:?} 合并失败：{e:#}"));

        let actual = parse(format, &out)
            .unwrap_or_else(|e| panic!("{format:?} 产出无法解析：{e}\n---\n{out}"));

        assert_eq!(actual, expected, "{format:?} 语义不符\n--- 输出 ---\n{out}");
    }

    /// 回归：YAML 新建嵌套映射时，子键曾被**提到顶层**（`model:` 变 null），
    /// 且切换报告成功——静默损坏用户配置。
    #[test]
    fn yaml_new_nested_mapping_stays_nested() {
        assert_semantics(
            DocFormat::Yaml,
            "user_setting: keep\n",
            None,
            &json!({ "model": { "default": "m", "provider": "custom:x" } }),
        );
    }

    /// 回归：YAML 序列长度变化时曾产出**非法 YAML**（第 2 项起顶格）。
    #[test]
    fn yaml_sequence_length_change_is_valid() {
        assert_semantics(
            DocFormat::Yaml,
            "list:\n  - a\n  - b\n",
            None,
            &json!({ "list": [{ "n": 1 }] }),
        );
        assert_semantics(
            DocFormat::Yaml,
            "list:\n  - a\n",
            None,
            &json!({ "list": [{ "n": 1 }, { "n": 2 }] }),
        );
    }

    /// 回归：YAML 序列元素是数组时曾被**字符串化**（`[1,2,3]` → `'[1,2,3]'`）。
    #[test]
    fn yaml_nested_array_is_not_stringified() {
        assert_semantics(
            DocFormat::Yaml,
            "l:\n- a\n- b\n",
            None,
            &json!({ "l": [[1, 2, 3], "b"] }),
        );
    }

    /// 回归：YAML 曾**从不摘除**陈旧受管键（TOML 会摘），导致配置无限累积。
    #[test]
    fn yaml_removes_stale_managed_keys() {
        assert_semantics(
            DocFormat::Yaml,
            "a: 1\nkeep: 1\n",
            Some(&json!({ "a": 1 })),
            &json!({}),
        );
    }

    /// 回归：TOML 遇到用户的 `[[array-of-tables]]` 曾**直接报错**，
    /// 使原本可切换的配置无法切换（旧实现能处理）。
    #[test]
    fn toml_array_of_tables_does_not_abort() {
        assert_semantics(
            DocFormat::Toml,
            "[m]\nn = \"x\"\n\n[[pl]]\nname = \"p1\"\n",
            None,
            &json!({ "m": { "n": "y" }, "pl": [{ "name": "p1" }] }),
        );
    }

    /// 回归：TOML 顶层赋表值时曾**连带删除兄弟子表**。
    #[test]
    fn toml_table_update_keeps_sibling_subtables() {
        assert_semantics(
            DocFormat::Toml,
            "[mcp.fs]\ncommand = \"npx\"\n\n[mcp.other]\ncommand = \"uvx\"\n",
            None,
            &json!({ "mcp": { "fs": { "command": "npx2" }, "other": { "command": "uvx" } } }),
        );
    }

    /// 回归：TOML 的嵌套摘除曾误删用户手写子表。
    #[test]
    fn toml_nested_removal_keeps_user_subtables() {
        assert_semantics(
            DocFormat::Toml,
            "[mp.custom]\nbase_url = \"https://old\"\n\n[mp.myown]\nbase_url = \"https://mine\"\n",
            Some(&json!({ "mp": { "custom": { "base_url": "https://old" } } })),
            &json!({ "model": "gpt-5" }),
        );
    }
}

#[cfg(test)]
mod key_order_tests {
    use super::*;
    use serde_json::json;

    /// 回归：启用 `preserve_order` 后，`Map::remove` 是 `swap_remove`——会把
    /// 最后一个元素搬到被删位置，打乱剩余键序。本模块承诺保留键序，故必须
    /// 用 `shift_remove`。
    #[test]
    fn removing_managed_keys_keeps_remaining_order() {
        let live = json!({
            "a": 1, "m1": "x", "b": 2, "m2": "x", "c": 3
        });
        let previous = json!({ "m1": "x", "m2": "x" });
        let next = json!({});

        let merged = merge_three_way(&live, Some(&previous), &next);

        let keys: Vec<&String> = merged.as_object().unwrap().keys().collect();
        assert_eq!(
            keys,
            vec!["a", "b", "c"],
            "剩余键序应保持 a,b,c；被打乱说明用了 swap_remove"
        );
    }

    /// JSON 渲染路径同样要保序（走的是同一个 `merge_three_way`）。
    #[test]
    fn json_rendering_keeps_order_after_removal() {
        let live = r#"{"z":1,"m":2,"a":3}"#;
        let out = merge_document(
            DocFormat::Json,
            live,
            Some(&json!({ "m": 2 })),
            &json!({ "z": 1, "a": 3 }),
        )
        .unwrap();

        let z = out.find("\"z\"").unwrap();
        let a = out.find("\"a\"").unwrap();
        assert!(z < a, "键序应保留 z 在 a 前:\n{out}");
    }
}
