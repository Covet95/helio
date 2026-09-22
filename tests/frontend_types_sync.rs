//! 前后端类型镜像守卫。
//!
//! `gui/src/types/index.ts` 是 Rust 结构体/枚举的**手写**镜像，仓库里没有任何
//! 自动生成机制（无 ts-rs / specta / typeshare）。因此一旦 Rust 侧新增字段或
//! 枚举取值而 TS 未同步，前端在「保存」时会把该字段静默丢弃：TypeScript 不会
//! 报错，运行时也看不出异常，数据就是少了一块。
//!
//! 这个测试把两侧的字段名集合与枚举取值集合钉死，把「静默丢弃」变成
//! 「`cargo test` 立刻失败」。
//!
//! 覆盖范围：跨 IPC 边界、且前端会回传给 Rust 的类型。
//! 有意不覆盖：
//! - 前端独有、Rust 侧不存在的类型（如 `ToolInfo` 由前端自行构造）；
//! - 描述 `serde_json::Value` 内部形状的类型（`OpenCodeModelConfig` 等，
//!   Rust 侧视其为不透明 JSON，没有可比对的字段集合）。
//!
//! 只依赖 std，不引入额外 crate。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// 必须与 `gui/src/types/index.ts` 字段一致的 Rust 结构体。
///
/// `ApiProfile` 经 `#[serde(flatten)]` 把 5 个子结构体摊平到顶层，本测试会
/// 自动递归展开，所以只需列出 `ApiProfile` 本身。
const MIRRORED_TYPES: &[&str] = &[
    "ApiProfile",
    "ApiKeyEntry",
    "CodexCatalogModel",
    "TargetStatus",
    "StatusInfo",
    "DatabaseInfo",
    "ToolProbeResult",
    "FetchedModel",
    "ModelTestResult",
    "SessionMeta",
    "PreviewMessage",
    "DeleteResult",
    "AppError",
    "LocalConfigInfo",
    "McpServerConfig",
];

/// 必须与 `gui/src/types/index.ts` 字符串联合类型一致的 Rust 枚举。
///
/// 比较的是**线上取值**（套用 `#[serde(rename_all)]` / `#[serde(rename)]` 之后
/// 的结果），而不是 Rust 里的变体名。
const MIRRORED_ENUMS: &[&str] = &["TargetApp", "ReachabilityStatus", "ErrorKind"];

const TS_TYPES_PATH: &str = "gui/src/types/index.ts";

// ---------------------------------------------------------------- 测试入口

#[test]
fn frontend_types_mirror_rust_structs() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let rust = RustIndex::load(root);
    let ts = TsIndex::load(&root.join(TS_TYPES_PATH));

    check_all_present(
        rust.bodies.keys(),
        ts.interfaces.keys(),
        MIRRORED_TYPES,
        "结构体",
        "interface",
    );

    let mut problems = Vec::new();
    for name in MIRRORED_TYPES {
        let rust_fields = rust
            .fields(name)
            .unwrap_or_else(|| panic!("{name}: 无法提取 Rust 字段"));
        let ts_fields = ts
            .fields(name)
            .unwrap_or_else(|| panic!("{name}: 无法提取 TS 字段"));

        let missing_in_ts: Vec<_> = rust_fields.difference(&ts_fields).cloned().collect();
        let missing_in_rust: Vec<_> = ts_fields.difference(&rust_fields).cloned().collect();

        if !missing_in_ts.is_empty() {
            problems.push(format!(
                "{name}: Rust 有而 {TS_TYPES_PATH} 缺 {} —— 前端回传时这些字段会被静默丢弃",
                fmt(&missing_in_ts)
            ));
        }
        if !missing_in_rust.is_empty() {
            problems.push(format!(
                "{name}: TS 有而 Rust 缺 {} —— 前端读到的永远是 undefined（多半是改名后残留或拼写错误）",
                fmt(&missing_in_rust)
            ));
        }
    }

    assert!(
        problems.is_empty(),
        "前后端类型已漂移，请同步 {TS_TYPES_PATH}：\n  {}",
        problems.join("\n  ")
    );
}

