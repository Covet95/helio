//! JSON / JSONC 文档读写。
//!
//! 两件事：
//!
//! 1. **键序**：`serde_json` 默认用 `BTreeMap` 会按字典序重排，导致用户文件
//!    被「整理」。启用 `preserve_order` feature（`IndexMap`）保住原始键序。
//! 2. **JSONC 容错**：真实世界的工具配置常带注释与尾随逗号
//!    （`~/.claude.json`、`opencode.json` 都是）。`serde_json` 严格拒绝，
//!    早期实现因此退化为「整体覆盖」——注释和用户键一起丢。这里先剥掉
//!    注释与尾随逗号再解析。

use anyhow::{Context, Result};
use serde_json::Value;

/// 解析 JSON 文本，容忍 JSONC 扩展（注释、尾随逗号）。
///
/// 保序由 `serde_json/preserve_order` feature 提供。
pub fn parse(text: &str) -> Result<Value> {
    // 先按严格 JSON 试——绝大多数文件不需要预处理，省掉一次扫描。
    if let Ok(value) = serde_json::from_str(text) {
        return Ok(value);
    }
    let stripped = strip_jsonc(text);
    serde_json::from_str(&stripped).context("Failed to parse JSON document")
}

/// 剥离 JSONC 扩展，产出严格 JSON。
///
/// 处理两类扩展：
/// - `//` 行注释与 `/* */` 块注释（字符串内的 `//` 不受影响）；
/// - 尾随逗号（`[1,2,]` / `{"a":1,}`）。
///
/// 保留换行与空白，让报错位置尽量贴近原文。
pub fn strip_jsonc(input: &str) -> String {
    let without_comments = strip_comments(input);
    strip_trailing_commas(&without_comments)
}

fn strip_comments(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;

    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }

        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                chars.next();
                // 注释内容丢弃，但保留换行以维持行号。
                for nc in chars.by_ref() {
                    if nc == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = '\0';
                for nc in chars.by_ref() {
                    if nc == '\n' {
                        out.push('\n');
                    }
                    if prev == '*' && nc == '/' {
                        break;
                    }
                    prev = nc;
                }
            }
            _ => out.push(c),
        }
    }

    out
}

/// 删除对象/数组里紧跟在值后面的多余逗号（`[1,2,]` → `[1,2]`）。
fn strip_trailing_commas(input: &str) -> String {
    let bytes: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut in_string = false;
    let mut escaped = false;
    let mut index = 0;

    while index < bytes.len() {
        let c = bytes[index];

        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            index += 1;
            continue;
        }

        if c == '"' {
            in_string = true;
            out.push(c);
            index += 1;
            continue;
        }

        if c == ',' {
            // 向后看：跳过空白与注释残留的换行，若下一个非空白字符是
            // `]` 或 `}`，说明这是尾随逗号，丢弃。
            let mut lookahead = index + 1;
            while lookahead < bytes.len() && bytes[lookahead].is_whitespace() {
                lookahead += 1;
            }
            if lookahead < bytes.len() && (bytes[lookahead] == ']' || bytes[lookahead] == '}') {
                index += 1;
                continue;
            }
        }

        out.push(c);
        index += 1;
    }

    out
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

    // ---- JSONC 容错：真实工具配置常带注释与尾随逗号 ----

    #[test]
    fn parses_jsonc_with_comments_and_trailing_commas() {
        let jsonc = r#"{
  // 我的 Claude 配置
  "user_own": "keep-me",   // 行尾注释
  "numStartups": 42,
  "list": [1, 2, 3,],
  /* 块注释 */
  "nested": { "a": 1, },
}"#;
        let value = parse(jsonc).expect("JSONC 应可解析");

        assert_eq!(value["user_own"], serde_json::json!("keep-me"));
        assert_eq!(value["numStartups"], serde_json::json!(42));
        assert_eq!(value["list"], serde_json::json!([1, 2, 3]));
        assert_eq!(value["nested"]["a"], serde_json::json!(1));
    }

    /// 字符串里的 `//` 不能被当成注释。
    #[test]
    fn comment_like_content_inside_strings_is_preserved() {
        let jsonc = r#"{ "url": "https://example.com//path", "note": "a /* b */ c" }"#;
        let value = parse(jsonc).expect("应可解析");

        assert_eq!(value["url"], serde_json::json!("https://example.com//path"));
        assert_eq!(value["note"], serde_json::json!("a /* b */ c"));
    }

    /// 转义引号不能让解析器误判字符串结束。
    #[test]
    fn escaped_quotes_do_not_end_the_string() {
        let jsonc = r#"{ "a": "say \"hi\" // not a comment", "b": 1 }"#;
        let value = parse(jsonc).expect("应可解析");

        assert_eq!(value["a"], serde_json::json!("say \"hi\" // not a comment"));
        assert_eq!(value["b"], serde_json::json!(1));
    }

    /// 合法 JSON 走快路径，不受 JSONC 预处理影响。
    #[test]
    fn strict_json_is_unaffected() {
        let strict = r#"{"a":[1,2],"b":{"c":"d"}}"#;
        assert_eq!(parse(strict).unwrap(), parse(strict).unwrap());
        assert_eq!(parse(strict).unwrap()["b"]["c"], serde_json::json!("d"));
    }

    /// 尾随逗号剥离不应误删正常逗号。
    #[test]
    fn commas_between_values_are_kept() {
        let jsonc = r#"{"a": 1, "b": 2}"#;
        let value = parse(jsonc).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 2);
    }

    /// 空容器里的孤立逗号。
    #[test]
    fn handles_comma_before_closing_bracket() {
        let jsonc = r#"{"a": [1,], "b": {"c": 2,},}"#;
        let value = parse(jsonc).expect("应可解析");
        assert_eq!(value["a"], serde_json::json!([1]));
        assert_eq!(value["b"]["c"], serde_json::json!(2));
    }
}
