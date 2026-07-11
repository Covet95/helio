use super::ConfigAdapter;
use crate::models::{ApiProfile, OpenCodeProfileFields};
use anyhow::{Context, Result};
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
        if api_profile.provider.is_empty() {
            "custom".to_string()
        } else {
            api_profile.provider.to_lowercase()
        }
    }

    /// 把单个 Claude MCP server 转成 OpenCode 格式。
    /// 既无 command 又无 url → 无法转换，返回 None（跳过该项）。
    /// 丢弃 OpenCode 不认的字段（type/alwaysAllow/startup_timeout_sec/未知项）。
    fn convert_one_server(server: &serde_json::Value) -> Option<serde_json::Value> {
        let obj = server.as_object()?;
        let mut out = serde_json::Map::new();

        if let Some(url) = obj.get("url").and_then(|v| v.as_str()) {
            // 远程型
            out.insert("type".into(), serde_json::Value::String("remote".into()));
            out.insert("url".into(), serde_json::Value::String(url.to_string()));
            // headers 携带认证信息（如 Bearer token），OpenCode 远程 MCP 支持，保留
            if let Some(headers) = obj.get("headers").and_then(|v| v.as_object()) {
                if !headers.is_empty() {
                    out.insert(
                        "headers".into(),
                        serde_json::Value::Object(headers.clone()),
                    );
                }
            }
        } else if let Some(command) = obj.get("command").and_then(|v| v.as_str()) {
            // 本地型：command(字符串) + args(数组) → command(数组)
            let mut cmd = vec![serde_json::Value::String(command.to_string())];
            if let Some(args) = obj.get("args").and_then(|v| v.as_array()) {
                cmd.extend(args.iter().cloned());
            }
            out.insert("type".into(), serde_json::Value::String("local".into()));
            out.insert("command".into(), serde_json::Value::Array(cmd));
            // env → environment（仅非空对象）
            if let Some(env) = obj.get("env").and_then(|v| v.as_object()) {
                if !env.is_empty() {
                    out.insert(
                        "environment".into(),
                        serde_json::Value::Object(env.clone()),
                    );
                }
            }
        } else {
            // 既无 url 又无 command：无法转换
            return None;
        }

        out.insert("enabled".into(), serde_json::Value::Bool(true));
        Some(serde_json::Value::Object(out))
    }

    /// 把 Claude 的 mcpServers 对象整体转成 OpenCode 的 mcp 对象。
    /// 非对象输入 → 空对象。转不了的单项跳过。
    fn claude_mcp_to_opencode(claude_servers: &serde_json::Value) -> serde_json::Value {
        let mut out = serde_json::Map::new();
        if let Some(servers) = claude_servers.as_object() {
            for (name, server) in servers {
                if let Some(converted) = Self::convert_one_server(server) {
                    out.insert(name.clone(), converted);
                }
            }
        }
        serde_json::Value::Object(out)
    }

    /// 把转换后的 mcp 对象合并进 config["mcp"]：同名覆盖，保留 config 已有的额外项。
    /// config.mcp 不存在或不是对象时，以空对象起步。
    fn merge_mcp_into(config: &mut serde_json::Value, mcp: &serde_json::Value) {
        let incoming = match mcp.as_object() {
            Some(m) if !m.is_empty() => m,
            _ => return, // 没有要合并的，直接返回
        };
        // 确保 config["mcp"] 是对象
        let needs_init = !config
            .get("mcp")
            .map(|v| v.is_object())
            .unwrap_or(false);
        if needs_init {
            config["mcp"] = serde_json::json!({});
        }
        if let Some(dst) = config.get_mut("mcp").and_then(|v| v.as_object_mut()) {
            for (name, server) in incoming {
                dst.insert(name.clone(), server.clone()); // 同名覆盖
            }
        }
    }

    /// 读 ~/.claude.json 的 mcpServers。读不到/解析失败/无该键 → None（不报错）。
    fn read_claude_mcp_servers() -> Option<serde_json::Value> {
        let home = dirs::home_dir()?;
        let path = home.join(".claude.json");
        let content = fs::read_to_string(&path).ok()?;
        let parsed: serde_json::Value = serde_json::from_str(&content).ok()?;
        parsed.get("mcpServers").cloned()
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

    fn extract_shared_config(&self, config: &serde_json::Value) -> serde_json::Value {
        // OpenCode 多 provider 共存：保留所有 provider（含各自凭据），
        // 这样切换某个 provider 时，其他 provider 仍然可用。
        // 切换逻辑只在 merge_config 里更新「当前 provider」的凭据 + 顶层默认 model。
        config.clone()
    }

    fn merge_config(
        &self,
        api_profile: &ApiProfile,
        shared_config: &serde_json::Value,
    ) -> serde_json::Value {
        let mut config = shared_config.clone();
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
                        serde_json::Value::String(api_profile.api_url.clone()),
                    );
                }

                // 模型列表：把 profile.models（多选）里的每个模型写进 provider 的
                // models 声明，加上默认模型 model（OpenCode 要列出/切换这些模型）。
                let mut model_ids: Vec<String> = Vec::new();
                if let Some(list) = api_profile.opencode.models.as_ref() {
                    for m in list {
                        let m = m.trim();
                        if !m.is_empty() {
                            model_ids.push(m.to_string());
                        }
                    }
                }
                if let Some(default_model) = api_profile
                    .model
                    .as_deref()
                    .map(str::trim)
                    .filter(|m| !m.is_empty())
                {
                    if !model_ids.iter().any(|m| m == default_model) {
                        model_ids.push(default_model.to_string());
                    }
                }
                if !model_ids.is_empty() {
                    let models = p
                        .entry("models".to_string())
                        .or_insert_with(|| serde_json::json!({}));
                    if let Some(models_obj) = models.as_object_mut() {
                        for m in &model_ids {
                            models_obj
                                .entry(m.clone())
                                .or_insert_with(|| serde_json::json!({ "name": m }));
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
            .or_else(|| {
                api_profile.opencode.models.as_ref().and_then(|list| {
                    list.iter()
                        .map(|m| m.trim())
                        .find(|m| !m.is_empty())
                        .map(|m| m.to_string())
                })
            });

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

        // 自动同步 Claude 的 MCP（读 ~/.claude.json，转 OpenCode 格式，合并进 config.mcp）。
        // 读不到 Claude 配置时静默跳过，不影响切换。
        if let Some(claude_servers) = Self::read_claude_mcp_servers() {
            let converted = Self::claude_mcp_to_opencode(&claude_servers);
            Self::merge_mcp_into(&mut config, &converted);
        }

        config
    }

    fn write_config(&self, config: &serde_json::Value) -> Result<()> {
        let path = self.config_path();

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).context("Failed to create OpenCode config directory")?;
        }

        let content =
            serde_json::to_string_pretty(config).context("Failed to serialize OpenCode config")?;
        let temp_path = path.with_extension("json.tmp");
        fs::write(&temp_path, &content).context("Failed to write temp OpenCode config")?;
        if let Ok(file) = fs::File::open(&temp_path) {
            let _ = file.sync_all();
        }
        fs::rename(&temp_path, &path).context("Failed to rename temp OpenCode config")?;

        Ok(())
    }

    fn backup_config(&self) -> Result<PathBuf> {
        let path = self.config_path();

        if !path.exists() {
            anyhow::bail!("Config file does not exist");
        }

        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let backup_path = self
            .config_dir
            .join(format!("opencode.backup.{}.json", timestamp));

        fs::copy(&path, &backup_path).context("Failed to backup config")?;

        self.cleanup_old_backups(10)?;

        Ok(backup_path)
    }

    fn cleanup_old_backups(&self, keep: usize) -> Result<()> {
        let mut backups: Vec<_> = fs::read_dir(&self.config_dir)?
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("opencode.backup.")
            })
            .collect();

        backups.sort_by_key(|entry| {
            entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        });
        backups.reverse();

        for entry in backups.iter().skip(keep) {
            let _ = fs::remove_file(entry.path());
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_extract_shared_preserves_provider_creds() {
        // 方向 B（多 provider 共存）：extract 保留所有 provider 的凭据，
        // 否则共存的其他 provider 会变成空凭据的坏 provider。
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

        // 凭据被保留（共存 provider 仍可用）
        assert_eq!(shared["provider"]["anthropic"]["options"]["apiKey"], "sk-secret");
        assert_eq!(shared["provider"]["anthropic"]["options"]["baseURL"], "https://api.com");
        assert_eq!(shared["provider"]["anthropic"]["options"]["timeout"], 30000);
        // 其余配置原样保留
        assert_eq!(shared["provider"]["anthropic"]["name"], "Anthropic");
        assert_eq!(shared["mcp"]["fs"]["type"], "local");
        assert_eq!(shared["permission"]["edit"], "ask");
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
        assert_eq!(merged["provider"]["anthropic"]["options"]["apiKey"], "sk-test-key");
        // 旧 provider cpa 凭据原样保留，不被清空
        assert_eq!(merged["provider"]["cpa"]["options"]["apiKey"], "sk-cpa");
        assert_eq!(merged["provider"]["cpa"]["options"]["baseURL"], "http://127.0.0.1:8317/v1");
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
                models: Some(vec!["claude-sonnet-4-6".to_string(), "claude-haiku-4-5".to_string()]),
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
        assert_eq!(merged["provider"]["anthropic"]["options"]["apiKey"], "sk-test-key");
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
    fn test_merge_writes_multiple_models() {
        let adapter = OpenCodeAdapter::new();
        let profile = ApiProfile {
            model: Some("claude-opus-4-8".to_string()),
            opencode: OpenCodeProfileFields {
                models: Some(vec![
                    "claude-sonnet-4-6".to_string(),
                    "claude-haiku-4-5".to_string(),
                ]),
            },
            ..sample_profile()
        };
        let merged = adapter.merge_config(&profile, &serde_json::json!({}));

        let models = &merged["provider"]["anthropic"]["models"];
        // 多选的两个 + 默认模型，共 3 个都在 provider.models
        assert!(models["claude-sonnet-4-6"].is_object());
        assert!(models["claude-haiku-4-5"].is_object());
        assert!(models["claude-opus-4-8"].is_object(), "默认模型也应在 models 里");
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
        assert_eq!(merged["provider"]["anthropic"]["models"]["gpt-5.5"]["name"], "GPT-5.5");
        assert_eq!(merged["provider"]["anthropic"]["options"]["baseURL"], "https://api.example.com/v1");
        assert_eq!(merged["provider"]["anthropic"]["options"]["apiKey"], "sk-test-key");
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
        assert_eq!(merged["provider"]["anthropic"]["options"]["apiKey"], "sk-test-key");
    }

    #[test]
    fn test_convert_stdio_server() {
        let claude = serde_json::json!({
            "type": "stdio",
            "command": "npx",
            "args": ["-y", "pkg"],
            "env": { "K": "v" }
        });
        let out = OpenCodeAdapter::convert_one_server(&claude).unwrap();
        assert_eq!(out["type"], "local");
        assert_eq!(out["command"], serde_json::json!(["npx", "-y", "pkg"]));
        assert_eq!(out["environment"]["K"], "v");
        assert_eq!(out["enabled"], true);
        assert!(out.get("args").is_none());
    }

    #[test]
    fn test_convert_command_only() {
        let claude = serde_json::json!({ "command": "foo" });
        let out = OpenCodeAdapter::convert_one_server(&claude).unwrap();
        assert_eq!(out["command"], serde_json::json!(["foo"]));
        assert_eq!(out["type"], "local");
    }

    #[test]
    fn test_convert_empty_env_omitted() {
        let claude = serde_json::json!({ "command": "foo", "env": {} });
        let out = OpenCodeAdapter::convert_one_server(&claude).unwrap();
        assert!(out.get("environment").is_none(), "空 env 不应写 environment");
    }

    #[test]
    fn test_convert_remote_server() {
        let claude = serde_json::json!({ "type": "sse", "url": "https://x/mcp" });
        let out = OpenCodeAdapter::convert_one_server(&claude).unwrap();
        assert_eq!(out["type"], "remote");
        assert_eq!(out["url"], "https://x/mcp");
        assert_eq!(out["enabled"], true);
    }

    #[test]
    fn test_convert_remote_preserves_headers() {
        let claude = serde_json::json!({
            "type": "http",
            "url": "https://api.githubcopilot.com/mcp/",
            "headers": { "Authorization": "Bearer tok123" }
        });
        let out = OpenCodeAdapter::convert_one_server(&claude).unwrap();
        assert_eq!(out["type"], "remote");
        assert_eq!(out["url"], "https://api.githubcopilot.com/mcp/");
        assert_eq!(out["headers"]["Authorization"], "Bearer tok123");
        assert_eq!(out["enabled"], true);
    }

    #[test]
    fn test_convert_drops_claude_specific_fields() {
        let claude = serde_json::json!({
            "command": "npx",
            "args": ["chrome-devtools-mcp@latest"],
            "alwaysAllow": ["navigate_page", "click"],
            "startup_timeout_sec": 120
        });
        let out = OpenCodeAdapter::convert_one_server(&claude).unwrap();
        assert!(out.get("alwaysAllow").is_none());
        assert!(out.get("startup_timeout_sec").is_none());
        assert_eq!(out["command"], serde_json::json!(["npx", "chrome-devtools-mcp@latest"]));
    }

    #[test]
    fn test_convert_no_command_no_url_skipped() {
        let claude = serde_json::json!({ "env": { "K": "v" } });
        assert!(OpenCodeAdapter::convert_one_server(&claude).is_none());
    }

    #[test]
    fn test_claude_mcp_to_opencode_batch() {
        let claude = serde_json::json!({
            "playwright": { "command": "npx", "args": ["@playwright/mcp@latest"] },
            "remote-x": { "type": "http", "url": "https://x/mcp" },
            "broken": { "env": { "K": "v" } }
        });
        let out = OpenCodeAdapter::claude_mcp_to_opencode(&claude);
        assert_eq!(out["playwright"]["type"], "local");
        assert_eq!(out["remote-x"]["type"], "remote");
        // 转不了的被跳过
        assert!(out.get("broken").is_none());
    }

    #[test]
    fn test_claude_mcp_to_opencode_non_object() {
        let out = OpenCodeAdapter::claude_mcp_to_opencode(&serde_json::json!(null));
        assert_eq!(out, serde_json::json!({}));
        let out2 = OpenCodeAdapter::claude_mcp_to_opencode(&serde_json::json!("nope"));
        assert_eq!(out2, serde_json::json!({}));
    }

    #[test]
    fn test_merge_mcp_overwrites_same_name_keeps_extra() {
        let mut config = serde_json::json!({
            "mcp": {
                "foo": { "type": "local", "command": ["OLD"] },
                "bar": { "type": "local", "command": ["keep"] }
            }
        });
        let incoming = serde_json::json!({
            "foo": { "type": "local", "command": ["NEW"], "enabled": true }
        });
        OpenCodeAdapter::merge_mcp_into(&mut config, &incoming);
        assert_eq!(config["mcp"]["foo"]["command"], serde_json::json!(["NEW"]));
        assert_eq!(config["mcp"]["bar"]["command"], serde_json::json!(["keep"]));
    }

    #[test]
    fn test_merge_mcp_empty_incoming_noop() {
        let mut config = serde_json::json!({ "mcp": { "bar": { "x": 1 } } });
        OpenCodeAdapter::merge_mcp_into(&mut config, &serde_json::json!({}));
        assert_eq!(config["mcp"]["bar"]["x"], 1);
    }

    #[test]
    fn test_merge_mcp_creates_block_when_absent() {
        let mut config = serde_json::json!({ "model": "x/y" });
        let incoming = serde_json::json!({ "foo": { "type": "local", "command": ["a"] } });
        OpenCodeAdapter::merge_mcp_into(&mut config, &incoming);
        assert_eq!(config["mcp"]["foo"]["command"], serde_json::json!(["a"]));
        assert_eq!(config["model"], "x/y");
    }
}