#[test]
fn frontend_enum_unions_mirror_rust_enums() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let rust = RustIndex::load(root);
    let ts = TsIndex::load(&root.join(TS_TYPES_PATH));

    check_all_present(
        rust.enums.keys(),
        ts.unions.keys(),
        MIRRORED_ENUMS,
        "枚举",
        "export type",
    );

    let mut problems = Vec::new();
    for name in MIRRORED_ENUMS {
        let rust_values = rust
            .enum_values(name)
            .unwrap_or_else(|| panic!("{name}: 无法提取 Rust 枚举取值"));
        let ts_values = ts
            .union_members(name)
            .unwrap_or_else(|| panic!("{name}: 无法提取 TS 联合成员"));

        let missing_in_ts: Vec<_> = rust_values.difference(&ts_values).cloned().collect();
        let missing_in_rust: Vec<_> = ts_values.difference(&rust_values).cloned().collect();

        if !missing_in_ts.is_empty() {
            problems.push(format!(
                "{name}: Rust 有而 {TS_TYPES_PATH} 缺 {} —— 前端无法表达该取值",
                fmt(&missing_in_ts)
            ));
        }
        if !missing_in_rust.is_empty() {
            problems.push(format!(
                "{name}: TS 有而 Rust 缺 {} —— 后端永远不会返回该取值",
                fmt(&missing_in_rust)
            ));
        }
    }

    assert!(
        problems.is_empty(),
        "枚举取值已漂移，请同步 {TS_TYPES_PATH}：\n  {}",
        problems.join("\n  ")
    );
}

/// 防止守卫自身悄悄失效：列出的类型必须真的被两侧解析到。
fn check_all_present<'a>(
    rust_names: impl Iterator<Item = &'a String>,
    ts_names: impl Iterator<Item = &'a String>,
    names: &[&str],
    rust_kind: &str,
    ts_kind: &str,
) {
    let rust_names: BTreeSet<&str> = rust_names.map(String::as_str).collect();
    let ts_names: BTreeSet<&str> = ts_names.map(String::as_str).collect();

    let mut problems = Vec::new();
    for name in names {
        if !rust_names.contains(name) {
            problems.push(format!(
                "{name}: 未能在 Rust 源码中找到该{rust_kind}（已改名或移动？）"
            ));
        }
        if !ts_names.contains(name) {
            problems.push(format!("{name}: 未能在 {TS_TYPES_PATH} 中找到该 {ts_kind}"));
        }
    }
    if !problems.is_empty() {
        panic!("类型镜像守卫自身失效：\n  {}", problems.join("\n  "));
    }
}

fn fmt(names: &[String]) -> String {
    format!("[{}]", names.join(", "))
}

// ---------------------------------------------------------------- Rust 侧

/// 从 Rust 源码提取 `pub struct` / `pub enum` 的主体与紧邻属性。
struct RustIndex {
    bodies: BTreeMap<String, (String, Vec<String>)>,
    enums: BTreeMap<String, (String, Vec<String>)>,
}

impl RustIndex {
    fn load(root: &Path) -> Self {
        let mut files = Vec::new();
        collect_rs_files(&root.join("src"), &mut files);
        collect_rs_files(&root.join("src-tauri/src"), &mut files);

        let mut bodies = BTreeMap::new();
        let mut enums = BTreeMap::new();
        for file in files {
            let Ok(raw) = std::fs::read_to_string(&file) else {
                continue;
            };
            let src = strip_comments(&raw);
            for (name, body, attrs) in named_bodies(&src, "pub struct ") {
                bodies.entry(name).or_insert((body, attrs));
            }
            for (name, body, attrs) in named_bodies(&src, "pub enum ") {
                enums.entry(name).or_insert((body, attrs));
            }
        }
        Self { bodies, enums }
    }

    fn fields(&self, name: &str) -> Option<BTreeSet<String>> {
        self.fields_inner(name, &mut BTreeSet::new())
    }

