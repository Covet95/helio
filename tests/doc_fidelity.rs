//! `doc` 模块的跨层契约：格式保真编辑在**真实形状**的工具配置上成立。
//!
//! 单元测试用的是小片段；这里用接近生产的手写 `config.toml`（含注释、空行、
//! 多级子表、schema 行）验证「改一个字段」不会破坏其余内容——这正是旧实现
//! （TOML → JSON → 全量重新序列化）会失败的地方。

use switch_api::doc::toml::apply_top_level_updates;

const LIVE_CONFIG: &str = r#"#:schema none
# 我的 Codex 配置 —— 手写注释，不许动
model = "gpt-5.6-sol"
model_provider = "custom"
approval_policy = "never"        # 我习惯永不确认

[model_providers.custom]
name = "我的中转"
base_url = "https://relay.example/v1"
wire_api = "responses"

[mcp_servers.filesystem]
command = "npx"

[features]
plugins = true
"#;

fn updates(pairs: &[(&str, serde_json::Value)]) -> serde_json::Map<String, serde_json::Value> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

#[test]
fn editing_a_field_keeps_every_handwritten_comment() {
    let out = apply_top_level_updates(
        LIVE_CONFIG,
        &updates(&[
            ("approval_policy", serde_json::json!("on-request")),
            ("service_tier", serde_json::json!("priority")),
        ]),
    )
    .unwrap();

    // 注释：顶层、行尾、子表前，一个都不能少。
    assert!(out.contains("手写注释，不许动"), "顶层注释丢失:\n{out}");
    assert!(out.contains("我习惯永不确认"), "行尾注释丢失:\n{out}");

    // 未受管的子表必须原样存活。
    assert!(
        out.contains("[model_providers.custom]"),
        "provider 表丢失:\n{out}"
    );
    assert!(
        out.contains("base_url = \"https://relay.example/v1\""),
        "base_url 丢失:\n{out}"
    );
    assert!(
        out.contains("[mcp_servers.filesystem]"),
        "mcp 表丢失:\n{out}"
    );
    assert!(out.contains("[features]"), "features 表丢失:\n{out}");
    assert!(out.contains("plugins = true"), "plugins 丢失:\n{out}");

    // 受管字段被更新 / 新增。
    assert!(
        out.contains("approval_policy = \"on-request\""),
        "字段未更新:\n{out}"
    );
    assert!(
        out.contains("service_tier = \"priority\""),
        "新字段未写入:\n{out}"
    );
    assert!(!out.contains("\"never\""), "旧值残留:\n{out}");
}

#[test]
fn editing_a_field_keeps_key_order() {
    let out = apply_top_level_updates(
        LIVE_CONFIG,
        &updates(&[("approval_policy", serde_json::json!("on-request"))]),
    )
    .unwrap();

    let model = out.find("model =").expect("model 应存在");
    let provider = out.find("model_provider =").expect("model_provider 应存在");
    let policy = out
        .find("approval_policy =")
        .expect("approval_policy 应存在");
    assert!(model < provider, "键序被打乱:\n{out}");
    assert!(provider < policy, "键序被打乱:\n{out}");
}

#[test]
fn removing_a_field_keeps_the_rest() {
    let out = apply_top_level_updates(
        LIVE_CONFIG,
        &updates(&[("approval_policy", serde_json::Value::Null)]),
    )
    .unwrap();

    assert!(!out.contains("approval_policy"), "字段应被删除:\n{out}");
    assert!(out.contains("手写注释，不许动"), "删除不应影响注释:\n{out}");
    assert!(
        out.contains("[model_providers.custom]"),
        "删除不应影响子表:\n{out}"
    );
}

/// 幂等：同样的编辑连做两次，第二次不再改变文本。
#[test]
fn repeated_edits_are_stable() {
    let updates = updates(&[("approval_policy", serde_json::json!("on-request"))]);

    let once = apply_top_level_updates(LIVE_CONFIG, &updates).unwrap();
    let twice = apply_top_level_updates(&once, &updates).unwrap();

    assert_eq!(once, twice, "重复编辑应稳定");
}

/// 用户手写的、与受管字段无关的自定义顶层键必须存活。
#[test]
fn custom_user_keys_survive() {
    let live = format!("{LIVE_CONFIG}my_custom_key = \"do not touch\"\n");
    let out = apply_top_level_updates(
        &live,
        &updates(&[("approval_policy", serde_json::json!("on-request"))]),
    )
    .unwrap();

    assert!(
        out.contains("my_custom_key = \"do not touch\""),
        "自定义键丢失:\n{out}"
    );
}
