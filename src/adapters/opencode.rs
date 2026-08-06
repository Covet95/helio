use super::{backup, ConfigAdapter};
use crate::models::ApiProfile;
use crate::utils::secure_fs::atomic_write_private;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

pub struct OpenCodeAdapter {
    config_dir: PathBuf,
}

impl OpenCodeAdapter {
    pub fn new() -> Self {
        let home = dirs::home_dir().expect("Failed to get home directory");
        let config_dir = home.join(".config").join("opencode");
        Self { config_dir }
    }

    fn config_file_path(&self) -> PathBuf {
        self.config_dir.join("opencode.json")
    }

    /// 去除 JSONC 注释（// 行注释和 /* */ 块注释），简单实现。
    /// 不处理字符串内的 // 等边界情况，对标准 opencode.json 足够。
    fn strip_jsonc_comments(input: &str) -> String {
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
                    // 行注释：跳到行尾
                    chars.next();
                    for nc in chars.by_ref() {
                        if nc == '\n' {
                            out.push('\n');
                            break;
                        }
                    }
                }
                '/' if chars.peek() == Some(&'*') => {
                    // 块注释：跳到 */
                    chars.next();
                    let mut prev = '\0';
                    for nc in chars.by_ref() {
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

    /// 从 profile.provider 推导 OpenCode provider id（小写）
    fn provider_id(api_profile: &ApiProfile) -> String {
        Self::normalize_provider_id(&api_profile.provider)
    }

    /// provider 名 → OpenCode 配置里的 id（空 → "custom"，其余小写）
    pub fn normalize_provider_id(provider: &str) -> String {
        if provider.is_empty() {
            "custom".to_string()
        } else {
            provider.to_lowercase()
        }
    }

    /// 从配置 JSON 中移除指定 provider；若顶层 model/small_model 指向它也一并清掉。
    /// 不写盘，纯函数便于单测。
    pub fn remove_provider_from_config(
        config: &serde_json::Value,
        provider_id: &str,
    ) -> serde_json::Value {
        let pid = Self::normalize_provider_id(provider_id);
        let mut config = config.clone();

        if let Some(providers) = config.get_mut("provider").and_then(|v| v.as_object_mut()) {
            providers.remove(&pid);
        }

        // model / small_model 格式为 provider/model；指向被删 id 时清掉，避免脏引用
        for key in ["model", "small_model"] {
            let should_clear = config
                .get(key)
                .and_then(|v| v.as_str())
                .and_then(|m| m.split_once('/'))
                .map(|(p, _)| p.eq_ignore_ascii_case(&pid))
                .unwrap_or(false);
            if should_clear {
                if let Some(obj) = config.as_object_mut() {
                    obj.remove(key);
                }
            }
        }

        config
    }

    /// 剩余档案里是否还有 OpenCode 工具、且 provider id 相同的条目。
    /// 删除档案后若仍有人用同一 provider，就不该动本地 opencode.json。
    pub fn provider_still_used(remaining: &[ApiProfile], provider_id: &str) -> bool {
        let pid = Self::normalize_provider_id(provider_id);
        remaining.iter().any(|p| {
            p.target_app == Some(crate::models::TargetApp::OpenCode)
                && Self::normalize_provider_id(&p.provider) == pid
        })
    }

    /// 读盘 → 移除 provider → 备份 → 写回。目标不存在视为成功。
    pub fn remove_provider(&self, provider_id: &str) -> Result<()> {
        let pid = Self::normalize_provider_id(provider_id);
        let config = self.read_config()?;
        let present = config
            .get("provider")
            .and_then(|p| p.as_object())
            .map(|m| m.contains_key(&pid))
            .unwrap_or(false);
        if !present {
            return Ok(());
        }
        if self.config_path().exists() {
            // 备份失败必须中止删除,否则配置被改写后无恢复手段。
            self.backup_config()
                .with_context(|| "Failed to back up opencode config before removal")?;
        }
        let next = Self::remove_provider_from_config(&config, &pid);
        self.write_config(&next)
    }

    /// 删除 OpenCode 档案的统一入口：先确保不再使用的本地 provider 可清理，
    /// 再删除 DB 档案，避免清理失败后留下无法重试的档案状态。
    /// CLI / GUI 共用，避免删前取 provider 的逻辑分叉。
    ///
    /// 返回值：档案是否存在并已删除。
    pub fn delete_profile_and_cleanup_local(db: &crate::db::Database, name: &str) -> Result<bool> {
        let profiles = db.list_profiles()?;
        let Some(profile) = profiles
            .into_iter()
            .find(|p| p.target_app == Some(crate::models::TargetApp::OpenCode) && p.name == name)
        else {
            return Ok(false);
        };
        let provider = profile.provider;
        let provider_still_used = db.list_profiles()?.into_iter().any(|p| {
            p.id != profile.id
                && p.target_app == Some(crate::models::TargetApp::OpenCode)
                && Self::normalize_provider_id(&p.provider)
                    == Self::normalize_provider_id(&provider)
        });

        if !provider_still_used {
            Self::new().remove_provider(&provider)?;
        }

        let deleted = db.delete_profile(name, crate::models::TargetApp::OpenCode)?;
        if deleted && !provider_still_used {
            db.clear_opencode_managed_provider(&provider)?;
        }
        Ok(deleted)
    }

    /// 规范化 OpenCode openai-compatible 的 baseURL。
    ///
    /// OpenCode 默认 `npm = @ai-sdk/openai-compatible` 会把路径拼成
    /// `{baseURL}/chat/completions`。若 base 只有域名（如 Hermes 常见的
    /// `https://host`），就会打到站点 HTML 而不是 API。
    /// 因此：去掉尾斜杠后，若尚未是版本根（`/v1` 或 `/paas/v4`）则补 `/v1`。
    /// 已带 `/v1` 的保持原样，避免出现 `/v1/v1`。
    fn normalize_openai_compatible_base_url(api_url: &str) -> String {
        let base = api_url.trim().trim_end_matches('/');
        if base.is_empty() {
            return String::new();
        }
        if base.ends_with("/v1") || base.ends_with("/paas/v4") {
            return base.to_string();
        }
        format!("{base}/v1")
    }

    pub fn normalize_api_mode(mode: Option<&str>) -> Result<Option<&'static str>> {
        match mode.map(str::trim).filter(|m| !m.is_empty()) {
            None => Ok(None),
            Some("chat_completions") => Ok(Some("chat_completions")),
            Some("responses") => Ok(Some("responses")),
            Some(other) => anyhow::bail!(
                "OpenCode api mode must be chat_completions or responses, got `{other}`"
            ),
        }
    }

    fn npm_for_api_mode(mode: &str) -> &'static str {
        match mode {
            "responses" => "@ai-sdk/openai",
            _ => "@ai-sdk/openai-compatible",
        }
    }