    fn fields_inner(&self, name: &str, seen: &mut BTreeSet<String>) -> Option<BTreeSet<String>> {
        if !seen.insert(name.to_string()) {
            // 自引用（理论上不会出现）：停止递归，避免无限展开。
            return Some(BTreeSet::new());
        }
        let (body, attrs) = self.bodies.get(name)?;
        let rename_all = serde_rename_all(attrs);

        let mut out = BTreeSet::new();
        let mut pending: Vec<&str> = Vec::new();
        for line in body.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if line.starts_with("#[") {
                pending.push(line);
                continue;
            }
            let Some(rest) = line.strip_prefix("pub ") else {
                pending.clear();
                continue;
            };
            let field_attrs = pending.join(" ");
            pending.clear();

            let Some(colon) = rest.find(':') else {
                continue;
            };
            if serde_skips(&field_attrs) {
                continue;
            }
            if field_attrs.contains("flatten") {
                let ty: String = rest[colon + 1..]
                    .trim()
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if let Some(sub) = self.fields_inner(&ty, seen) {
                    out.extend(sub);
                }
                continue;
            }
            let field = rest[..colon].trim();
            match serde_rename(&field_attrs) {
                Some(renamed) => out.insert(renamed),
                None => out.insert(apply_rename_all(field, rename_all.as_deref())),
            };
        }
        Some(out)
    }

    /// 枚举的线上取值集合。
    fn enum_values(&self, name: &str) -> Option<BTreeSet<String>> {
        let (body, attrs) = self.enums.get(name)?;
        let rename_all = serde_rename_all(attrs);

        let mut out = BTreeSet::new();
        let mut pending: Vec<&str> = Vec::new();
        for line in body.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if line.starts_with("#[") {
                pending.push(line);
                continue;
            }
            let variant_attrs = pending.join(" ");
            pending.clear();

            // 单元变体形如 `Operational,`。带载荷的变体本守卫不支持（当前不存在）。
            let variant: String = line
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if variant.is_empty() {
                continue;
            }
            match serde_rename(&variant_attrs) {
                Some(renamed) => out.insert(renamed),
                None => out.insert(apply_rename_all(&variant, rename_all.as_deref())),
            };
        }
        Some(out)
    }
}

/// 该字段是否被 serde 排除在序列化之外。
fn serde_skips(attrs: &str) -> bool {
    if attrs.contains("skip_serializing_if") {
        return false; // 条件跳过：字段仍可能出现在线上格式里
    }
    attrs.contains("skip_serializing") || attrs.contains("serde(skip)")
}

