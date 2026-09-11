//! 适配器共用的凭据剥离与回填。
//!
//! 各工具的 API 凭据散落在各自的配置结构里
//! （`provider.*.options.apiKey`、`models.providers.*.apiKey`、
//! `custom_providers[].api_key` 等），但处理契约完全一致：
//!
//! - 入库前**剥离**凭据：`shared_configs` 表只保存非凭据部分，key 只落在
//!   `api_profiles` 表；
//! - 切换写盘前**回填**：把磁盘上其他 provider 的凭据补回已剥离的 shared，
//!   避免切换一个 provider 就把其他 provider 的 key 洗掉；当前 provider 的
//!   凭据随后由 `merge_config` 用 profile 的值覆盖。
//!
//! 差异只在「容器位置」与「凭据键名」，因此统一用
//! `容器路径 + 键路径` 参数化，避免每个适配器各写一份同构实现。
//!
//! 路径按 JSON 层级书写：`&["provider"]`、`&["options", "apiKey"]`。
//! 键路径的最后一段是凭据键名，前面几段是它在条目内的父级路径。

use serde_json::{Map, Value};

/// 沿 `path` 取不可变引用；任一层缺失返回 `None`。
fn path_value<'a>(root: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = root;
    for segment in path {
        current = current.get(*segment)?;
    }
    Some(current)
}

/// 沿 `path` 取可变引用；任一层缺失返回 `None`。
fn path_value_mut<'a>(root: &'a mut Value, path: &[&str]) -> Option<&'a mut Value> {
    let mut current = root;
    for segment in path {
        current = current.get_mut(*segment)?;
    }
    Some(current)
}

