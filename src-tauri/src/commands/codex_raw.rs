//! Codex `config.toml` 的原始文本编辑：读、写、字段级更新。
//!
//! 与 `adapters::codex` 的分工：适配器负责「按 Profile 生成配置」，本模块
//! 负责「用户直接编辑原始 TOML」这条路径——包括校验、备份、原子写、
//! 以及写失败时的补偿回滚（DB 行 + 文件都要还原）。
//!
//! 高风险写盘路径：所有入口都必须「校验通过才写」+「写前备份」。

use crate::commands::{AppError, AppState};
use switch_api::db::Database;
use switch_api::models::{ApiProfile, TargetApp};
use tauri::State;

/// 读取 Codex 的 config.toml 原始文本（不经 JSON 往返，保留用户格式/注释）。
/// 文件不存在时返回空字符串。仅 Codex 提供此能力。
#[tauri::command]
pub async fn read_codex_config_raw() -> Result<String, AppError> {
    use switch_api::adapters::get_adapter;
    let path = get_adapter(TargetApp::Codex).config_path();
    if !path.exists() {
        return Ok(String::new());
    }
    std::fs::read_to_string(&path)
        .map_err(|e| AppError::from(e).with_context("读取 config.toml 失败"))
}

fn toml_string_field(value: Option<&toml::Value>, key: &str) -> Option<String> {
    value
        .and_then(|value| value.as_table())
        .and_then(|table| table.get(key))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn sync_codex_profile_from_raw_config(profile: &ApiProfile, config: &toml::Value) -> ApiProfile {
    let mut synced = profile.clone();
    let provider_id = toml_string_field(Some(config), "model_provider")
        .unwrap_or_else(|| profile.provider.clone());
    let provider = config
        .as_table()
        .and_then(|table| table.get("model_providers"))
        .and_then(|value| value.as_table())
        .and_then(|providers| providers.get(&provider_id));

    synced.provider = provider_id.clone();
    synced.target_app = Some(TargetApp::Codex);
    synced.model = toml_string_field(Some(config), "model");
    synced.context_1m = config
        .as_table()
        .and_then(|table| table.get("model_context_window"))
        .and_then(|value| value.as_integer())
        .map(|value| value >= 1_000_000);
    synced.codex.reasoning_effort = toml_string_field(Some(config), "model_reasoning_effort");
    synced.codex.reasoning_summary = toml_string_field(Some(config), "model_reasoning_summary");
    synced.codex.verbosity = toml_string_field(Some(config), "model_verbosity");
    synced.codex.service_tier = toml_string_field(Some(config), "service_tier");
    synced.codex.wire_api = toml_string_field(provider, "wire_api")
        .and_then(|w| switch_api::models::normalize_wire_api(Some(&w)).or(Some(w)));
    synced.codex.env_key = toml_string_field(provider, "env_key");
    synced.codex.experimental_bearer_token =
        toml_string_field(provider, "experimental_bearer_token");
    let auth_table = provider
        .and_then(|value| value.as_table())
        .and_then(|table| table.get("auth"))
        .and_then(|value| value.as_table());
    synced.codex.auth_command = auth_table
        .and_then(|table| table.get("command"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    synced.codex.auth_args = auth_table
        .and_then(|table| table.get("args"))
        .and_then(|value| value.as_array())
        .map(|args| {
            args.iter()
                .filter_map(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|args| !args.is_empty());
    synced.codex.auth_timeout_ms = auth_table
        .and_then(|table| table.get("timeout_ms"))
        .and_then(|value| value.as_integer())
        .filter(|value| *value > 0);
    synced.codex.auth_refresh_interval_ms = auth_table
        .and_then(|table| table.get("refresh_interval_ms"))
        .and_then(|value| value.as_integer())
        .filter(|value| *value > 0);
    synced.codex.auth_cwd = auth_table
        .and_then(|table| table.get("cwd"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    synced.codex.requires_openai_auth = provider
        .and_then(|value| value.as_table())
        .and_then(|table| table.get("requires_openai_auth"))
        .and_then(|value| value.as_bool());
    synced.codex.supports_standalone_web_search = provider
        .and_then(|value| value.as_table())
        .and_then(|table| table.get("supports_standalone_web_search"))
        .and_then(|value| value.as_bool());

    if provider_id == "amazon-bedrock" {
        synced.api_url.clear();
        synced.codex.aws_profile = toml_string_field(
            provider
                .and_then(|value| value.as_table())
                .and_then(|table| table.get("aws")),
            "profile",
        );
        synced.codex.aws_region = toml_string_field(
            provider
                .and_then(|value| value.as_table())
                .and_then(|table| table.get("aws")),
            "region",
        );
    } else {
        synced.api_url = toml_string_field(provider, "base_url")
            .or_else(|| toml_string_field(Some(config), "base_url"))
            .unwrap_or_default();
        synced.codex.aws_profile = None;
        synced.codex.aws_region = None;
    }

    synced
}

fn restore_codex_raw_file(path: &std::path::Path, previous: Option<&[u8]>) -> Result<(), String> {
    match previous {
        Some(contents) => switch_api::utils::secure_fs::atomic_write_private(path, contents)
            .map_err(|error| format!("恢复 Codex config.toml 失败：{error}")),
        None if path.exists() => {
            std::fs::remove_file(path).map_err(|error| format!("删除新 Codex config 失败：{error}"))
        }
        None => Ok(()),
    }
}

fn restore_codex_profile_state(
    db: &Database,
    previous_profile: Option<&ApiProfile>,
    previous_shared_config: Option<&serde_json::Value>,
) -> Result<(), String> {
    if let Some(profile) = previous_profile {
        db.update_profile(profile)
            .map_err(|error| format!("恢复 Codex active Profile 失败：{error}"))?;
    }
    match previous_shared_config {
        Some(config) => db
            .save_shared_config(TargetApp::Codex, config.clone())
            .map_err(|error| format!("恢复 Codex shared config 失败：{error}")),
        None => db
            .delete_shared_config(TargetApp::Codex)
            .map_err(|error| format!("删除失败的 Codex shared config 失败：{error}")),
    }
}

/// 把回滚结果并入错误：**回滚失败 = 部分生效 → PartialFailure**。
///
/// 这条判断很关键。回滚成功的失败是干净的（什么都没落地，可以放心重试）；
/// 回滚失败的失败意味着磁盘上可能留着半套配置，前端必须提示用户去核对工具
/// 实际状态，而不是让用户重试一遍。
fn with_rollback(err: AppError, rollback_errors: Vec<String>) -> AppError {
    if rollback_errors.is_empty() {
        return err;
    }
    AppError::partial_failure(format!(
        "{}；回滚失败：{}",
        err.message,
        rollback_errors.join("；")
    ))
    .with_detail(err.detail.unwrap_or_default())
}

fn persist_codex_raw_config(content: &str, state: &AppState) -> Result<(), AppError> {
    let parsed = toml::from_str::<toml::Value>(content)
        .map_err(|error| AppError::invalid_input(format!("TOML 语法错误，未保存：{error}")))?;
    let adapter = switch_api::adapters::get_adapter(TargetApp::Codex);
    let path = adapter.config_path();
    let shared = adapter.extract_shared_config(
        &serde_json::to_value(&parsed)
            .map_err(|error| AppError::internal(format!("转换 TOML 失败：{error}")))?,
    );
    let previous_contents =
        if path.exists() {
            Some(std::fs::read(&path).map_err(|error| {
                AppError::from(error).with_context("读取当前 Codex config 失败")
            })?)
        } else {
            None
        };

    let _write_guard = state.config_lock.lock()?;
    let db = state.db.lock()?;
    let previous_profile = db
        .get_active_profile_full(TargetApp::Codex)
        .map_err(|error| AppError::from(error).with_context("读取 Codex active Profile 失败"))?;
    let previous_shared_config = db
        .get_shared_config(TargetApp::Codex)
        .map_err(|error| AppError::from(error).with_context("读取 Codex shared config 失败"))?
        .map(|config| config.config);
    let synced_profile = previous_profile
        .as_ref()
        .map(|profile| sync_codex_profile_from_raw_config(profile, &parsed));

    if path.exists() {
        adapter
            .backup_config()
            .map_err(|error| AppError::from(error).with_context("备份当前配置失败"))?;
    }

    if let Some(profile) = synced_profile.as_ref() {
        // db 层已经返回 AppError，直接补上下文即可；再套一层 AppError::from 是空转。
        db.update_profile(profile)
            .map_err(|error| error.with_context("同步 Codex active Profile 失败"))?;
    }

    // 两处写盘失败走同一套补偿：数据库行 + config.toml 都要还原。
    let rollback = |db: &Database| -> Vec<String> {
        let mut errors = Vec::new();
        if let Err(e) = restore_codex_profile_state(
            db,
            previous_profile.as_ref(),
            previous_shared_config.as_ref(),
        ) {
            errors.push(e);
        }
        if let Err(e) = restore_codex_raw_file(&path, previous_contents.as_deref()) {
            errors.push(e);
        }
        errors
    };

    if let Err(error) = validate_and_write_codex_config_raw(content, &path) {
        return Err(with_rollback(AppError::io(error), rollback(&db)));
    }

    if let Err(error) = db.save_shared_config(TargetApp::Codex, shared) {
        return Err(with_rollback(
            error.with_context("保存 Codex shared config 失败"),
            rollback(&db),
        ));
    }

    Ok(())
}

/// 保存用户在 GUI 里手编的 Codex config.toml 原始文本。
/// 高风险写操作：必须「校验通过才写」+「写前备份」。
#[tauri::command]
pub async fn save_codex_config_raw(
    content: String,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    persist_codex_raw_config(&content, &state)
}

/// 「校验 + 原子写入」核心逻辑，接受路径参数便于单测（不依赖真实 HOME）。
/// 先用 toml::from_str 校验，非法则返回 Err 且不写盘；合法则临时文件 + rename
/// 原子写入原始文本，返回解析出的 toml::Value。
fn validate_and_write_codex_config_raw(
    content: &str,
    path: &std::path::Path,
) -> Result<toml::Value, String> {
    let parsed = toml::from_str::<toml::Value>(content)
        .map_err(|e| format!("TOML 语法错误，未保存：{}", e))?;

    switch_api::utils::secure_fs::atomic_write_private(path, content.as_bytes())
        .map_err(|e| format!("替换 config.toml 失败：{}", e))?;

    Ok(parsed)
}

/// 编辑 Codex 全局行为字段（approval_policy / sandbox_mode 等顶层键）并写回
/// ~/.codex/config.toml。
///
/// 走**保留格式**的编辑路径：直接在 live 文本上改这几个顶层键，其余内容
/// （注释、键序、空行、子表）一律不动。早期实现是「TOML → JSON → 改字段 →
/// 全量重新序列化」，会把用户手写的注释和键序全部洗掉。
///
/// 与原始文本编辑共用同一事务路径（校验 + 备份 + 原子写 + 同步 active Profile）。
#[tauri::command]
pub async fn update_codex_fields(
    fields: serde_json::Value,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    use switch_api::adapters::get_adapter;
    let adapter = get_adapter(TargetApp::Codex);
    let path = adapter.config_path();

    let live_text = if path.exists() {
        std::fs::read_to_string(&path)
            .map_err(|e| AppError::from(e).with_context("读取 config.toml 失败"))?
    } else {
        String::new()
    };

    let updates = fields
        .as_object()
        .ok_or_else(|| AppError::invalid_input("字段更新必须是一个对象"))?;

    let content = switch_api::doc::toml::apply_top_level_updates(&live_text, updates)
        .map_err(|e| AppError::invalid_input(format!("更新 config.toml 失败：{e}")))?;

    persist_codex_raw_config(&content, &state)
}

#[cfg(test)]
mod codex_raw_config_tests {
    use super::validate_and_write_codex_config_raw;
    use std::sync::atomic::{AtomicU64, Ordering};

    static CTR: AtomicU64 = AtomicU64::new(0);

    fn temp_path(name: &str) -> std::path::PathBuf {
        let n = CTR.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("switch-api-codex-raw-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn test_bad_toml_returns_err_and_does_not_write() {
        let path = temp_path("config.toml");
        // 预置一个已存在的合法文件，验证坏 TOML 不会覆盖它
        std::fs::write(&path, "model_provider = \"openai\"\n").unwrap();

        let result = validate_and_write_codex_config_raw("this is = = not valid", &path);
        assert!(result.is_err());
        // 原文件未被改动
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert_eq!(on_disk, "model_provider = \"openai\"\n");
        // 不留临时文件
        assert!(!path.with_extension("toml.tmp").exists());

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn test_good_toml_writes_raw_content_verbatim() {
        let path = temp_path("config.toml");
        // 带注释和格式，验证原始文本被逐字写入（不经序列化往返）
        let content = "# my codex config\nmodel_provider = \"openai-custom\"\n\n[model_providers.openai-custom]\nbase_url = \"https://api.example.com/v1\"\n";

        let parsed = validate_and_write_codex_config_raw(content, &path).unwrap();
        // 返回解析结果可用
        assert_eq!(parsed["model_provider"].as_str(), Some("openai-custom"));
        // 磁盘内容与输入逐字一致（注释保留）
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert_eq!(on_disk, content);
        // 不留临时文件
        assert!(!path.with_extension("toml.tmp").exists());

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}

/// `update_codex_fields` 的字段编辑语义基线。
/// 这些断言原先针对 `apply_field_updates`（在 JSON 值上改字段，再由调用方全量
/// 重新序列化）。改为保真路径后语义不变，但**额外**保证：注释、键序、空行与
/// 未受管子表原样存活——旧实现会把它们全部洗掉。
#[cfg(test)]
mod codex_field_update_tests {
    use serde_json::json;
    use switch_api::doc::toml::apply_top_level_updates;

    /// 把 JSON 对象转成 `apply_top_level_updates` 需要的 Map。
    fn updates(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        value.as_object().expect("updates 必须是对象").clone()
    }

    #[test]
    fn test_set_new_field() {
        let live = "model_provider = \"openai\"\n";
        let result =
            apply_top_level_updates(live, &updates(json!({ "approval_policy": "never" }))).unwrap();
        assert!(result.contains("approval_policy = \"never\""), "{result}");
        // 原有字段不受影响
        assert!(result.contains("model_provider = \"openai\""), "{result}");
    }

    #[test]
    fn test_override_existing_field() {
        let live = "sandbox_mode = \"read-only\"\n";
        let result =
            apply_top_level_updates(live, &updates(json!({ "sandbox_mode": "workspace-write" })))
                .unwrap();
        assert!(
            result.contains("sandbox_mode = \"workspace-write\""),
            "{result}"
        );
        assert!(!result.contains("read-only"), "{result}");
    }

    #[test]
    fn test_null_removes_field() {
        let live = "service_tier = \"fast\"\nmodel_provider = \"openai\"\n";
        let result =
            apply_top_level_updates(live, &updates(json!({ "service_tier": null }))).unwrap();
        assert!(
            !result.contains("service_tier"),
            "null 应删除该键:\n{result}"
        );
        // 其他字段保留
        assert!(result.contains("model_provider = \"openai\""), "{result}");
    }

    #[test]
    fn test_does_not_touch_other_fields() {
        let live = "\
model_provider = \"openai\"
approval_policy = \"on-request\"

[model_providers.openai]
base_url = \"https://api.com\"

[mcp_servers.fs]
command = \"npx\"
";
        let result = apply_top_level_updates(
            live,
            &updates(json!({
                "approval_policy": "untrusted",
                "model_auto_compact_token_limit": 200000,
                "disable_response_storage": true,
            })),
        )
        .unwrap();

        // 改了/加了指定字段
        assert!(
            result.contains("approval_policy = \"untrusted\""),
            "{result}"
        );
        assert!(
            result.contains("model_auto_compact_token_limit = 200000"),
            "{result}"
        );
        assert!(
            result.contains("disable_response_storage = true"),
            "{result}"
        );
        // 完整保留嵌套结构
        assert!(result.contains("[model_providers.openai]"), "{result}");
        assert!(
            result.contains("base_url = \"https://api.com\""),
            "{result}"
        );
        assert!(result.contains("[mcp_servers.fs]"), "{result}");
        assert!(result.contains("command = \"npx\""), "{result}");
        assert!(result.contains("model_provider = \"openai\""), "{result}");
    }

    #[test]
    fn test_mixed_set_and_remove() {
        let live = "personality = \"friendly\"\nenable_workflows = true\n";
        let result = apply_top_level_updates(
            live,
            &updates(json!({
                "personality": null,
                "model_reasoning_effort": "high",
                "enable_workflows": false,
            })),
        )
        .unwrap();

        assert!(!result.contains("personality"), "{result}");
        assert!(
            result.contains("model_reasoning_effort = \"high\""),
            "{result}"
        );
        assert!(result.contains("enable_workflows = false"), "{result}");
    }

    /// 回归：旧实现（JSON 往返 + 全量序列化）会洗掉注释与键序，新实现必须保住。
    #[test]
    fn test_preserves_comments_and_key_order() {
        let live = "\
# 我的 Codex 配置
model = \"gpt-5\"     # 行尾注释
approval_policy = \"never\"

# 下面是中转配置
[model_providers.custom]
base_url = \"https://x.example\"
";
        let result =
            apply_top_level_updates(live, &updates(json!({ "approval_policy": "on-request" })))
                .unwrap();

        assert!(
            result.contains("# 我的 Codex 配置"),
            "顶层注释应保留:\n{result}"
        );
        assert!(result.contains("# 行尾注释"), "行尾注释应保留:\n{result}");
        assert!(
            result.contains("# 下面是中转配置"),
            "子表前注释应保留:\n{result}"
        );
        assert!(
            result.contains("approval_policy = \"on-request\""),
            "{result}"
        );

        let model_pos = result.find("model =").expect("model 应存在");
        let policy_pos = result
            .find("approval_policy")
            .expect("approval_policy 应存在");
        assert!(model_pos < policy_pos, "键序应保留:\n{result}");
    }
}
