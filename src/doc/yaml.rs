//! YAML 文档读写，**保留注释、键序与空白**。
//!
//! 与 [`super::toml`] 同理：`serde_yaml` 的往返会把用户手写的注释抹掉，
//! 而 Hermes 的 `~/.hermes/config.yaml` 是用户会手改的文件。
//!
//! 用 `yaml_edit`（`toml_edit` 的 YAML 对应物）做保真编辑——它把格式信息
//! 挂在语法树上，未触碰的节点原样保留。

use anyhow::{Context, Result};
use serde_json::Value;
use yaml_edit::YamlFile;

/// 解析 YAML 为 `serde_json::Value`（值级视图，不保格式）。
pub fn parse(text: &str) -> Result<Value> {
    let value: serde_yaml::Value =
        serde_yaml::from_str(text).context("Failed to parse YAML document")?;
    serde_json::to_value(value).context("Failed to convert YAML document to JSON")
}

/// 渲染 `serde_json::Value` 为 YAML 文本（值级，不保格式）。
///
/// 仅用于「从零构造」；从已有文件出发的写回必须走 [`merge_documents`]。
pub fn render(value: &Value) -> Result<String> {
    let yaml_value: serde_yaml::Value =
        serde_yaml::to_value(value).context("Failed to convert JSON document to YAML")?;
    serde_yaml::to_string(&yaml_value).context("Failed to serialize YAML document")
}

/// 三路合并，**保格式**。返回渲染好的文本。
///
/// 语义与 [`super::merge_three_way`] 一致，差别只在于作用在保留格式的
/// `YamlFile` 上，因此未被摘除/覆盖的注释与键序会原样存活。
///
/// `yaml_edit` 不提供「删除键」的公开 API，因此摘除通过「整表重建」实现：
/// 先按值级三路合并算出最终文档，再把它写回原文档——`set` 只更新变化的
/// 节点，未触碰的注释保留。键被删除时该节点的注释一并消失，这是可接受的
/// 代价（用户可重新添加）。
pub fn merge_documents(
    live_text: &str,
    previous_managed: Option<&Value>,
    next_managed: &Value,
) -> Result<String> {
    // `yaml_edit` 对**空文档**的 `set` 会静默丢弃（文档节点不存在，无处可写），
    // 表现为「写入成功但文件仍为空」。空 live 没有内容需要保留，直接序列化。
    if live_text.trim().is_empty() {
        return render(next_managed);
    }

    let live_value = parse(live_text)?;
    let merged = super::merge_three_way(&live_value, previous_managed, next_managed);

    let document = YamlFile::parse(live_text)
        .to_result()
        .map_err(|error| anyhow::anyhow!("Failed to parse YAML document: {error}"))?;

    apply_value(&document, &merged)?;

    let out = document.to_string();
    if out.trim().is_empty() {
        // 兜底：解析成功但渲染为空（异常 YAML 形状）。宁可整体写入，
        // 也不能让调用方以为写成功、实际得到空文件。
        tracing::warn!("YAML 合并结果为空，退回整体序列化");
        return render(next_managed);
    }

    Ok(out)
}

/// 把 `value` 递归写入文档，**只改叶子**。
///
/// 关键约束：绝不整棵子树替换。`yaml_edit` 的 `set` 在写入复合值时会丢失
/// 嵌套缩进（实测产出 `model:\ndefault: old` 这类把子键提到顶层的损坏 YAML）。
/// 因此对已存在的映射/序列逐层下钻，只在标量叶子上 `set`；只有 live 中
/// 原本不存在的键才整体插入。
fn apply_value(document: &YamlFile, value: &Value) -> Result<()> {
    let Some(map) = value.as_object() else {
        return Ok(());
    };
    let Some(root) = document.document().and_then(|d| d.as_mapping()) else {
        return Ok(());
    };

    for (key, item) in map {
        apply_mapping_entry(&root, key, item)?;
    }
    Ok(())
}

