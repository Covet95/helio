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
/// **已知限制**：本实现只做「写入」，不做「摘除」。`yaml_edit` 其实提供
/// `Mapping::remove`，但摘除的语义（哪些键该删）需要与值级合并保持一致，
/// 而这套逻辑目前只在 [`super::merge_three_way`] 里。
///
/// 因此本函数只负责把 `merge_three_way` 的**结果**写进保格式文档；若写入
/// 后语义与结果不符（含「该删的没删」），由 `merge_document` 的安全网
/// 检测并退回值级重写。这样既不会写坏文件，也不会残留陈旧键。
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

/// 在映射 `mapping` 上写入 `key`：已有同形结构则下钻，否则插入新节点。
///
/// **关键约束（踩过坑）**：`yaml_edit` 只在节点**已挂在文档树上**时才知缩进
/// 上下文。任何「先在游离节点上构造好、再整体 set 进去」的写法都会丢缩进，
/// 产出把子键提到顶层的损坏 YAML。因此新增复合值时一律：
///
/// 1. 先把**空**节点 `set` 到树上；
/// 2. 再从树上取回句柄往里填。
///
/// 注意：`merge_document` 的外层安全网会把语义不符的结果退回值级重写，
/// 所以即使某条分支漏了缩进，也不会写出损坏文件——但会丢注释。这里是
/// 尽量保住注释的「正确路径」。
fn apply_mapping_entry(mapping: &yaml_edit::Mapping, key: &str, value: &Value) -> Result<()> {
    use yaml_edit::Mapping;

    match value {
        Value::Object(sub) => {
            if let Some(existing) = mapping.get_mapping(key) {
                for (k, v) in sub {
                    apply_mapping_entry(&existing, k, v)?;
                }
            } else {
                // attach-then-fill：先挂空节点，取回句柄再填。
                mapping.set(key, Mapping::new_pending_block());
                if let Some(fresh) = mapping.get_mapping(key) {
                    for (k, v) in sub {
                        apply_mapping_entry(&fresh, k, v)?;
                    }
                }
            }
        }
        Value::Array(items) => {
            // 序列按索引就地更新已有元素；长度变化时重建。
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
                    // 重建整条序列。
                    //
                    // 不能用 `Sequence::new_pending_block()` + `push`：对含映射
                    // 的元素，库算不出正确缩进，会产出把子键顶格的**非法 YAML**
                    // （实测）。而 `parse_raw` 接受块风格的完整序列文本，能产出
                    // 正确缩进——这是唯一可靠的重建方式。
                    let text = serde_yaml::to_string(&Value::Array(items.clone()))
                        .context("Failed to render YAML sequence")?;
                    let raw = yaml_edit::YamlValue::parse_raw(text.trim_end());
                    mapping.set(key, raw);
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

/// 标量 → `yaml_edit::YamlValue`。复合类型不走这里。
fn scalar_to_yaml(value: &Value) -> yaml_edit::YamlValue {
    use yaml_edit::YamlValue;

    match value {
        // YAML 有真正的 null（`~`）。早期实现写成 `scalar("null")`，产出的是
        // **带引号的字符串** `'null'`，回读成 `String("null")`——与 JSON/TOML
        // 语义都不一致。
        Value::Null => YamlValue::scalar(yaml_edit::ScalarValue::null()),
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
        // 复合值不应到达这里：调用方对 Object/Array 有专门分支。真到了说明
        // 有分支遗漏，字符串化会静默损坏数据——交给安全网去发现。
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

        assert!(
            merged.contains("# 我的 Hermes 配置"),
            "顶层注释丢失:\n{merged}"
        );
        assert!(merged.contains("# 行尾注释"), "行尾注释丢失:\n{merged}");
        assert!(
            merged.contains("# 下面是我自己的段"),
            "段前注释丢失:\n{merged}"
        );
        assert!(merged.contains("keep: me"), "自定义内容丢失:\n{merged}");
        assert!(
            merged.contains("https://new.example"),
            "值未更新:\n{merged}"
        );
        assert!(
            !merged.contains("https://old.example"),
            "旧值残留:\n{merged}"
        );
    }

    #[test]
    fn merge_without_previous_keeps_user_content() {
        let live = "# keep\nuser_key: value\n";
        let next = json!({ "api_url": "https://new.example" });

        let merged = merge_documents(live, None, &next).unwrap();

        assert!(merged.contains("# keep"), "首次切换不应删注释:\n{merged}");
        assert!(
            merged.contains("user_key: value"),
            "用户键应保留:\n{merged}"
        );
        assert!(
            merged.contains("https://new.example"),
            "新值应写入:\n{merged}"
        );
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

        assert_eq!(
            reparsed["model"]["default"],
            serde_yaml::Value::String("new".into())
        );
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

        assert!(
            merged.contains("https://new.example/v1"),
            "值应更新:\n{merged}"
        );
        assert!(merged.contains("# 我搭的中转"), "行尾注释应存活:\n{merged}");
        assert!(
            merged.contains("https://other.example/v1"),
            "其他项应保留:\n{merged}"
        );
    }

    #[test]
    fn empty_document_is_handled() {
        let merged = merge_documents("", None, &json!({ "a": 1 })).unwrap();
        assert!(merged.contains("a: 1"), "空文档应能写入:\n{merged}");
    }
}