/// 定位条目上承载凭据的对象，并给出凭据键名（`key_path` 的最后一段）。
///
/// 键路径只有一段时（如 `&["apiKey"]`）返回条目自身；父级不存在或不是对象时
/// 返回 `None`，调用方据此跳过该条目。
fn credential_slot<'entry, 'key>(
    entry: &'entry mut Value,
    key_path: &[&'key str],
) -> Option<(&'entry mut Map<String, Value>, &'key str)> {
    let (key, parent_path) = key_path.split_last()?;
    let slot = path_value_mut(entry, parent_path)?.as_object_mut()?;
    Some((slot, key))
}

/// 剥离 `map_path` 指向的 provider 映射里所有条目的凭据。
///
/// 适用于 `{"provider": {"<id>": {...}}}` 这类以 provider id 为键的映射。
pub fn strip_credential_map(config: &mut Value, map_path: &[&str], key_path: &[&str]) {
    let Some(entries) = path_value_mut(config, map_path).and_then(Value::as_object_mut) else {
        return;
    };
    for entry in entries.values_mut() {
        if let Some((slot, key)) = credential_slot(entry, key_path) {
            slot.remove(key);
        }
    }
}

/// 剥离 `array_path` 指向的数组里所有条目的凭据。
///
/// 适用于 `{"custom_providers": [{...}]}` 这类以数组承载 provider 的结构。
pub fn strip_credential_array(config: &mut Value, array_path: &[&str], key_path: &[&str]) {
    let Some(entries) = path_value_mut(config, array_path).and_then(Value::as_array_mut) else {
        return;
    };
    for entry in entries.iter_mut() {
        if let Some((slot, key)) = credential_slot(entry, key_path) {
            slot.remove(key);
        }
    }
}

/// 把 `disk` 中其他 provider 的凭据补回 `shared`（`shared` 已剥离凭据）。
///
/// 只补 `shared` 缺失的凭据，`shared` 已有的以 `shared` 为准（磁盘优先）。
/// `case_insensitive` 用于容忍 `OpenAI` / `openai` 这类大小写差异。
/// 凭据值按原样回填，不额外做类型判断。
pub fn backfill_credential_map(
    shared: &mut Value,
    disk: &Value,
    map_path: &[&str],
    key_path: &[&str],
    case_insensitive: bool,
) {
    let Some(disk_entries) = path_value(disk, map_path).and_then(Value::as_object) else {
        return;
    };
    // 先收集磁盘凭据，避免与 shared 的可变借用冲突。
    let disk_credentials: Vec<(String, Value)> = disk_entries
        .iter()
        .filter_map(|(id, entry)| {
            let value = path_value(entry, key_path)?;
            let id = if case_insensitive {
                id.to_lowercase()
            } else {
                id.clone()
            };
            Some((id, value.clone()))
        })
        .collect();

    let Some(shared_entries) = path_value_mut(shared, map_path).and_then(Value::as_object_mut)
    else {
        return;
    };
    for (id, entry) in shared_entries.iter_mut() {
        let Some((slot, key)) = credential_slot(entry, key_path) else {
            continue;
        };
        if slot.contains_key(key) {
            continue;
        }
        let needle = if case_insensitive {
            id.to_lowercase()
        } else {
            id.clone()
        };
        if let Some((_, value)) = disk_credentials
            .iter()
            .find(|(disk_id, _)| *disk_id == needle)
        {
            slot.insert(key.to_string(), value.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strip_map_removes_nested_credential_only() {
        let mut config = json!({
            "provider": {
                "openai": { "options": { "apiKey": "sk-a", "baseURL": "https://x" } },
                "anthropic": { "options": {} }
            }
        });
        strip_credential_map(&mut config, &["provider"], &["options", "apiKey"]);
        assert!(config["provider"]["openai"]["options"]
            .get("apiKey")
            .is_none());
        assert_eq!(
            config["provider"]["openai"]["options"]["baseURL"],
            "https://x"
        );
    }

    #[test]
    fn strip_map_skips_entries_without_credential_parent() {
        let mut config = json!({ "provider": { "p": { "other": 1 } } });
        strip_credential_map(&mut config, &["provider"], &["options", "apiKey"]);
        assert_eq!(config["provider"]["p"]["other"], 1);
    }

    #[test]
    fn strip_map_tolerates_missing_container() {
        let mut config = json!({ "other": 1 });
        strip_credential_map(&mut config, &["provider"], &["options", "apiKey"]);
        assert_eq!(config, json!({ "other": 1 }));
    }

    #[test]
    fn strip_array_removes_credentials() {
        let mut config = json!({
            "custom_providers": [
                { "name": "a", "api_key": "k1" },
                { "name": "b" }
            ]
        });
        strip_credential_array(&mut config, &["custom_providers"], &["api_key"]);
        assert!(config["custom_providers"][0].get("api_key").is_none());
        assert_eq!(config["custom_providers"][0]["name"], "a");
    }

    #[test]
    fn backfill_restores_only_missing_credentials() {
        let mut shared = json!({
            "provider": {
                "openai": { "options": {} },
                "anthropic": { "options": { "apiKey": "shared-wins" } }
            }
        });
        let disk = json!({
            "provider": {
                "openai": { "options": { "apiKey": "sk-openai" } },
                "anthropic": { "options": { "apiKey": "sk-anthropic" } }
            }
        });
        backfill_credential_map(
            &mut shared,
            &disk,
            &["provider"],
            &["options", "apiKey"],
            false,
        );
        assert_eq!(
            shared["provider"]["openai"]["options"]["apiKey"],
            "sk-openai"
        );
        assert_eq!(
            shared["provider"]["anthropic"]["options"]["apiKey"],
            "shared-wins"
        );
    }

    #[test]
    fn backfill_matches_ids_case_insensitively_when_requested() {
        let mut shared = json!({ "provider": { "OpenAI": { "options": {} } } });
        let disk = json!({ "provider": { "openai": { "options": { "apiKey": "sk-a" } } } });
        backfill_credential_map(
            &mut shared,
            &disk,
            &["provider"],
            &["options", "apiKey"],
            true,
        );
        assert_eq!(shared["provider"]["OpenAI"]["options"]["apiKey"], "sk-a");
    }

    #[test]
    fn backfill_keeps_ids_apart_when_case_sensitive() {
        let mut shared = json!({ "provider": { "OpenAI": { "options": {} } } });
        let disk = json!({ "provider": { "openai": { "options": { "apiKey": "sk-a" } } } });
        backfill_credential_map(
            &mut shared,
            &disk,
            &["provider"],
            &["options", "apiKey"],
            false,
        );
        assert!(shared["provider"]["OpenAI"]["options"]
            .get("apiKey")
            .is_none());
    }

    #[test]
    fn backfill_supports_multi_level_container_path() {
        let mut shared = json!({ "models": { "providers": { "p": {} } } });
        let disk = json!({ "models": { "providers": { "p": { "apiKey": "sk-p" } } } });
        backfill_credential_map(
            &mut shared,
            &disk,
            &["models", "providers"],
            &["apiKey"],
            false,
        );
        assert_eq!(shared["models"]["providers"]["p"]["apiKey"], "sk-p");
    }

    #[test]
    fn backfill_is_noop_without_disk_credentials() {
        let mut shared = json!({ "provider": { "openai": { "options": {} } } });
        backfill_credential_map(
            &mut shared,
            &json!({}),
            &["provider"],
            &["options", "apiKey"],
            false,
        );
        assert!(shared["provider"]["openai"]["options"]
            .get("apiKey")
            .is_none());
    }

    #[test]
    fn backfill_is_noop_when_shared_container_missing() {
        let mut shared = json!({ "other": 1 });
        let disk = json!({ "provider": { "openai": { "options": { "apiKey": "sk-a" } } } });
        backfill_credential_map(
            &mut shared,
            &disk,
            &["provider"],
            &["options", "apiKey"],
            false,
        );
        assert_eq!(shared, json!({ "other": 1 }));
    }
}