    /// Resolve the complete model set managed by the profile.
    ///
    /// The legacy list remains readable, while the default model and
    /// model-config keys are included so a config-only model is not omitted.
    pub fn resolve_model_ids(api_profile: &ApiProfile) -> Vec<String> {
        let mut model_ids = Vec::new();
        let mut push = |model: &str| {
            let model = model.trim();
            if !model.is_empty() && !model_ids.iter().any(|m| m == model) {
                model_ids.push(model.to_string());
            }
        };

        if let Some(list) = api_profile.opencode.models.as_ref() {
            for model in list {
                push(model);
            }
        }
        if let Some(model) = api_profile.model.as_deref() {
            push(model);
        }
        if let Some(configs) = api_profile.opencode.model_configs.as_ref() {
            let mut config_ids: Vec<&String> = configs.keys().collect();
            config_ids.sort();
            for model in config_ids {
                push(model);
            }
        }
        model_ids
    }

    /// Remove only models previously written by Helio for this provider.
    pub fn prepare_shared_config_for_switch(
        shared_config: &serde_json::Value,
        api_profile: &ApiProfile,
        previous_state: &HashMap<String, Vec<String>>,
    ) -> (serde_json::Value, HashMap<String, Vec<String>>) {
        let provider_id = Self::provider_id(api_profile);
        let desired = Self::resolve_model_ids(api_profile);
        let mut config = shared_config.clone();

        if let Some(previous_ids) = previous_state.get(&provider_id) {
            if let Some(models) = config
                .get_mut("provider")
                .and_then(|providers| providers.as_object_mut())
                .and_then(|providers| providers.get_mut(&provider_id))
                .and_then(|provider| provider.get_mut("models"))
                .and_then(|models| models.as_object_mut())
            {
                for model_id in previous_ids {
                    if !desired.iter().any(|model| model == model_id) {
                        models.remove(model_id);
                    }
                }
            }
        }

        let mut next_state = previous_state.clone();
        if desired.is_empty() {
            next_state.remove(&provider_id);
        } else {
            next_state.insert(provider_id, desired);
        }
        (config, next_state)
    }

    fn merge_json_objects(
        target: &mut serde_json::Map<String, serde_json::Value>,
        patch: &serde_json::Map<String, serde_json::Value>,
    ) {
        for (key, value) in patch {
            match (target.get_mut(key), value) {
                (Some(serde_json::Value::Object(existing)), serde_json::Value::Object(next)) => {
                    Self::merge_json_objects(existing, next);
                }
                _ => {
                    target.insert(key.clone(), value.clone());
                }
            }
        }
    }
}

impl Default for OpenCodeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

/// 剥离所有 provider 的 options.apiKey。
fn strip_credentials(config: &mut serde_json::Value) {
    if let Some(providers) = config.get_mut("provider").and_then(|v| v.as_object_mut()) {
        for p in providers.values_mut() {
            if let Some(options) = p.get_mut("options").and_then(|v| v.as_object_mut()) {
                options.remove("apiKey");
            }
        }
    }
}

