//! TOML 文档读写，**保留注释、键序与空白**。
//!
//! 这是本模块存在的理由：`toml::Value` → 序列化 的往返会把用户手写的注释、
//! 空行、键序全部抹掉。Helio 的 Codex 配置（`~/.codex/config.toml`）是用户会
//! 手改的文件，这种「整理」等同于破坏。
//!
//! 做法：用 `toml_edit::DocumentMut` 作为文档树——它把格式信息（装饰）挂在每个
//! 节点上，未触碰的节点原样保留。三路合并在 `DocumentMut` 上原生进行，
//! 因此**只有被摘除/覆盖的节点会丢失格式，其余一律不动**。
//!
//! 值级读写（[`parse`] / [`render`]）仍走 `serde_json::Value`，供上层做
//! 语义判断；它们不保格式，只用于「读出来看看」。

use anyhow::{Context, Result};
use serde_json::Value;
use toml_edit::{DocumentMut, Item, Table};

/// 解析 TOML 为 `serde_json::Value`（值级视图，不保格式）。
pub fn parse(text: &str) -> Result<Value> {
    let toml_value: toml::Value = toml::from_str(text).context("Failed to parse TOML document")?;
    Ok(toml_to_json(toml_value))
}

/// 渲染 `serde_json::Value` 为 TOML 文本（值级，不保格式）。
///
/// 仅用于「从零构造」的场景；从已有文件出发的写回必须走 [`merge_documents`]。
pub fn render(value: &Value) -> Result<String> {
    let toml_value = json_to_toml(value.clone())?;
    toml::to_string_pretty(&toml_value).context("Failed to serialize TOML document")
}

/// 三路合并，**保格式**。返回渲染好的文本。
///
/// 语义与 [`super::merge_three_way`] 完全一致，差别只在于它作用在保留格式的
/// `DocumentMut` 上，因此未被摘除/覆盖的注释与键序会原样存活。
pub fn merge_documents(
    live_text: &str,
    previous_managed_text: Option<&str>,
    next_managed_text: &str,
) -> Result<String> {
    let previous_value = match previous_managed_text {
        Some(text) if !text.trim().is_empty() => Some(parse(text)?),
        _ => None,
    };
    let next_value = if next_managed_text.trim().is_empty() {
        Value::Object(serde_json::Map::new())
    } else {
        parse(next_managed_text)?
    };

    merge_json_into_toml(live_text, previous_value.as_ref(), &next_value)
}

/// 三路合并的 TOML 实现：`live` 文本 + JSON 表达的前后受管片段。
///
/// 受管片段用 **JSON 值**而非 TOML 文本表达，原因是删除语义：
/// 「上次写了 `model_context_window`、这次不写」必须能表达为「删掉它」。
/// TOML 文本里没有「删除」这个写法，而 JSON 里「键不存在」天然就是该语义——
/// 因此 `previous_managed` 有、`next_managed` 无的键会被摘除。
///
/// 合并规则：
/// - `previous` 有、`next` 无 → 摘除（上次受管、本次不再管）；
/// - `next` 有 → 就地覆盖（保留原有装饰/注释）；
/// - 只在 `live` 里有的键 → 原样保留（用户手写内容）。
pub fn merge_json_into_toml(
    live_text: &str,
    previous_managed: Option<&Value>,
    next_managed: &Value,
) -> Result<String> {
    let mut live = parse_document(live_text, "live config")?;

    let empty = Value::Object(serde_json::Map::new());
    let previous = previous_managed.unwrap_or(&empty);

    merge_value_tables(live.as_table_mut(), previous, next_managed)?;

    Ok(live.to_string())
}

