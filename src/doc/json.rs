//! JSON 文档读写。
//!
//! JSON 本身无注释概念，但键序需要保留——`serde_json` 默认用 `BTreeMap`
//! 会按字典序重排，导致用户文件被「整理」得面目全非。这里启用
//! `preserve_order` feature（`IndexMap`）保住原始键序。

use anyhow::{Context, Result};
use serde_json::Value;

/// 解析 JSON 文本。保序由 `serde_json/preserve_order` feature 提供。
pub fn parse(text: &str) -> Result<Value> {
    serde_json::from_str(text).context("Failed to parse JSON document")
}

/// 渲染 JSON 文本：两空格缩进，行尾无多余空白。
pub fn render(value: &Value) -> Result<String> {
    let mut text =
        serde_json::to_string_pretty(value).context("Failed to serialize JSON document")?;
    text.push('\n');
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_preserves_key_order() {
        // 键序故意不是字典序——若实现用了 BTreeMap 会被重排成 a/b/z。
        let text = r#"{"z":1,"a":2,"b":{"y":1,"x":2}}"#;
        let parsed = parse(text).unwrap();
        let rendered = render(&parsed).unwrap();

        let z_pos = rendered.find("\"z\"").expect("z 应存在");
        let a_pos = rendered.find("\"a\"").expect("a 应存在");
        assert!(z_pos < a_pos, "键序应保留，实际渲染:\n{rendered}");

        let y_pos = rendered.find("\"y\"").expect("y 应存在");
        let x_pos = rendered.find("\"x\"").expect("x 应存在");
        assert!(y_pos < x_pos, "嵌套键序应保留，实际渲染:\n{rendered}");
    }

    #[test]
    fn render_is_stable_across_round_trips() {
        let text = r#"{"b":1,"a":[1,2,{"c":3}]}"#;
        let once = render(&parse(text).unwrap()).unwrap();
        let twice = render(&parse(&once).unwrap()).unwrap();
        assert_eq!(once, twice, "二次往返应稳定");
    }

    #[test]
    fn invalid_json_is_an_error_not_a_panic() {
        assert!(parse("{not json").is_err());
    }
}