/// 把磁盘配置中其他 provider 的 key 补回 shared（shared 已剥离）。
/// 当前 provider 的 key 随后会被 merge 用 profile 的值覆盖。
fn restore_credentials(config: &mut serde_json::Value, disk: &serde_json::Value) {
    let Some(shared_providers) = config.get_mut("provider").and_then(|v| v.as_object_mut()) else {
        return;
    };
    let Some(disk_providers) = disk.get("provider").and_then(|v| v.as_object()) else {
        return;
    };
    for (id, shared_p) in shared_providers.iter_mut() {
        let Some(disk_p) = disk_providers.get(id) else {
            continue;
        };
        let Some(shared_options) = shared_p.get_mut("options").and_then(|v| v.as_object_mut())
        else {
            continue;
        };
        if shared_options.contains_key("apiKey") {
            continue;
        }
        if let Some(key) = disk_p.pointer("/options/apiKey") {
            shared_options.insert("apiKey".into(), key.clone());
        }
    }
}

impl ConfigAdapter for OpenCodeAdapter {
    fn config_path(&self) -> PathBuf {
        self.config_file_path()
    }

    fn read_config(&self) -> Result<serde_json::Value> {
        let path = self.config_path();

        if !path.exists() {
            return Ok(serde_json::json!({}));
        }

        let content = fs::read_to_string(&path).context("Failed to read OpenCode config")?;
        let stripped = Self::strip_jsonc_comments(&content);
        serde_json::from_str(&stripped).context("Failed to parse OpenCode config")
    }

    fn validate_profile(&self, api_profile: &ApiProfile) -> Result<()> {
        Self::normalize_api_mode(api_profile.opencode.opencode_api_mode.as_deref())?;
        Ok(())
    }

    fn extract_shared_config(&self, config: &serde_json::Value) -> serde_json::Value {
        // 凭据不落 shared_configs（key 只存 api_profiles 表，避免明文冗余 + 回传前端）。
        // merge_config 时会从磁盘补回其他 provider 的 key，共存语义不受影响。
        let mut shared = config.clone();
        strip_credentials(&mut shared);
        shared
    }

    fn merge_config(
        &self,
        api_profile: &ApiProfile,
        shared_config: &serde_json::Value,
    ) -> serde_json::Value {
        let mut config = shared_config.clone();
        // shared 已剥离凭据：从磁盘补回其他 provider 的 key（多 provider 共存）
        if let Ok(disk) = self.read_config() {
            restore_credentials(&mut config, &disk);
        }
        let provider_id = Self::provider_id(api_profile);

        if config.get("provider").is_none() {
            config["provider"] = serde_json::json!({});
        }

        if let Some(providers) = config.get_mut("provider").and_then(|v| v.as_object_mut()) {
            let is_new = !providers.contains_key(&provider_id);
            let entry = providers
                .entry(provider_id.clone())
                .or_insert_with(|| serde_json::json!({}));
            if let Some(p) = entry.as_object_mut() {
                // 全新 provider：补上 OpenCode 加载所必需的 npm 适配器与显示名。
                // 缺 npm 时 OpenCode 无法加载该 provider，切换会失效。
                // 已有 provider 的 npm / name / models 保留不动，只更新凭据。
                if is_new {
                    p.entry("npm".to_string()).or_insert_with(|| {
                        serde_json::Value::String("@ai-sdk/openai-compatible".to_string())
                    });
                    p.entry("name".to_string())
                        .or_insert_with(|| serde_json::Value::String(provider_id.clone()));
                }
                if let Ok(Some(mode)) =
                    Self::normalize_api_mode(api_profile.opencode.opencode_api_mode.as_deref())
                {
                    p.insert(
                        "npm".to_string(),
                        serde_json::Value::String(Self::npm_for_api_mode(mode).to_string()),
                    );
                }
                let options = p
                    .entry("options".to_string())
                    .or_insert_with(|| serde_json::json!({}));
                if let Some(opt) = options.as_object_mut() {
                    opt.insert(
                        "apiKey".to_string(),
                        serde_json::Value::String(api_profile.api_key.clone()),
                    );
                    opt.insert(
                        "baseURL".to_string(),
                        serde_json::Value::String(Self::normalize_openai_compatible_base_url(
                            &api_profile.api_url,
                        )),
                    );
                }

                // 模型列表：把 profile.models、默认模型和模型配置里的每个模型写进
                // provider 的 models 声明，加上 per-model OpenCode 配置。
                let model_ids = Self::resolve_model_ids(api_profile);
                if !model_ids.is_empty() {
                    let models = p
                        .entry("models".to_string())
                        .or_insert_with(|| serde_json::json!({}));
                    if let Some(models_obj) = models.as_object_mut() {
                        for m in &model_ids {
                            let model = models_obj
                                .entry(m.clone())
                                .or_insert_with(|| serde_json::json!({ "name": m }));
                            if let Some(model_obj) = model.as_object_mut() {
                                model_obj
                                    .entry("name".to_string())
                                    .or_insert_with(|| serde_json::Value::String(m.clone()));
                                if let Some(model_config) = api_profile
                                    .opencode
                                    .model_configs
                                    .as_ref()
                                    .and_then(|configs| configs.get(m))
                                    .and_then(|config| config.as_object())
                                {
                                    Self::merge_json_objects(model_obj, model_config);
                                }
                            }
                        }
                    }
                }
            }
        }

        // 顶层 model 指定默认模型，格式 provider/model（OpenCode 要求）。
        // 规则：① 有 model → provider/model；② 无 model 有 models[0] → provider/models[0]；
        // ③ 都没有 → 删掉顶层 model（不留指向旧/失效 provider 的脏值，让 OpenCode 用内置默认）。
        let default_model = api_profile
            .model
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .map(|m| m.to_string())
            .or_else(|| Self::resolve_model_ids(api_profile).into_iter().next());

        match default_model {
            Some(model) => {
                config["model"] = serde_json::Value::String(format!("{provider_id}/{model}"));
            }
            None => {
                // 没有可指定的模型：移除顶层 model，避免指向已切走/失效的 provider
                if let Some(obj) = config.as_object_mut() {
                    obj.remove("model");
                }
            }
        }

        config
    }