/// 三路合并的核心：以 `base` 为基底，摘掉「上次受管、本次不再管」的键，
/// 再把「本次受管」的键就地覆盖上去。
///
/// 与「先整体删除、再整体叠加」的朴素做法相比，这里**只删除 next 不管的键**，
/// 两边都管的键走就地覆盖。差别在于装饰（行尾注释、缩进）：整体删除会连注释
/// 一起丢掉，就地覆盖可以把它保留下来。注释是用户手写的，Helio 无权删除。
fn merge_value_tables(base: &mut Table, previous: &Value, next: &Value) -> Result<()> {
    let empty = serde_json::Map::new();
    let previous_map = previous.as_object().unwrap_or(&empty);
    let next_map = next.as_object().unwrap_or(&empty);

    // 1) 摘除：上次受管、本次不再受管的键。
    let mut stale = Vec::new();
    let mut recurse = Vec::new();
    for (key, previous_value) in previous_map {
        match next_map.get(key) {
            Some(next_value) => {
                // 两边都管：三方都是表才继续下探，否则交给第 2 步就地覆盖。
                let all_tables = matches!(base.get(key), Some(Item::Table(_)))
                    && previous_value.is_object()
                    && next_value.is_object();
                if all_tables {
                    recurse.push(key.clone());
                }
            }
            None => stale.push(key.clone()),
        }
    }
    for key in stale {
        base.remove(&key);
    }
    for key in recurse {
        let (Some(Item::Table(base_child)), Some(previous_value), Some(next_value)) = (
            base.get_mut(&key),
            previous_map.get(&key),
            next_map.get(&key),
        ) else {
            continue;
        };
        merge_value_tables(base_child, previous_value, next_value)?;
    }

    // 2) 叠加：本次受管的键就地覆盖，保留原有装饰。
    for (key, next_value) in next_map {
        match base.get_mut(key) {
            Some(base_item) => apply_value(base_item, next_value)?,
            None => {
                base.insert(key, json_to_toml_item(next_value)?);
            }
        }
    }

    Ok(())
}

/// 把 `patch` 写到 `base` 上，尽量保留 `base` 已有的装饰。
fn apply_value(base: &mut Item, patch: &Value) -> Result<()> {
    if let (Item::Table(base_table), Some(patch_map)) = (&mut *base, patch.as_object()) {
        for (key, patch_value) in patch_map {
            match base_table.get_mut(key) {
                Some(base_child) => apply_value(base_child, patch_value)?,
                None => {
                    base_table.insert(key, json_to_toml_item(patch_value)?);
                }
            }
        }
        return Ok(());
    }

    // 值级覆盖：搬运原装饰，否则行尾注释会随旧值一起消失。
    let decor = match &*base {
        Item::Value(existing) => Some(existing.decor().clone()),
        _ => None,
    };
    let mut item = json_to_toml_item(patch)?;
    if let (Item::Value(new_value), Some(decor)) = (&mut item, decor) {
        *new_value.decor_mut() = decor;
    }
    *base = item;
    Ok(())
}

/// 解析为保留格式的文档树。空文本 → 空文档。
pub fn parse_document(text: &str, label: &str) -> Result<DocumentMut> {
    if text.trim().is_empty() {
        return Ok(DocumentMut::new());
    }
    text.parse::<DocumentMut>()
        .with_context(|| format!("Failed to parse {label} as TOML"))
}


/// 在保留格式的前提下，对顶层键做「设置 / 删除」。
///
/// `updates` 中值为 `null` 表示删除该键，否则设置。**其余内容一律不动**——
/// 注释、键序、空行、子表全部原样保留。
///
/// 这是「改一个字段」场景的正确实现：不经过 JSON 往返，因此不会把用户的
/// 手写注释洗掉。
pub fn apply_top_level_updates(
    live_text: &str,
    updates: &serde_json::Map<String, Value>,
) -> Result<String> {
    let mut document = parse_document(live_text, "live config")?;

    for (key, value) in updates {
        if value.is_null() {
            document.as_table_mut().remove(key);
            continue;
        }

        // 保留原有节点的装饰（行尾注释、缩进、表前注释）。
        //
        // 理由：注释永远是用户手写的，Helio 无权删除。改值时若不搬运装饰，
        // `approval_policy = "never"  # 说明` 会变成 `approval_policy = "on-request"`，
        // 用户的说明文字无声消失。代价是：值变了而注释可能过时——但过时的注释
        // 用户看得见、删得掉，被删掉的文字则不可恢复。
        let preserved_decor = match document.as_table().get(key) {
            Some(Item::Value(existing)) => Some(existing.decor().clone()),
            Some(Item::Table(existing)) => Some(existing.decor().clone()),
            _ => None,
        };

        let mut item = json_to_toml_item(value)
            .with_context(|| format!("Failed to convert field '{key}' to a TOML value"))?;
        match (&mut item, preserved_decor) {
            (Item::Value(new_value), Some(decor)) => *new_value.decor_mut() = decor,
            (Item::Table(new_table), Some(decor)) => *new_table.decor_mut() = decor,
            _ => {}
        }

        document.as_table_mut().insert(key, item);
    }

    Ok(document.to_string())
}