/// 在映射 `mapping` 上写入 `key`：已有同形结构则下钻，否则整体插入。
fn apply_mapping_entry(mapping: &yaml_edit::Mapping, key: &str, value: &Value) -> Result<()> {
    use yaml_edit::{Mapping, Sequence};

    match value {
        Value::Object(sub) => {
            if let Some(existing) = mapping.get_mapping(key) {
                for (k, v) in sub {
                    apply_mapping_entry(&existing, k, v)?;
                }
            } else {
                let fresh = Mapping::new_pending_block();
                for (k, v) in sub {
                    apply_mapping_entry(&fresh, k, v)?;
                }
                mapping.set(key, fresh);
            }
        }
        Value::Array(items) => {
            // 序列按索引就地更新已有元素；长度变化时整体重建。
            //
            // 就地更新是为了保住每个元素的注释与格式——Helio 改的是
            // 数组里的某几项（如 custom_providers 的一个 provider），
            // 整体替换会把其他项的注释一并抹掉。
            match mapping.get_sequence(key) {
                Some(existing) if existing.len() == items.len() => {
                    for (index, item) in items.iter().enumerate() {
                        apply_sequence_item(&existing, index, item)?;
                    }
                }
                _ => {
                    let fresh = Sequence::new_pending_block();
                    for item in items {
                        fresh.push(sequence_element(item)?);
                    }
                    mapping.set(key, fresh);
                }
            }
        }
        scalar => {
            mapping.set(key, scalar_to_yaml(scalar));
        }
    }
    Ok(())
}

/// 就地更新序列的第 `index` 项：映射逐键下钻，标量直接覆盖。
fn apply_sequence_item(seq: &yaml_edit::Sequence, index: usize, value: &Value) -> Result<()> {
    let Some(existing) = seq.get(index) else {
        return Ok(());
    };
    match value {
        Value::Object(sub) => {
            if let Some(existing_map) = existing.as_mapping() {
                for (k, v) in sub {
                    apply_mapping_entry(existing_map, k, v)?;
                }
            }
        }
        _ => {
            seq.set(index, scalar_to_yaml(value));
        }
    }
    Ok(())
}

/// 构造序列元素（live 中无对应项时使用）。
///
/// 序列用 `Vec<YamlValue>` 表达——`YamlValue::from(Vec)` 是库提供的转换，
/// 而 `Sequence` 节点类型没有对应的 `From` 实现。
fn sequence_element(value: &Value) -> Result<yaml_edit::YamlValue> {
    use yaml_edit::YamlValue;

    Ok(match value {
        // 映射用 `BTreeMap` 表达：`YamlValue::from(BTreeMap)` 是库提供的
        // 转换，而 `Mapping` 节点类型没有对应实现。
        Value::Object(sub) => {
            let mut map = std::collections::BTreeMap::new();
            for (k, v) in sub {
                map.insert(k.clone(), sequence_element(v)?);
            }
            YamlValue::from(map)
        }
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(sequence_element(item)?);
            }
            YamlValue::from(out)
        }
        scalar => scalar_to_yaml(scalar),
    })
}