/// `#[serde(rename = "x")]` 的显式改名。
fn serde_rename(attrs: &str) -> Option<String> {
    let at = attrs.find("rename = \"")?;
    let rest = &attrs[at + "rename = \"".len()..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// `#[serde(rename_all = "...")]`。
fn serde_rename_all(attrs: &[String]) -> Option<String> {
    for a in attrs {
        if let Some(at) = a.find("rename_all = \"") {
            let rest = &a[at + "rename_all = \"".len()..];
            if let Some(end) = rest.find('"') {
                return Some(rest[..end].to_string());
            }
        }
    }
    None
}

/// 按 serde 的 `rename_all` 规则换算标识符。
fn apply_rename_all(ident: &str, rule: Option<&str>) -> String {
    match rule {
        Some("camelCase") => {
            let mut out = String::new();
            let mut upper_next = false;
            for c in ident.chars() {
                if c == '_' {
                    upper_next = true;
                } else if upper_next {
                    out.push(c.to_ascii_uppercase());
                    upper_next = false;
                } else {
                    out.push(c);
                }
            }
            out
        }
        Some("kebab-case") => {
            let mut out = String::new();
            for (i, c) in ident.chars().enumerate() {
                if c.is_ascii_uppercase() {
                    if i != 0 {
                        out.push('-');
                    }
                    out.push(c.to_ascii_lowercase());
                } else {
                    out.push(c);
                }
            }
            out
        }
        Some("snake_case") => to_snake(ident, false),
        Some("SCREAMING_SNAKE_CASE") => to_snake(ident, true),
        Some("lowercase") => ident.to_ascii_lowercase(),
        Some("UPPERCASE") => ident.to_ascii_uppercase(),
        _ => ident.to_string(),
    }
}

/// `PascalCase` / `camelCase` -> `snake_case`。
///
/// 曾经这里没有 `snake_case` 分支，于是 `#[serde(rename_all = "snake_case")]`
/// 会落到 `_ => ident.to_string()`，把 `NotFound` 当成线上取值 `NotFound` 去比对
/// TS 的 `'not_found'`——守卫不但没报错，还给出「已同步」的假象。
/// 任何新增的 rename_all 规则都必须在这里显式实现，否则就是静默失效。
///
/// 已知简化：连续大写（`HTTPServer`）会得到 `h_t_t_p_server`，而 serde 得到
/// `http_server`。当前枚举里没有这种变体；若将来出现，需要按大写连续段切分。
fn to_snake(ident: &str, screaming: bool) -> String {
    let mut out = String::new();
    for (i, c) in ident.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i != 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    if screaming {
        out.to_ascii_uppercase()
    } else {
        out
    }
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// 剥掉 `//` 行注释与 `/* */` 块注释，字符串字面量原样保留。
fn strip_comments(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    let mut in_str = false;
    let mut escaped = false;

    while i < b.len() {
        let c = b[i] as char;
        if in_str {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_str = true;
            out.push(c);
            i += 1;
            continue;
        }
        if c == '/' && i + 1 < b.len() && b[i + 1] as char == '/' {
            while i < b.len() && b[i] as char != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && i + 1 < b.len() && b[i + 1] as char == '*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] as char == '*' && b[i + 1] as char == '/') {
                i += 1;
            }
            i += 2;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// 收集源码里所有 `<marker><Name> { ... }` 的名字、主体与紧邻属性。
fn named_bodies(src: &str, marker: &str) -> Vec<(String, String, Vec<String>)> {
    let mut out = Vec::new();
    let mut from = 0usize;

    while let Some(rel) = src[from..].find(marker) {
        let at = from + rel;
        let after = &src[at + marker.len()..];
        let name_len = after
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(after.len());
        let name = &after[..name_len];
        let rest_off = at + marker.len() + name_len;

        let Some(open_rel) = src[rest_off..].find('{') else {
            break;
        };
        let open = rest_off + open_rel;
        let Some(body) = brace_body(src, open) else {
            break;
        };
        out.push((name.to_string(), body.to_string(), preceding_attrs(src, at)));
        from = open + 1 + body.len() + 1;
    }
    out
}

/// 取 `at` 之前紧邻的 `#[...]` 属性行（注释已被剥掉，只剩空行）。
fn preceding_attrs(src: &str, at: usize) -> Vec<String> {
    let mut out = Vec::new();
    for line in src[..at].lines().rev() {
        let s = line.trim();
        if s.is_empty() {
            continue;
        }
        if s.starts_with("#[") {
            out.push(s.to_string());
        } else {
            break;
        }
    }
    out.reverse();
    out
}

/// 返回 `open` 处 `{` 与配对 `}` 之间的内容。
fn brace_body(src: &str, open: usize) -> Option<&str> {
    let b = src.as_bytes();
    let mut depth = 0usize;
    let mut i = open;
    let mut in_str = false;
    let mut escaped = false;

    while i < b.len() {
        let c = b[i] as char;
        if in_str {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
        } else if c == '{' {
            depth += 1;
        } else if c == '}' {
            depth -= 1;
            if depth == 0 {
                return Some(&src[open + 1..i]);
            }
        }
        i += 1;
    }
    None
}

// ---------------------------------------------------------------- TS 侧

/// 从 `gui/src/types/index.ts` 提取 `export interface` 与 `export type`。
struct TsIndex {
    /// interface 名 → (主体, `extends` 的父接口)
    interfaces: BTreeMap<String, (String, Option<String>)>,
    /// type 别名名 → `=` 与 `;` 之间的原文
    unions: BTreeMap<String, String>,
}

impl TsIndex {
    fn load(path: &Path) -> Self {
        let raw = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("读取 {} 失败：{e}", path.display()));
        let src = strip_comments(&raw);

        let mut interfaces = BTreeMap::new();
        let mut from = 0usize;
        const IFACE: &str = "export interface ";

        while let Some(rel) = src[from..].find(IFACE) {
            let at = from + rel;
            let after = &src[at + IFACE.len()..];
            let name_len = after
                .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                .unwrap_or(after.len());
            let name = after[..name_len].to_string();
            let rest_off = at + IFACE.len() + name_len;

            let Some(open_rel) = src[rest_off..].find('{') else {
                break;
            };
            let header = &src[rest_off..rest_off + open_rel];
            let parent = header
                .split("extends")
                .nth(1)
                .map(|s| s.trim().trim_end_matches(',').trim().to_string())
                .filter(|s| !s.is_empty());

            let open = rest_off + open_rel;
            let Some(body) = brace_body(&src, open) else {
                break;
            };
            interfaces.insert(name, (body.to_string(), parent));
            from = open + 1 + body.len() + 1;
        }

        let mut unions = BTreeMap::new();
        let mut from = 0usize;
        const ALIAS: &str = "export type ";

        while let Some(rel) = src[from..].find(ALIAS) {
            let at = from + rel;
            let after = &src[at + ALIAS.len()..];
            let name_len = after
                .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                .unwrap_or(after.len());
            let name = after[..name_len].to_string();
            let rest_off = at + ALIAS.len() + name_len;

            let Some(eq_rel) = src[rest_off..].find('=') else {
                break;
            };
            let eq = rest_off + eq_rel;
            let Some(semi_rel) = src[eq..].find(';') else {
                break;
            };
            let semi = eq + semi_rel;
            unions.insert(name, src[eq + 1..semi].to_string());
            from = semi + 1;
        }

        Self { interfaces, unions }
    }

    fn fields(&self, name: &str) -> Option<BTreeSet<String>> {
        let (body, parent) = self.interfaces.get(name)?;
        let mut out = BTreeSet::new();
        if let Some(parent) = parent {
            if let Some(sub) = self.fields(parent) {
                out.extend(sub);
            }
        }
        for line in body.lines() {
            let line = line.trim();
            let Some((key, _)) = line.split_once(':') else {
                continue;
            };
            let key = key.trim().trim_end_matches('?').trim();
            // 跳过索引签名（[key: string]）与任何非标识符行。
            if !key.is_empty()
                && !key.starts_with(|c: char| c.is_ascii_digit())
                && key.chars().all(|c| c.is_alphanumeric() || c == '_')
            {
                out.insert(key.to_string());
            }
        }
        Some(out)
    }

    fn union_members(&self, name: &str) -> Option<BTreeSet<String>> {
        let body = self.unions.get(name)?;
        let mut out = BTreeSet::new();
        for part in body.split('|') {
            let member = part.trim().trim_matches(|c| c == '\'' || c == '"');
            if !member.is_empty() {
                out.insert(member.to_string());
            }
        }
        Some(out)
    }
}

// ------------------------------------------------- rename_all 换算自检
//
// 这些是守卫**自身**的测试。之前这里没有测试，于是 `apply_rename_all` 缺
// `snake_case` 分支这件事一直没被发现——守卫照常通过，给出「类型已同步」的
// 假象，而实际上根本没比对上。守卫不可信，比没有守卫更危险。

#[test]
fn rename_all_covers_every_rule_the_repo_uses() {
    assert_eq!(
        apply_rename_all("NotFound", Some("snake_case")),
        "not_found"
    );
    assert_eq!(
        apply_rename_all("PartialFailure", Some("snake_case")),
        "partial_failure"
    );
    assert_eq!(apply_rename_all("Io", Some("snake_case")), "io");
    assert_eq!(
        apply_rename_all("InvalidInput", Some("snake_case")),
        "invalid_input"
    );
    // 已经是 snake_case 的字段名不应被二次加工
    assert_eq!(apply_rename_all("api_url", Some("snake_case")), "api_url");

    assert_eq!(
        apply_rename_all("ClaudeCode", Some("kebab-case")),
        "claude-code"
    );
    assert_eq!(
        apply_rename_all("latency_ms", Some("camelCase")),
        "latencyMs"
    );
    assert_eq!(
        apply_rename_all("Operational", Some("lowercase")),
        "operational"
    );
    assert_eq!(
        apply_rename_all("api_key", Some("SCREAMING_SNAKE_CASE")),
        "API_KEY"
    );
    assert_eq!(apply_rename_all("ok", None), "ok");
}

#[test]
fn rename_all_never_silently_passes_through_a_known_rule() {
    // 回归保护：曾经的实现没有 snake_case 分支，落到 `_ => ident.to_string()`，
    // 于是 `NotFound` 原样返回，与 TS 的 'not_found' 永远对不上。
    // 这条断言把「规则必须真的被实现」钉死。
    for (ident, rule) in [
        ("NotFound", "snake_case"),
        ("ClaudeCode", "kebab-case"),
        ("latency_ms", "camelCase"),
        ("Operational", "lowercase"),
        ("api_key", "SCREAMING_SNAKE_CASE"),
    ] {
        assert_ne!(
            apply_rename_all(ident, Some(rule)),
            ident,
            "rename_all = {rule:?} 必须真的换算 {ident}，不能原样返回"
        );
    }
}