/// `serde_json::Value` → `toml_edit::Item`。用于保真写入。
///
/// 直接构造 `toml_edit` 类型，不经过 `toml::Value`——两者依赖的 `toml_edit`
/// 版本不同（`toml` 0.8 用 0.20），跨版本转换不成立。
fn json_to_toml_item(value: &Value) -> Result<Item> {
    Ok(match value {
        Value::Null => anyhow::bail!("TOML 不支持 null 值；删除字段请用显式的 null 语义"),
        Value::Bool(b) => Item::Value((*b).into()),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Item::Value(i.into())
            } else if let Some(f) = n.as_f64() {
                Item::Value(f.into())
            } else {
                anyhow::bail!("无法转换数字 {n} 为 TOML 值")
            }
        }
        Value::String(s) => Item::Value(s.as_str().into()),
        Value::Array(items) => {
            let mut array = toml_edit::Array::new();
            for item in items {
                array.push(json_to_toml_value(item)?);
            }
            Item::Value(array.into())
        }
        Value::Object(map) => {
            let mut table = Table::new();
            for (k, v) in map {
                table.insert(k, json_to_toml_item(v)?);
            }
            Item::Table(table)
        }
    })
}

fn json_to_toml_value(value: &Value) -> Result<toml_edit::Value> {
    match json_to_toml_item(value)? {
        Item::Value(v) => Ok(v),
        _ => anyhow::bail!("TOML 数组元素不能是表；请改用表数组语法"),
    }
}

fn toml_to_json(value: toml::Value) -> Value {
    match value {
        toml::Value::String(s) => Value::String(s),
        toml::Value::Integer(i) => Value::Number(i.into()),
        toml::Value::Float(f) => serde_json::Number::from_f64(f)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        toml::Value::Boolean(b) => Value::Bool(b),
        toml::Value::Datetime(d) => Value::String(d.to_string()),
        toml::Value::Array(items) => {
            Value::Array(items.into_iter().map(toml_to_json).collect())
        }
        toml::Value::Table(table) => Value::Object(
            table
                .into_iter()
                .map(|(k, v)| (k, toml_to_json(v)))
                .collect(),
        ),
    }
}