/// 标量 → `yaml_edit::YamlValue`。复合类型不走这里。
fn scalar_to_yaml(value: &Value) -> yaml_edit::YamlValue {
    use yaml_edit::YamlValue;

    match value {
        Value::Null => YamlValue::scalar("null"),
        Value::Bool(b) => YamlValue::scalar(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                YamlValue::scalar(i)
            } else if let Some(f) = n.as_f64() {
                YamlValue::scalar(f)
            } else {
                YamlValue::scalar(n.to_string())
            }
        }
        // 字符串交给 `scalar`——它会按 YAML 规则决定是否需要引号
        // （含 `:`、`#`、以 `-` 开头等都会自动加引号）。
        Value::String(s) => YamlValue::scalar(s.as_str()),
        composite => YamlValue::scalar(composite.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merge_preserves_comments() {
        let live = "\
# 我的 Hermes 配置 —— 手写注释
provider: custom       # 行尾注释
api_url: https://old.example

# 下面是我自己的段
my_section:
  keep: me
";
        let previous = json!({ "api_url": "https://old.example" });
        let next = json!({ "api_url": "https://new.example" });

        let merged = merge_documents(live, Some(&previous), &next).unwrap();

        assert!(merged.contains("# 我的 Hermes 配置"), "顶层注释丢失:\n{merged}");
        assert!(merged.contains("# 行尾注释"), "行尾注释丢失:\n{merged}");
        assert!(merged.contains("# 下面是我自己的段"), "段前注释丢失:\n{merged}");
        assert!(merged.contains("keep: me"), "自定义内容丢失:\n{merged}");
        assert!(merged.contains("https://new.example"), "值未更新:\n{merged}");
        assert!(!merged.contains("https://old.example"), "旧值残留:\n{merged}");
    }

    #[test]
    fn merge_without_previous_keeps_user_content() {
        let live = "# keep\nuser_key: value\n";
        let next = json!({ "api_url": "https://new.example" });

        let merged = merge_documents(live, None, &next).unwrap();

        assert!(merged.contains("# keep"), "首次切换不应删注释:\n{merged}");
        assert!(merged.contains("user_key: value"), "用户键应保留:\n{merged}");
        assert!(merged.contains("https://new.example"), "新值应写入:\n{merged}");
    }

    #[test]
    fn value_round_trip() {
        let text = "model: gpt-5\ncount: 3\nflag: true\n";
        let value = parse(text).unwrap();
        assert_eq!(value["model"], json!("gpt-5"));
        assert_eq!(value["count"], json!(3));
        assert_eq!(value["flag"], json!(true));

        let rendered = render(&value).unwrap();
        let reparsed = parse(&rendered).unwrap();
        assert_eq!(value, reparsed);
    }

    #[test]
    fn invalid_yaml_is_an_error_not_a_panic() {
        assert!(parse("key: [unclosed").is_err());
    }

    /// 回归：嵌套结构必须保住缩进。
    ///
    /// 早期实现用 `YamlValue::from(BTreeMap)` / `Mapping::new_pending_block()`
    /// 整体替换子树，产出 `model:\ndefault: old` 这种把子键提到顶层的**损坏
    /// YAML**（子键与父键同列）。现在改为逐层下钻、只改叶子。
    #[test]
    fn merge_preserves_nesting_indentation() {
        let live = "\
custom_providers:
- name: target
  base_url: https://old.example/v1
model:
  default: old
  provider: custom:target
mcp_servers:
  keep:
    command: uvx
";
        let previous = json!({ "model": { "default": "old", "provider": "custom:target" } });
        let next = json!({
            "model": { "default": "new", "provider": "custom:target" },
            "mcp_servers": { "keep": { "command": "uvx" } },
        });

        let merged = merge_documents(live, Some(&previous), &next).unwrap();

        // 必须能重新解析——损坏的缩进会在这里失败。
        let reparsed: serde_yaml::Value = serde_yaml::from_str(&merged)
            .unwrap_or_else(|e| panic!("合并结果不是合法 YAML：{e}\n{merged}"));

        assert_eq!(reparsed["model"]["default"], serde_yaml::Value::String("new".into()));
        assert_eq!(
            reparsed["mcp_servers"]["keep"]["command"],
            serde_yaml::Value::String("uvx".into())
        );
        assert!(merged.contains("custom_providers"), "序列应保留:\n{merged}");
    }

    /// 回归：序列内元素的非受管字段与注释必须存活。
    #[test]
    fn merge_updates_sequence_item_in_place() {
        let live = "\
custom_providers:
- name: target
  base_url: https://old.example/v1   # 我搭的中转
- name: other
  base_url: https://other.example/v1
";
        let previous = json!({});
        let next = json!({
            "custom_providers": [
                { "name": "target", "base_url": "https://new.example/v1" },
                { "name": "other", "base_url": "https://other.example/v1" },
            ]
        });

        let merged = merge_documents(live, Some(&previous), &next).unwrap();

        assert!(merged.contains("https://new.example/v1"), "值应更新:\n{merged}");
        assert!(merged.contains("# 我搭的中转"), "行尾注释应存活:\n{merged}");
        assert!(merged.contains("https://other.example/v1"), "其他项应保留:\n{merged}");
    }

    #[test]
    fn empty_document_is_handled() {
        let merged = merge_documents("", None, &json!({ "a": 1 })).unwrap();
        assert!(merged.contains("a: 1"), "空文档应能写入:\n{merged}");
    }
}