    fn write_config(&self, config: &serde_json::Value) -> Result<()> {
        let path = self.config_path();

        let content =
            serde_json::to_string_pretty(config).context("Failed to serialize OpenCode config")?;
        atomic_write_private(&path, content.as_bytes())
            .context("Failed to write OpenCode config")?;

        Ok(())
    }

    fn backup_config(&self) -> Result<PathBuf> {
        let path = self.config_path();
        if !path.exists() {
            anyhow::bail!("Config file does not exist");
        }

        let backup_path = backup::backup_required(&self.config_dir, &path, "opencode")?;

        self.cleanup_old_backups(10)?;

        Ok(backup_path)
    }

    fn cleanup_old_backups(&self, keep: usize) -> Result<()> {
        backup::cleanup_prefix(&self.config_dir, "opencode.backup.", keep)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::OpenCodeProfileFields;
    use std::collections::HashMap;

    fn sample_profile() -> ApiProfile {
        ApiProfile {
            id: Some(1),
            name: "test".to_string(),
            provider: "anthropic".to_string(),
            api_url: "https://api.example.com/v1".to_string(),
            api_key: "sk-test-key".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn test_normalize_openai_compatible_base_url() {
        assert_eq!(
            OpenCodeAdapter::normalize_openai_compatible_base_url("https://api.astrdark.cyou"),
            "https://api.astrdark.cyou/v1"
        );
        assert_eq!(
            OpenCodeAdapter::normalize_openai_compatible_base_url("https://api.astrdark.cyou/"),
            "https://api.astrdark.cyou/v1"
        );
        assert_eq!(
            OpenCodeAdapter::normalize_openai_compatible_base_url("https://api.astrdark.cyou/v1"),
            "https://api.astrdark.cyou/v1"
        );
        assert_eq!(
            OpenCodeAdapter::normalize_openai_compatible_base_url("https://api.astrdark.cyou/v1/"),
            "https://api.astrdark.cyou/v1"
        );
        assert_eq!(
            OpenCodeAdapter::normalize_openai_compatible_base_url("http://127.0.0.1:8317/v1"),
            "http://127.0.0.1:8317/v1"
        );
        assert_eq!(
            OpenCodeAdapter::normalize_openai_compatible_base_url(
                "https://dashscope.aliyuncs.com/compatible-mode/v1"
            ),
            "https://dashscope.aliyuncs.com/compatible-mode/v1"
        );
        assert_eq!(
            OpenCodeAdapter::normalize_openai_compatible_base_url(
                "https://dashscope.aliyuncs.com/api/v1/services/aigc/text-generation/generation/paas/v4"
            ),
            "https://dashscope.aliyuncs.com/api/v1/services/aigc/text-generation/generation/paas/v4"
        );
        assert_eq!(
            OpenCodeAdapter::normalize_openai_compatible_base_url("  https://host.example  "),
            "https://host.example/v1"
        );
        assert_eq!(
            OpenCodeAdapter::normalize_openai_compatible_base_url(""),
            ""
        );
        assert_eq!(
            OpenCodeAdapter::normalize_openai_compatible_base_url("   "),
            ""
        );
    }

    #[test]
    fn test_protocol_mode_maps_to_provider_npm() {
        let adapter = OpenCodeAdapter::new();
        let profile = ApiProfile {
            provider: "openai".into(),
            opencode: OpenCodeProfileFields {
                opencode_api_mode: Some("responses".into()),
                ..Default::default()
            },
            ..sample_profile()
        };
        let merged = adapter.merge_config(&profile, &serde_json::json!({}));
        assert_eq!(merged["provider"]["openai"]["npm"], "@ai-sdk/openai");

        let existing = serde_json::json!({
            "provider": {
                "openai": {
                    "npm": "@ai-sdk/openai-compatible",
                    "options": {}
                }
            }
        });
        let changed = adapter.merge_config(&profile, &existing);
        assert_eq!(changed["provider"]["openai"]["npm"], "@ai-sdk/openai");

        let invalid = ApiProfile {
            opencode: OpenCodeProfileFields {
                opencode_api_mode: Some("invalid".into()),
                ..Default::default()
            },
            ..sample_profile()
        };
        assert!(adapter.validate_profile(&invalid).is_err());
    }

    #[test]
    fn test_merge_writes_model_config_and_preserves_unknown_fields() {
        let adapter = OpenCodeAdapter::new();
        let profile = ApiProfile {
            provider: "openai".into(),
            model: Some("gpt-5".into()),
            opencode: OpenCodeProfileFields {
                model_configs: Some(HashMap::from([(
                    "gpt-5".into(),
                    serde_json::json!({
                        "limit": { "context": 200000, "output": 65536 },
                        "options": { "reasoningEffort": "high" },
                        "variants": {
                            "low": { "reasoningEffort": "low" },
                            "max": { "reasoningEffort": "xhigh" }
                        }
                    }),
                )])),
                ..Default::default()
            },
            ..sample_profile()
        };
        let shared = serde_json::json!({
            "provider": {
                "openai": {
                    "models": {
                        "gpt-5": {
                            "customField": "keep",
                            "options": { "existing": true }
                        }
                    }
                }
            }
        });
        let merged = adapter.merge_config(&profile, &shared);
        let model = &merged["provider"]["openai"]["models"]["gpt-5"];
        assert_eq!(model["customField"], "keep");
        assert_eq!(model["limit"]["output"], 65536);
        assert_eq!(model["options"]["existing"], true);
        assert_eq!(model["options"]["reasoningEffort"], "high");
        assert_eq!(model["variants"]["low"]["reasoningEffort"], "low");
        assert_eq!(model["variants"]["max"]["reasoningEffort"], "xhigh");
    }

    #[test]
    fn test_reconcile_removes_only_previous_helio_models() {
        let profile = ApiProfile {
            provider: "cpa".into(),
            model: Some("new".into()),
            opencode: OpenCodeProfileFields {
                models: Some(vec!["new".into()]),
                ..Default::default()
            },
            ..sample_profile()
        };
        let shared = serde_json::json!({
            "provider": {
                "cpa": {
                    "models": {
                        "old": { "name": "old" },
                        "manual": { "name": "manual", "custom": true }
                    }
                },
                "other": {
                    "models": {
                        "keep": { "name": "keep" }
                    }
                }
            }
        });
        let previous = HashMap::from([
            ("cpa".into(), vec!["old".into()]),
            ("other".into(), vec!["keep".into()]),
        ]);
        let (pruned, next) =
            OpenCodeAdapter::prepare_shared_config_for_switch(&shared, &profile, &previous);
        assert!(pruned["provider"]["cpa"]["models"].get("old").is_none());
        assert!(pruned["provider"]["cpa"]["models"].get("manual").is_some());
        assert!(pruned["provider"]["other"]["models"].get("keep").is_some());
        assert_eq!(next["cpa"], vec!["new"]);
        assert_eq!(next["other"], vec!["keep"]);
    }

    #[test]
    fn test_reconcile_empty_set_removes_managed_only() {
        let profile = ApiProfile {
            provider: "cpa".into(),
            ..sample_profile()
        };
        let shared = serde_json::json!({
            "provider": {
                "cpa": {
                    "models": {
                        "old": { "name": "old" },
                        "manual": { "name": "manual" }
                    }
                }
            },
            "model": "cpa/old"
        });
        let previous = HashMap::from([("cpa".into(), vec!["old".into()])]);
        let (pruned, next) =
            OpenCodeAdapter::prepare_shared_config_for_switch(&shared, &profile, &previous);

        assert!(pruned["provider"]["cpa"]["models"].get("old").is_none());
        assert!(pruned["provider"]["cpa"]["models"].get("manual").is_some());
        assert!(!next.contains_key("cpa"));
    }

    #[test]
    fn test_merge_appends_v1_when_api_url_has_no_version_root() {
        // 与 Hermes 习惯对齐：用户只填域名时，OpenCode 写入须补 /v1，
        // 否则 @ai-sdk/openai-compatible 会打到 /chat/completions 站点 HTML。
        let adapter = OpenCodeAdapter::new();
        let profile = ApiProfile {
            api_url: "https://api.astrdark.cyou".to_string(),
            provider: "openai".to_string(),
            model: Some("grok-4.5".to_string()),
            ..sample_profile()
        };
        let merged = adapter.merge_config(&profile, &serde_json::json!({}));
        assert_eq!(
            merged["provider"]["openai"]["options"]["baseURL"],
            "https://api.astrdark.cyou/v1"
        );
        assert_eq!(merged["model"], "openai/grok-4.5");
    }

    #[test]
    fn test_merge_does_not_double_append_v1() {
        let adapter = OpenCodeAdapter::new();
        let profile = ApiProfile {
            api_url: "https://api.astrdark.cyou/v1".to_string(),
            provider: "openai".to_string(),
            ..sample_profile()
        };
        let merged = adapter.merge_config(&profile, &serde_json::json!({}));
        assert_eq!(
            merged["provider"]["openai"]["options"]["baseURL"],
            "https://api.astrdark.cyou/v1"
        );
    }

    #[test]
    fn test_strip_jsonc_comments() {
        let input = r#"{
  // line comment
  "model": "x", /* block */
  "url": "http://a//b"
}"#;
        let out = OpenCodeAdapter::strip_jsonc_comments(input);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["model"], "x");
        // 字符串内的 // 被保留
        assert_eq!(parsed["url"], "http://a//b");
    }

    #[test]
    fn test_extract_shared_strips_credentials() {
        // key 不落 shared_configs；其他配置原样保留。
        let adapter = OpenCodeAdapter::new();
        let config = serde_json::json!({
            "provider": {
                "anthropic": {
                    "name": "Anthropic",
                    "options": {
                        "apiKey": "sk-secret",
                        "baseURL": "https://api.com",
                        "timeout": 30000
                    }
                }
            },
            "mcp": { "fs": { "type": "local" } },
            "permission": { "edit": "ask" }
        });

        let shared = adapter.extract_shared_config(&config);

        // apiKey 被剥离
        assert!(
            shared["provider"]["anthropic"]["options"]
                .get("apiKey")
                .is_none(),
            "apiKey 不应进 shared config"
        );
        // 其余配置原样保留
        assert_eq!(
            shared["provider"]["anthropic"]["options"]["baseURL"],
            "https://api.com"
        );
        assert_eq!(shared["provider"]["anthropic"]["options"]["timeout"], 30000);
        assert_eq!(shared["provider"]["anthropic"]["name"], "Anthropic");
        assert_eq!(shared["mcp"]["fs"]["type"], "local");
        assert_eq!(shared["permission"]["edit"], "ask");
    }

    #[test]
    fn test_restore_credentials_from_disk() {
        // merge 时从磁盘补回其他 provider 的 key（共存可用），当前 provider 被 profile 覆盖
        let config = serde_json::json!({
            "provider": {
                "cpa": {
                    "name": "cpa",
                    "options": { "apiKey": "sk-disk" }
                }
            }
        });
        let shared = serde_json::json!({
            "provider": {
                "cpa": { "name": "cpa", "options": { "baseURL": "http://127.0.0.1:8317/v1" } }
            }
        });
        let mut merged = shared.clone();
        restore_credentials(&mut merged, &config);
        assert_eq!(merged["provider"]["cpa"]["options"]["apiKey"], "sk-disk");
        // 已带 key 的不覆盖
        let mut has_key = shared.clone();
        has_key["provider"]["cpa"]["options"]["apiKey"] = serde_json::json!("sk-keep");
        restore_credentials(&mut has_key, &config);
        assert_eq!(has_key["provider"]["cpa"]["options"]["apiKey"], "sk-keep");
    }

    #[test]
    fn test_merge_coexist_preserves_other_provider_creds() {
        // 切换到 anthropic 时，已存在的 cpa provider 的凭据必须原样保留（共存可用）
        let adapter = OpenCodeAdapter::new();
        let shared = serde_json::json!({
            "provider": {
                "cpa": {
                    "npm": "@ai-sdk/openai-compatible",
                    "name": "cpa",
                    "models": { "claude-opus-4-8": { "name": "claude-opus-4-8" } },
                    "options": { "apiKey": "sk-cpa", "baseURL": "http://127.0.0.1:8317/v1" }
                }
            }
        });

        let merged = adapter.merge_config(&sample_profile(), &shared);

        // 新 provider anthropic 写入
        assert_eq!(
            merged["provider"]["anthropic"]["options"]["apiKey"],
            "sk-test-key"
        );
        // 旧 provider cpa 凭据原样保留，不被清空
        assert_eq!(merged["provider"]["cpa"]["options"]["apiKey"], "sk-cpa");
        assert_eq!(
            merged["provider"]["cpa"]["options"]["baseURL"],
            "http://127.0.0.1:8317/v1"
        );
    }

    #[test]
    fn test_merge_no_model_removes_stale_top_level_model() {
        // profile 无 model 也无 models：删掉顶层旧 model（用户选项：用 OpenCode 内置默认）
        let adapter = OpenCodeAdapter::new();
        let shared = serde_json::json!({ "model": "cpa/claude-opus-4-8" });
        // sample_profile 不带 model/models
        let merged = adapter.merge_config(&sample_profile(), &shared);
        assert!(
            merged.get("model").is_none(),
            "无模型时应删掉顶层旧 model，实得 {:?}",
            merged.get("model")
        );
    }

    #[test]
    fn test_merge_uses_first_models_when_no_default() {
        // 无 model 但有 models：顶层用 provider/models[0]
        let adapter = OpenCodeAdapter::new();
        let profile = ApiProfile {
            opencode: OpenCodeProfileFields {
                models: Some(vec![
                    "claude-sonnet-4-6".to_string(),
                    "claude-haiku-4-5".to_string(),
                ]),
                ..Default::default()
            },
            ..sample_profile()
        };
        let merged = adapter.merge_config(&profile, &serde_json::json!({}));
        assert_eq!(merged["model"], "anthropic/claude-sonnet-4-6");
    }

    #[test]
    fn test_merge_inserts_api() {
        let adapter = OpenCodeAdapter::new();
        let shared = serde_json::json!({
            "mcp": { "fs": { "type": "local" } },
            "permission": { "edit": "ask" }
        });

        let merged = adapter.merge_config(&sample_profile(), &shared);

        // API 写入 provider.anthropic.options
        assert_eq!(
            merged["provider"]["anthropic"]["options"]["apiKey"],
            "sk-test-key"
        );
        assert_eq!(
            merged["provider"]["anthropic"]["options"]["baseURL"],
            "https://api.example.com/v1"
        );
        // 全新 provider 必须补 npm（否则 OpenCode 加载不了该 provider）
        assert_eq!(
            merged["provider"]["anthropic"]["npm"],
            "@ai-sdk/openai-compatible"
        );
        // 补显示名（默认用 provider id）
        assert_eq!(merged["provider"]["anthropic"]["name"], "anthropic");
        // 共享配置保留
        assert_eq!(merged["mcp"]["fs"]["type"], "local");
        assert_eq!(merged["permission"]["edit"], "ask");
    }

    #[test]
    fn test_merge_mcp_only_from_shared_config() {
        // MCP 只来自共享配置,不读本机 ~/.claude.json(即使本机有 8 个 MCP 也不混入)
        let adapter = OpenCodeAdapter::new();
        let shared = serde_json::json!({
            "mcp": { "test-only-srv": { "type": "local", "command": ["x"] } }
        });

        let merged = adapter.merge_config(&sample_profile(), &shared);

        let mcp = merged["mcp"].as_object().unwrap();
        assert_eq!(mcp.len(), 1);
        assert_eq!(mcp["test-only-srv"]["command"], serde_json::json!(["x"]));
    }

    #[test]
    fn test_merge_writes_multiple_models() {
        let adapter = OpenCodeAdapter::new();
        let profile = ApiProfile {
            model: Some("claude-opus-4-8".to_string()),
            opencode: OpenCodeProfileFields {
                models: Some(vec![
                    "claude-sonnet-4-6".to_string(),
                    "claude-haiku-4-5".to_string(),
                ]),
                ..Default::default()
            },
            ..sample_profile()
        };
        let merged = adapter.merge_config(&profile, &serde_json::json!({}));

        let models = &merged["provider"]["anthropic"]["models"];
        // 多选的两个 + 默认模型，共 3 个都在 provider.models
        assert!(models["claude-sonnet-4-6"].is_object());
        assert!(models["claude-haiku-4-5"].is_object());
        assert!(
            models["claude-opus-4-8"].is_object(),
            "默认模型也应在 models 里"
        );
        // 顶层默认 model
        assert_eq!(merged["model"], "anthropic/claude-opus-4-8");
    }

    #[test]
    fn test_merge_writes_default_model() {
        let adapter = OpenCodeAdapter::new();
        let profile = ApiProfile {
            model: Some("claude-opus-4-8".to_string()),
            ..sample_profile()
        };
        let merged = adapter.merge_config(&profile, &serde_json::json!({}));

        // 顶层 model = provider/model
        assert_eq!(merged["model"], "anthropic/claude-opus-4-8");
        // provider 的 models 声明了该模型（否则 OpenCode 选不到）
        assert!(merged["provider"]["anthropic"]["models"]["claude-opus-4-8"].is_object());
    }

    #[test]
    fn test_merge_preserves_existing_provider_npm_and_models() {
        let adapter = OpenCodeAdapter::new();
        // 已有同名 provider，带自定义 npm / name / models
        let shared = serde_json::json!({
            "provider": {
                "anthropic": {
                    "npm": "@ai-sdk/openai",
                    "name": "我的代理",
                    "models": { "gpt-5.5": { "name": "GPT-5.5" } },
                    "options": { "baseURL": "https://old.com/v1" }
                }
            }
        });

        let merged = adapter.merge_config(&sample_profile(), &shared);

        // 只更新凭据，保留已有 npm / name / models
        assert_eq!(merged["provider"]["anthropic"]["npm"], "@ai-sdk/openai");
        assert_eq!(merged["provider"]["anthropic"]["name"], "我的代理");
        assert_eq!(
            merged["provider"]["anthropic"]["models"]["gpt-5.5"]["name"],
            "GPT-5.5"
        );
        assert_eq!(
            merged["provider"]["anthropic"]["options"]["baseURL"],
            "https://api.example.com/v1"
        );
        assert_eq!(
            merged["provider"]["anthropic"]["options"]["apiKey"],
            "sk-test-key"
        );
    }

    #[test]
    fn test_merge_preserves_other_providers() {
        let adapter = OpenCodeAdapter::new();
        let shared = serde_json::json!({
            "provider": {
                "openai": {
                    "name": "OpenAI",
                    "options": { "timeout": 5000 }
                }
            }
        });

        let merged = adapter.merge_config(&sample_profile(), &shared);

        // 切换 anthropic 不影响 openai
        assert_eq!(merged["provider"]["openai"]["name"], "OpenAI");
        assert_eq!(merged["provider"]["openai"]["options"]["timeout"], 5000);
        // 新 provider 已添加
        assert_eq!(
            merged["provider"]["anthropic"]["options"]["apiKey"],
            "sk-test-key"
        );
    }

    #[test]
    fn test_remove_provider_from_config_removes_only_target() {
        let config = serde_json::json!({
            "provider": {
                "cpa": { "options": { "apiKey": "k1", "baseURL": "http://127.0.0.1:8317/v1" } },
                "openai": { "options": { "apiKey": "k2", "baseURL": "https://api.example/v1" } }
            },
            "model": "cpa/claude-opus-4-8",
            "small_model": "cpa/haiku",
            "mcp": { "bing-search": { "type": "local", "command": ["npx"] } }
        });
        let out = OpenCodeAdapter::remove_provider_from_config(&config, "cpa");
        assert!(out["provider"].get("cpa").is_none());
        assert!(out["provider"].get("openai").is_some());
        // 顶层 model / small_model 指向被删 provider 时一并清掉
        assert!(out.get("model").is_none());
        assert!(out.get("small_model").is_none());
        // 其它配置保留
        assert_eq!(out["mcp"]["bing-search"]["type"], "local");
    }

    #[test]
    fn test_remove_provider_keeps_model_for_other_provider() {
        let config = serde_json::json!({
            "provider": {
                "cpa": { "options": {} },
                "openai": { "options": {} }
            },
            "model": "openai/grok-4.5",
            "small_model": "openai/mini"
        });
        let out = OpenCodeAdapter::remove_provider_from_config(&config, "cpa");
        assert!(out["provider"].get("cpa").is_none());
        assert_eq!(out["model"], "openai/grok-4.5");
        assert_eq!(out["small_model"], "openai/mini");
    }

    #[test]
    fn test_remove_provider_normalizes_case_and_missing_is_noop() {
        let config = serde_json::json!({
            "provider": { "openai": { "options": { "apiKey": "k" } } },
            "model": "openai/x"
        });
        // 大小写归一
        let out = OpenCodeAdapter::remove_provider_from_config(&config, "OpenAI");
        assert!(out["provider"].get("openai").is_none());
        assert!(out.get("model").is_none());

        // 不存在的 provider：不改其它内容
        let out2 = OpenCodeAdapter::remove_provider_from_config(&config, "cpa");
        assert!(out2["provider"].get("openai").is_some());
        assert_eq!(out2["model"], "openai/x");
    }

    #[test]
    fn test_opencode_provider_still_used_by_remaining_profiles() {
        // 同 provider 还有别的档案时，不应清本地
        let remaining = vec![
            ApiProfile {
                name: "a".into(),
                provider: "CPA".into(),
                target_app: Some(crate::models::TargetApp::OpenCode),
                ..sample_profile()
            },
            ApiProfile {
                name: "b".into(),
                provider: "openai".into(),
                target_app: Some(crate::models::TargetApp::OpenCode),
                ..sample_profile()
            },
        ];
        assert!(OpenCodeAdapter::provider_still_used(&remaining, "cpa"));
        assert!(OpenCodeAdapter::provider_still_used(&remaining, "CPA"));
        assert!(!OpenCodeAdapter::provider_still_used(
            &remaining,
            "anthropic"
        ));
        // 其它工具的同名 provider 不算
        let mixed = vec![ApiProfile {
            name: "c".into(),
            provider: "cpa".into(),
            target_app: Some(crate::models::TargetApp::Hermes),
            ..sample_profile()
        }];
        assert!(!OpenCodeAdapter::provider_still_used(&mixed, "cpa"));
    }
}