fn json_to_toml(value: Value) -> Result<toml::Value> {
    Ok(match value {
        Value::Null => {
            anyhow::bail!("TOML 不支持 null 值；请在写入前剔除空字段")
        }
        Value::Bool(b) => toml::Value::Boolean(b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                toml::Value::Integer(i)
            } else if let Some(f) = n.as_f64() {
                toml::Value::Float(f)
            } else {
                anyhow::bail!("无法转换数字 {n} 为 TOML 值")
            }
        }
        Value::String(s) => toml::Value::String(s),
        Value::Array(items) => toml::Value::Array(
            items
                .into_iter()
                .map(json_to_toml)
                .collect::<Result<Vec<_>>>()?,
        ),
        Value::Object(map) => {
            let mut table = toml::map::Map::new();
            for (k, v) in map {
                table.insert(k, json_to_toml(v)?);
            }
            toml::Value::Table(table)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- 格式保真：本模块存在的理由 ----

    #[test]
    fn merge_preserves_comments_and_key_order() {
        let live = "\
# 用户手写的说明：这是我的 Codex 配置
model = \"gpt-5\"
approval_policy = \"never\"   # 行尾注释

[model_providers.custom]
base_url = \"https://old.example\"
";
        let previous = "\
[model_providers.custom]
base_url = \"https://old.example\"
";
        let next = "\
[model_providers.custom]
base_url = \"https://new.example\"
";

        let merged = merge_documents(live, Some(previous), next_text(next)).unwrap();

        assert!(merged.contains("# 用户手写的说明"), "顶层注释应保留:\n{merged}");
        assert!(merged.contains("# 行尾注释"), "行尾注释应保留:\n{merged}");
        assert!(merged.contains("https://new.example"), "受管字段应更新:\n{merged}");
        assert!(!merged.contains("https://old.example"), "旧值应被替换:\n{merged}");

        let model_pos = merged.find("model =").expect("model 应存在");
        let policy_pos = merged.find("approval_policy").expect("approval_policy 应存在");
        assert!(model_pos < policy_pos, "键序应保留:\n{merged}");
    }

    fn next_text(s: &str) -> &str {
        s
    }

    #[test]
    fn merge_removes_stale_managed_table() {
        let live = "\
[model_providers.old]
base_url = \"https://old.example\"
";
        let previous = "\
[model_providers.old]
base_url = \"https://old.example\"
";
        let next = "";

        let merged = merge_documents(live, Some(previous), next).unwrap();

        assert!(
            !merged.contains("old"),
            "不再受管的表应被摘除，实际:\n{merged}"
        );
    }

    #[test]
    fn merge_without_previous_keeps_everything_else() {
        let live = "\
# keep
model = \"gpt-5\"
";
        let next = "\
[model_providers.custom]
base_url = \"https://new.example\"
";

        let merged = merge_documents(live, None, next).unwrap();

        assert!(merged.contains("# keep"), "首次切换不应摘除任何内容:\n{merged}");
        assert!(merged.contains("model = \"gpt-5\""));
        assert!(merged.contains("https://new.example"));
    }

    #[test]
    fn merge_does_not_leave_empty_tables() {
        let live = "\
[model_providers.custom]
base_url = \"https://old.example\"
[other]
x = 1
";
        let previous = "\
[model_providers.custom]
base_url = \"https://old.example\"
";
        let next = "";

        let merged = merge_documents(live, Some(previous), next).unwrap();

        assert!(
            !merged.contains("[model_providers.custom]"),
            "摘空后的表应整体移除:\n{merged}"
        );
        assert!(merged.contains("[other]"), "无关表应保留:\n{merged}");
    }

    /// 受管字段被**重新赋值**时，其上的行尾注释必须存活。
    ///
    /// 这是「先整体删除、再整体叠加」与「只删 stale、就地覆盖」的分水岭：
    /// 前者会把注释连同旧值一起丢掉。
    #[test]
    fn merge_keeps_inline_comment_on_reassigned_field() {
        let live = "\
model = \"old\"    # 用户说明：这是我选的中转模型
";
        let previous = "model = \"old\"\n";
        let next = "model = \"new\"\n";

        let merged = merge_documents(live, Some(previous), next).unwrap();

        assert!(merged.contains("model = \"new\""), "值应更新:\n{merged}");
        assert!(
            merged.contains("# 用户说明：这是我选的中转模型"),
            "重新赋值不应丢失行尾注释:\n{merged}"
        );
    }

    /// 受管子表内的字段被重新赋值时，注释同样存活。
    #[test]
    fn merge_keeps_inline_comment_inside_managed_subtable() {
        let live = "\
[model_providers.custom]
base_url = \"https://old.example\"   # 我搭的中转
";
        let previous = "\
[model_providers.custom]
base_url = \"https://old.example\"
";
        let next = "\
[model_providers.custom]
base_url = \"https://new.example\"
";

        let merged = merge_documents(live, Some(previous), next).unwrap();

        assert!(merged.contains("https://new.example"), "值应更新:\n{merged}");
        assert!(merged.contains("# 我搭的中转"), "子表内注释应存活:\n{merged}");
    }

    // ---- 值级读写 ----

    #[test]
    fn value_round_trip() {
        let text = "model = \"gpt-5\"\ncount = 3\nflag = true\n";
        let value = parse(text).unwrap();
        assert_eq!(value["model"], serde_json::json!("gpt-5"));
        assert_eq!(value["count"], serde_json::json!(3));
        assert_eq!(value["flag"], serde_json::json!(true));

        let rendered = render(&value).unwrap();
        let reparsed = parse(&rendered).unwrap();
        assert_eq!(value, reparsed);
    }

    #[test]
    fn null_is_rejected_on_render() {
        let value = serde_json::json!({ "a": null });
        assert!(render(&value).is_err(), "TOML 无 null，应显式报错而非静默丢弃");
    }

    #[test]
    fn empty_text_is_an_empty_document() {
        let doc = parse_document("", "test").unwrap();
        assert!(doc.as_table().is_empty());
    }

    #[test]
    fn invalid_toml_is_an_error_not_a_panic() {
        assert!(parse("this is not = = toml").is_err());
        assert!(parse_document("a = = b", "test").is_err());
    }

    // ---- 保真字段编辑：修复「改一个字段洗掉全部注释」 ----

    #[test]
    fn field_update_preserves_comments_and_order() {
        let live = "\
# 我的 Codex 配置，别动注释
model = \"gpt-5\"          # 行尾也要留
approval_policy = \"never\"

[model_providers.custom]
base_url = \"https://x.example\"
";
        let mut updates = serde_json::Map::new();
        updates.insert("approval_policy".to_string(), serde_json::json!("on-request"));

        let result = apply_top_level_updates(live, &updates).unwrap();

        assert!(result.contains("# 我的 Codex 配置"), "顶层注释应保留:\n{result}");
        assert!(result.contains("# 行尾也要留"), "行尾注释应保留:\n{result}");
        assert!(result.contains("approval_policy = \"on-request\""), "字段应更新:\n{result}");
        assert!(!result.contains("\"never\""), "旧值应消失:\n{result}");
        assert!(result.contains("[model_providers.custom]"), "子表应保留:\n{result}");

        let model_pos = result.find("model =").unwrap();
        let policy_pos = result.find("approval_policy").unwrap();
        assert!(model_pos < policy_pos, "键序应保留:\n{result}");
    }

    #[test]
    fn field_update_with_null_removes_key() {
        let live = "model = \"gpt-5\"\nservice_tier = \"priority\"\n";
        let mut updates = serde_json::Map::new();
        updates.insert("service_tier".to_string(), Value::Null);

        let result = apply_top_level_updates(live, &updates).unwrap();

        assert!(!result.contains("service_tier"), "null 应删除该键:\n{result}");
        assert!(result.contains("model = \"gpt-5\""), "其余字段应保留:\n{result}");
    }

    #[test]
    fn field_update_adds_new_key_without_touching_rest() {
        let live = "# keep\nmodel = \"gpt-5\"\n";
        let mut updates = serde_json::Map::new();
        updates.insert("verbosity".to_string(), serde_json::json!("high"));

        let result = apply_top_level_updates(live, &updates).unwrap();

        assert!(result.contains("# keep"), "注释应保留:\n{result}");
        assert!(result.contains("verbosity = \"high\""), "新键应写入:\n{result}");
        assert!(result.contains("model = \"gpt-5\""));
    }

    #[test]
    fn field_update_on_empty_document() {
        let mut updates = serde_json::Map::new();
        updates.insert("model".to_string(), serde_json::json!("gpt-5"));

        let result = apply_top_level_updates("", &updates).unwrap();

        assert!(result.contains("model = \"gpt-5\""));
    }
}
