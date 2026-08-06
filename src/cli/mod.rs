use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use colored::Colorize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use switch_api::adapters::get_adapter;
use switch_api::db::Database;
use switch_api::models::{
    ApiKeyEntry, ApiProfile, CodexProfileFields, HermesProfileFields, OpenClawProfileFields,
    TargetApp,
};
use switch_api::probe::ProbeRequest;
use switch_api::utils;

#[derive(Parser)]
#[command(name = "switch-api")]
#[command(version, about = "智能 AI CLI 工具 API 切换器 - 配置分层架构", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// 数据库路径
    #[arg(long, env = "SWITCH_API_DB", default_value_os_t = default_db_path())]
    pub db_path: PathBuf,

    /// 详细输出
    #[arg(short, long, global = true)]
    pub verbose: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    /// 初始化：从现有配置导入
    Init {
        /// 目标应用 (claude-code, codex, pi, opencode, hermes, openclaw)
        target_app: String,
    },

    /// 管理 API Profiles
    #[command(subcommand)]
    Profile(Box<ProfileCommands>),

    /// 切换 API Profile
    Switch {
        /// 目标应用 (claude-code, codex, pi, opencode, hermes, openclaw)
        target_app: String,
        /// Profile 名称
        profile_name: String,
        /// 跳过备份
        #[arg(long)]
        no_backup: bool,
        /// 写入前按 key 顺序探活，失败则 failover 到下一把；全失败则不写
        #[arg(long)]
        probe: bool,
    },

    /// 查看当前状态
    Status {
        /// 显示详细信息
        #[arg(short, long)]
        verbose: bool,
    },

    /// 同步共享配置（从配置文件回填到数据库）
    Sync {
        /// 目标应用 (claude-code, codex, pi, opencode, hermes, openclaw)
        target_app: String,
    },

    /// 导出数据库
    Export {
        /// 输出路径
        #[arg(short, long)]
        output: PathBuf,
    },

    /// 导入数据库
    Import {
        /// 数据库文件路径
        input: PathBuf,
        /// 强制覆盖不提示
        #[arg(short, long)]
        force: bool,
    },
}

#[derive(Subcommand)]
pub enum ProfileCommands {
    /// 添加新的 API Profile
    Add {
        /// Profile 名称
        name: String,
        /// API URL
        #[arg(long)]
        url: Option<String>,
        /// API Key
        #[arg(long)]
        key: Option<String>,
        /// Provider (anthropic, openai, custom)
        #[arg(long, default_value = "anthropic")]
        provider: String,
        /// 模型映射 (JSON 格式)
        #[arg(long)]
        model_mapping: Option<String>,
        /// 默认模型
        #[arg(long)]
        model: Option<String>,
        /// 推理强度 (low, medium, high, xhigh)
        #[arg(long)]
        reasoning_effort: Option<String>,
        /// Codex provider-scoped API key environment variable
        #[arg(long)]
        env_key: Option<String>,
        /// Codex custom provider supports standalone web search
        #[arg(long)]
        supports_standalone_web_search: bool,
        /// Codex built-in Amazon Bedrock AWS profile
        #[arg(long)]
        aws_profile: Option<String>,
        /// Codex built-in Amazon Bedrock AWS region
        #[arg(long)]
        aws_region: Option<String>,
        /// 启用 1M 上下文窗口
        #[arg(long)]
        context_1m: bool,
        /// 协议模式（OpenCode/Hermes/OpenClaw：chat_completions / responses / anthropic_messages / codex_responses）
        #[arg(long)]
        api_mode: Option<String>,
        /// OpenClaw models[].maxTokens
        #[arg(long)]
        max_tokens: Option<i64>,
        /// 目标应用 (claude-code, codex, pi, opencode, hermes, openclaw)
        #[arg(long)]
        target_app: Option<String>,
    },

    /// 列出所有 Profiles
    List {
        /// 显示详细信息
        #[arg(short, long)]
        verbose: bool,
    },

    /// 删除 Profile
    Delete {
        /// Profile 名称
        name: String,
        /// 目标应用 (claude-code, codex, pi, opencode, hermes, openclaw)
        #[arg(long)]
        target_app: Option<String>,
        /// 强制删除不提示
        #[arg(short, long)]
        force: bool,
    },

    /// 查看 Profile 详情
    Show {
        /// Profile 名称
        name: String,
        /// 目标应用 (claude-code, codex, pi, opencode, hermes, openclaw)
        #[arg(long)]
        target_app: Option<String>,
    },

    /// 更新 Profile
    Update {
        /// Profile 名称
        name: String,
        /// 目标应用 (claude-code, codex, pi, opencode, hermes, openclaw)
        #[arg(long)]
        target_app: Option<String>,
        /// 新的 API URL
        #[arg(long)]
        url: Option<String>,
        /// 新的 API Key
        #[arg(long)]
        key: Option<String>,
        /// 新的 Provider
        #[arg(long)]
        provider: Option<String>,
        /// 新的模型映射
        #[arg(long)]
        model_mapping: Option<String>,
        /// 新的默认模型
        #[arg(long)]
        model: Option<String>,
        /// 新的推理强度 (low, medium, high, xhigh)
        #[arg(long)]
        reasoning_effort: Option<String>,
        /// 新的 Codex provider-scoped API key environment variable (empty clears)
        #[arg(long)]
        env_key: Option<String>,
        /// Codex custom provider standalone web-search support
        #[arg(long, num_args = 0..=1, default_missing_value = "true")]
        supports_standalone_web_search: Option<bool>,
        /// Codex built-in Amazon Bedrock AWS profile (empty clears)
        #[arg(long)]
        aws_profile: Option<String>,
        /// Codex built-in Amazon Bedrock AWS region (empty clears)
        #[arg(long)]
        aws_region: Option<String>,
        /// 启用 1M 上下文窗口
        #[arg(long, num_args = 0..=1, default_missing_value = "true")]
        context_1m: Option<bool>,
        /// 协议模式（Hermes/OpenClaw）
        #[arg(long)]
        api_mode: Option<String>,
        /// OpenClaw models[].maxTokens
        #[arg(long)]
        max_tokens: Option<i64>,
    },

    /// 管理同一 Profile 下的多把 API Key（池 + 手动活跃）
    #[command(subcommand)]
    Key(KeyCommands),
}

#[derive(Subcommand)]
pub enum KeyCommands {
    /// 添加一把 key
    Add {
        /// Profile 名称
        name: String,
        #[arg(long)]
        target_app: Option<String>,
        /// API Key 明文
        #[arg(long)]
        key: String,
        /// 备注标签
        #[arg(long, default_value = "")]
        label: String,
        /// 添加后设为活跃
        #[arg(long)]
        activate: bool,
    },
    /// 列出 keys（脱敏）
    List {
        name: String,
        #[arg(long)]
        target_app: Option<String>,
    },
    /// 将指定 id 或 label 设为活跃（switch 只写活跃 key）
    Use {
        name: String,
        /// key id 或 label
        key_ref: String,
        #[arg(long)]
        target_app: Option<String>,
    },
    /// 删除一把 key（按 id 或 label）
    Remove {
        name: String,
        key_ref: String,
        #[arg(long)]
        target_app: Option<String>,
    },
    /// 按顺序探活 key，成功则设为活跃（若该 profile 已是 active 会提示 re-switch）
    Failover {
        name: String,
        #[arg(long)]
        target_app: Option<String>,
    },
}

pub fn execute(cli: Cli) -> Result<()> {
    // 导入要删除并替换 live 文件，必须在打开连接之前处理：
    // 持着旧连接做替换会让它继续指向已删除的 inode，还可能把缓存的 WAL 状态写回。
    if let Commands::Import { input, force } = cli.command {
        return cmd_import(input, &cli.db_path, force);
    }

    let db = Database::open(&cli.db_path)?;
    // 上次切换可能在写盘与写 DB 之间崩溃，按 journal 恢复半状态。
    if let Err(error) = switch_api::adapters::journal::recover_interrupted_switch(&db) {
        tracing::warn!("恢复中断的切换失败(已跳过): {error:#}");
    }

    match cli.command {
        Commands::Init { target_app } => {
            let target = parse_target_app(&target_app)?;
            cmd_init(&db, target)?;
        }
        Commands::Profile(profile_cmd) => match *profile_cmd {
            ProfileCommands::Add {
                name,
                url,
                key,
                provider,
                model_mapping,
                model,
                reasoning_effort,
                env_key,
                supports_standalone_web_search,
                aws_profile,
                aws_region,
                context_1m,
                api_mode,
                max_tokens,
                target_app,
            } => {
                cmd_profile_add(
                    &db,
                    ProfileAddRequest {
                        name,
                        url,
                        key,
                        provider,
                        model_mapping,
                        model,
                        reasoning_effort,
                        env_key,
                        supports_standalone_web_search,
                        aws_profile,
                        aws_region,
                        context_1m,
                        api_mode,
                        max_tokens,
                        target_app,
                    },
                )?;
            }
            ProfileCommands::List { verbose } => {
                cmd_profile_list(&db, verbose)?;
            }
            ProfileCommands::Delete {
                name,
                target_app,
                force,
            } => {
                cmd_profile_delete(&db, name, target_app, force)?;
            }
            ProfileCommands::Show { name, target_app } => {
                cmd_profile_show(&db, name, target_app)?;
            }
            ProfileCommands::Update {
                name,
                target_app,
                url,
                key,
                provider,
                model_mapping,
                model,
                reasoning_effort,
                env_key,
                supports_standalone_web_search,
                aws_profile,
                aws_region,
                context_1m,
                api_mode,
                max_tokens,
            } => {
                cmd_profile_update(
                    &db,
                    ProfileUpdateRequest {
                        name,
                        target_app,
                        url,
                        key,
                        provider,
                        model_mapping,
                        model,
                        reasoning_effort,
                        env_key,
                        supports_standalone_web_search,
                        aws_profile,
                        aws_region,
                        context_1m,
                        api_mode,
                        max_tokens,
                    },
                )?;
            }
            ProfileCommands::Key(key_cmd) => match key_cmd {
                KeyCommands::Add {
                    name,
                    target_app,
                    key,
                    label,
                    activate,
                } => cmd_profile_key_add(&db, name, target_app, key, label, activate)?,
                KeyCommands::List { name, target_app } => {
                    cmd_profile_key_list(&db, name, target_app)?
                }
                KeyCommands::Use {
                    name,
                    key_ref,
                    target_app,
                } => cmd_profile_key_use(&db, name, target_app, key_ref)?,
                KeyCommands::Remove {
                    name,
                    key_ref,
                    target_app,
                } => cmd_profile_key_remove(&db, name, target_app, key_ref)?,
                KeyCommands::Failover { name, target_app } => {
                    cmd_profile_key_failover(&db, name, target_app)?
                }
            },
        },
        Commands::Switch {
            target_app,
            profile_name,
            no_backup,
            probe,
        } => {
            let target = parse_target_app(&target_app)?;
            cmd_switch(&db, target, profile_name, !no_backup, probe)?;
        }
        Commands::Status { verbose } => {
            cmd_status(&db, verbose)?;
        }
        Commands::Sync { target_app } => {
            cmd_sync(&db, parse_target_app(&target_app)?)?;
        }
        Commands::Export { output } => {
            cmd_export(&cli.db_path, output)?;
        }
        // 已在函数开头提前返回：导入既不能持有 live 连接，也不该要求 live 库能打开
        // （库损坏时导入正是恢复手段）。
        Commands::Import { .. } => unreachable!(),
    }

    Ok(())
}

// ========== 命令实现 ==========

struct ProfileAddRequest {
    name: String,
    url: Option<String>,
    key: Option<String>,
    provider: String,
    model_mapping: Option<String>,
    model: Option<String>,
    reasoning_effort: Option<String>,
    env_key: Option<String>,
    supports_standalone_web_search: bool,
    aws_profile: Option<String>,
    aws_region: Option<String>,
    context_1m: bool,
    api_mode: Option<String>,
    max_tokens: Option<i64>,
    target_app: Option<String>,
}

struct ProfileUpdateRequest {
    name: String,
    target_app: Option<String>,
    url: Option<String>,
    key: Option<String>,
    provider: Option<String>,
    model_mapping: Option<String>,
    model: Option<String>,
    reasoning_effort: Option<String>,
    env_key: Option<String>,
    supports_standalone_web_search: Option<bool>,
    aws_profile: Option<String>,
    aws_region: Option<String>,
    context_1m: Option<bool>,
    api_mode: Option<String>,
    max_tokens: Option<i64>,
}

fn validate_profile_input(profile: &ApiProfile) -> Result<()> {
    let is_bedrock = switch_api::adapters::codex::CodexAdapter::is_amazon_bedrock_profile(profile);
    if is_bedrock {
        if profile.target_app != Some(TargetApp::Codex) {
            anyhow::bail!("provider=amazon-bedrock requires --target-app codex");
        }
        return Ok(());
    }

    if profile.api_url.trim().is_empty() {
        anyhow::bail!("--url is required unless provider=amazon-bedrock for Codex");
    }
    let uses_codex_env =
        profile.target_app == Some(TargetApp::Codex) && profile.codex.env_key.is_some();
    if !uses_codex_env && profile.api_key.trim().is_empty() {
        anyhow::bail!("--key is required unless Codex --env-key is provided");
    }
    Ok(())
}

fn cmd_init(db: &Database, target_app: TargetApp) -> Result<()> {
    utils::info(&format!("初始化 {} 配置...", target_app));

    let adapter = get_adapter(target_app);

    // 读取当前配置
    let config = adapter.read_config()?;

    // 提取共享配置
    let shared = adapter.extract_shared_config(&config);

    // 保存到数据库
    db.save_shared_config(target_app, shared)?;

    utils::success(&format!(
        "已从 {} 导入共享配置",
        adapter.config_path().display()
    ));
    utils::info("现在可以添加 API Profile:");
    println!("   switch-api profile add official --url https://api.anthropic.com --key sk-ant-xxx");

    Ok(())
}

fn cmd_profile_add(db: &Database, request: ProfileAddRequest) -> Result<()> {
    let ProfileAddRequest {
        name,
        url,
        key,
        provider,
        model_mapping,
        model,
        reasoning_effort,
        env_key,
        supports_standalone_web_search,
        aws_profile,
        aws_region,
        context_1m,
        api_mode,
        max_tokens,
        target_app,
    } = request;
    let model_mapping_map = if let Some(json) = model_mapping {
        Some(
            serde_json::from_str::<HashMap<String, String>>(&json)
                .context("Invalid model mapping JSON")?,
        )
    } else {
        None
    };

    let mut profile = ApiProfile::new(
        name.clone(),
        provider,
        url.unwrap_or_default(),
        key.unwrap_or_default(),
        model_mapping_map,
    );
    profile.model = model.filter(|value| !value.trim().is_empty());
    let requested_reasoning_effort = reasoning_effort.filter(|value| !value.trim().is_empty());
    if context_1m {
        profile.context_1m = Some(true);
    }

    if let Some(t) = target_app.as_deref() {
        profile.target_app = Some(parse_target_app(t)?);
    }
    if profile.target_app == Some(TargetApp::Codex) {
        profile.codex = CodexProfileFields {
            reasoning_effort: requested_reasoning_effort,
            env_key: env_key.filter(|value| !value.trim().is_empty()),
            supports_standalone_web_search: supports_standalone_web_search.then_some(true),
            aws_profile: aws_profile.filter(|value| !value.trim().is_empty()),
            aws_region: aws_region.filter(|value| !value.trim().is_empty()),
            ..Default::default()
        };
    }
    let mode = api_mode.filter(|v| !v.trim().is_empty());
    match profile.target_app {
        Some(TargetApp::OpenCode) => {
            profile.opencode.opencode_api_mode =
                switch_api::adapters::opencode::OpenCodeAdapter::normalize_api_mode(
                    mode.as_deref(),
                )?
                .map(str::to_string);
            let _ = max_tokens;
        }
        Some(TargetApp::Hermes) => {
            profile.hermes = HermesProfileFields { api_mode: mode };
        }
        Some(TargetApp::OpenClaw) => {
            profile.openclaw = OpenClawProfileFields {
                api_mode: mode,
                max_tokens: max_tokens.filter(|&n| n > 0),
            };
        }
        _ => {
            // ignore tool-specific flags for other apps
            let _ = mode;
            let _ = max_tokens;
        }
    }
    validate_profile_input(&profile)?;

    db.add_profile(&profile)?;

    utils::success(&format!("已添加 API Profile: {}", name));

    Ok(())
}

fn cmd_profile_list(db: &Database, verbose: bool) -> Result<()> {
    let profiles = db.list_profiles()?;

    if profiles.is_empty() {
        utils::info("没有 API Profile");
        return Ok(());
    }

    println!("\n{}\n", "API Profiles:".bold());
    for profile in profiles {
        println!("  • {} ({})", profile.name.cyan().bold(), profile.provider);
        println!("    URL: {}", profile.api_url);

        if verbose {
            println!("    Key: {}", profile.masked_key());
        }

        if let Some(mapping) = profile.claude.model_mapping {
            println!("    Models:");
            for (key, value) in mapping {
                println!("      {} → {}", key, value);
            }
        }
        println!();
    }

    Ok(())
}

fn cmd_profile_delete(
    db: &Database,
    name: String,
    target_app: Option<String>,
    force: bool,
) -> Result<()> {
    let target = resolve_target_for_name(db, &name, target_app.as_deref())?;
    if !force {
        use std::io::Write;
        print!("确定删除 Profile「{}」({})？[y/N] ", name, target);
        std::io::stdout().flush().ok();
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).ok();
        if !matches!(input.trim().to_lowercase().as_str(), "y" | "yes") {
            utils::info("已取消");
            return Ok(());
        }
    }
    // OpenCode：删档案 + 无共用时清本地 provider（统一入口，与 GUI 共用）
    if target == TargetApp::OpenCode {
        if switch_api::adapters::opencode::OpenCodeAdapter::delete_profile_and_cleanup_local(
            db, &name,
        )? {
            utils::success(&format!("已删除 Profile: {} ({})", name, target));
        } else {
            utils::warning(&format!("未找到 Profile: {} ({})", name, target));
        }
        return Ok(());
    }
    if db.delete_profile(&name, target)? {
        utils::success(&format!("已删除 Profile: {} ({})", name, target));
    } else {
        utils::warning(&format!("未找到 Profile: {} ({})", name, target));
    }
    Ok(())
}

/// 解析 name 对应的 target：显式给了就用;没给则按 name 找所有匹配,唯一则用,多/零则报错。
fn resolve_target_for_name(
    db: &Database,
    name: &str,
    target_app: Option<&str>,
) -> Result<TargetApp> {
    if let Some(s) = target_app {
        return parse_target_app(s);
    }
    let matches: Vec<TargetApp> = db
        .list_profiles()?
        .into_iter()
        .filter(|p| p.name == name)
        .filter_map(|p| p.target_app)
        .collect();
    match matches.as_slice() {
        [t] => Ok(*t),
        [] => Err(anyhow::anyhow!("未找到 Profile: {name}")),
        _ => Err(anyhow::anyhow!(
            "存在多个同名 Profile「{name}」,请用 --target-app 指定工具"
        )),
    }
}

fn cmd_profile_show(db: &Database, name: String, target_app: Option<String>) -> Result<()> {
    let target = resolve_target_for_name(db, &name, target_app.as_deref())?;
    let profile = db.get_profile_by_name_and_target(&name, target)?;

    println!("\n{} {}\n", "API Profile:".bold(), profile.name.cyan());
    println!("  Provider: {}", profile.provider);
    println!("  URL: {}", profile.api_url);
    println!("  Key: {}", profile.masked_key());
    if let Some(model) = profile.model {
        println!("  Model: {}", model);
    }
    if let Some(reasoning_effort) = profile.codex.reasoning_effort {
        println!("  Reasoning Effort: {}", reasoning_effort);
    }
    if let Some(context_1m) = profile.context_1m {
        println!(
            "  1M Context: {}",
            if context_1m { "enabled" } else { "disabled" }
        );
    }
    match profile.target_app {
        Some(TargetApp::OpenCode) => {
            if let Some(mode) = profile.opencode.opencode_api_mode {
                println!("  API Mode (OpenCode): {}", mode);
            }
            if let Some(models) = profile.opencode.models {
                println!("  Models (OpenCode): {}", models.join(", "));
            }
        }
        Some(TargetApp::Hermes) => {
            if let Some(mode) = profile.hermes.api_mode {
                println!("  API Mode (Hermes): {}", mode);
            }
        }
        Some(TargetApp::OpenClaw) => {
            if let Some(mode) = profile.openclaw.api_mode {
                println!("  API Mode (OpenClaw): {}", mode);
            }
            if let Some(mt) = profile.openclaw.max_tokens {
                println!("  Max Tokens (OpenClaw): {}", mt);
            }
        }
        _ => {}
    }

    if let Some(mapping) = profile.claude.model_mapping {
        println!("  Model Mapping:");
        for (key, value) in mapping {
            println!("    {} → {}", key, value);
        }
    }

    if let Some(created_at) = profile.created_at {
        let dt = chrono::DateTime::from_timestamp(created_at, 0).unwrap_or_default();
        println!("  Created: {}", dt.format("%Y-%m-%d %H:%M:%S"));
    }

    Ok(())
}

fn cmd_profile_update(db: &Database, request: ProfileUpdateRequest) -> Result<()> {
    let ProfileUpdateRequest {
        name,
        target_app,
        url,
        key,
        provider,
        model_mapping,
        model,
        reasoning_effort,
        env_key,
        supports_standalone_web_search,
        aws_profile,
        aws_region,
        context_1m,
        api_mode,
        max_tokens,
    } = request;
    let target = resolve_target_for_name(db, &name, target_app.as_deref())?;
    let mut profile = db.get_profile_by_name_and_target(&name, target)?;

    let mut updated = false;

    if let Some(new_url) = url {
        profile.api_url = new_url;
        updated = true;
    }

    if let Some(new_key) = key {
        profile.api_key = new_key;
        profile.normalize_keys();
        if let Some(keys) = profile.api_keys.as_mut() {
            if let Some(active) = keys.iter_mut().find(|e| e.is_active) {
                active.key = profile.api_key.clone();
            }
        }
        updated = true;
    }

    if let Some(new_provider) = provider {
        profile.provider = new_provider;
        updated = true;
    }

    if let Some(mapping_json) = model_mapping {
        profile.claude.model_mapping = Some(serde_json::from_str(&mapping_json)?);
        updated = true;
    }

    if let Some(new_model) = model {
        profile.model = if new_model.trim().is_empty() {
            None
        } else {
            Some(new_model)
        };
        updated = true;
    }

    if let Some(new_reasoning_effort) = reasoning_effort {
        profile.codex.reasoning_effort = if new_reasoning_effort.trim().is_empty() {
            None
        } else {
            Some(new_reasoning_effort)
        };
        updated = true;
    }

    if let Some(new_env_key) = env_key {
        profile.codex.env_key = (!new_env_key.trim().is_empty()).then_some(new_env_key);
        updated = true;
    }

    if let Some(enabled) = supports_standalone_web_search {
        profile.codex.supports_standalone_web_search = enabled.then_some(true);
        updated = true;
    }

    if let Some(new_aws_profile) = aws_profile {
        profile.codex.aws_profile = (!new_aws_profile.trim().is_empty()).then_some(new_aws_profile);
        updated = true;
    }

    if let Some(new_aws_region) = aws_region {
        profile.codex.aws_region = (!new_aws_region.trim().is_empty()).then_some(new_aws_region);
        updated = true;
    }

    if let Some(new_context_1m) = context_1m {
        profile.context_1m = Some(new_context_1m);
        updated = true;
    }

    if let Some(mode) = api_mode {
        let mode = mode.trim().to_string();
        match target {
            TargetApp::OpenCode => {
                profile.opencode.opencode_api_mode =
                    switch_api::adapters::opencode::OpenCodeAdapter::normalize_api_mode(
                        (!mode.is_empty()).then_some(mode.as_str()),
                    )?
                    .map(str::to_string);
                updated = true;
            }
            TargetApp::Hermes => {
                profile.hermes.api_mode = if mode.is_empty() { None } else { Some(mode) };
                updated = true;
            }
            TargetApp::OpenClaw => {
                profile.openclaw.api_mode = if mode.is_empty() { None } else { Some(mode) };
                updated = true;
            }
            _ => {
                utils::warning("--api-mode 仅对 opencode / hermes / openclaw 生效，已忽略");
            }
        }
    }

    if let Some(mt) = max_tokens {
        match target {
            TargetApp::OpenClaw => {
                profile.openclaw.max_tokens = if mt > 0 { Some(mt) } else { None };
                updated = true;
            }
            _ => {
                utils::warning("--max-tokens 仅对 openclaw 生效，已忽略");
            }
        }
    }

    if !updated {
        utils::warning("没有提供任何更新");
        return Ok(());
    }

    if target == TargetApp::Codex
        && switch_api::adapters::codex::CodexAdapter::is_amazon_bedrock_profile(&profile)
    {
        profile.api_url.clear();
        profile.api_key.clear();
        profile.api_keys = None;
        profile.codex.env_key = None;
        profile.codex.supports_standalone_web_search = None;
    }
    validate_profile_input(&profile)?;
    db.update_profile(&profile)?;
    utils::success(&format!("已更新 Profile: {}", name));

    Ok(())
}

fn cmd_switch(
    db: &Database,
    target_app: TargetApp,
    profile_name: String,
    backup: bool,
    probe: bool,
) -> Result<()> {
    utils::info(&format!(
        "切换 {} 到 Profile: {}...",
        target_app, profile_name
    ));

    // 1. 获取 API Profile
    let mut api_profile = db.get_profile_by_name_and_target(&profile_name, target_app)?;
    api_profile.normalize_keys();
    if probe {
        utils::info("写入前探活（--probe）…");
        let rt = tokio::runtime::Runtime::new().context("创建 tokio runtime 失败")?;
        let r = rt.block_on(cli_failover(&mut api_profile, target_app))?;
        if !r {
            anyhow::bail!("探活 failover 失败，未写入配置");
        }
        db.update_profile(&api_profile)?;
    }

    let persisted_shared_config = db
        .get_shared_config(target_app)?
        .map(|config| config.config);
    let shared_config =
        switch_api::adapters::resolve_shared_config(target_app, persisted_shared_config)?;

    let applied = switch_api::adapters::apply_profile_switch(
        db,
        target_app,
        &api_profile,
        &shared_config,
        backup,
    )?;
    if let Some(backup_path) = applied.backup_path {
        utils::success(&format!("已备份到: {}", backup_path.display()));
    }

    utils::success(&format!("已切换到 {}", profile_name));
    println!("  配置文件: {}", applied.config_path.display());

    Ok(())
}

fn cmd_status(db: &Database, verbose: bool) -> Result<()> {
    println!("\n{}\n", "当前状态:".bold());

    for target_app in TargetApp::all() {
        print!("  {} ", target_app.to_string().cyan());

        if let Some(profile) = db.get_active_profile_full(target_app)? {
            println!("→ {} ({})", profile.name.green(), profile.api_url);

            if verbose {
                println!("     Provider: {}", profile.provider);
                println!("     Key: {}", profile.masked_key());
            }
        } else {
            println!("→ {}", "未设置".yellow());
        }
    }

    println!();
    Ok(())
}

fn cmd_sync(db: &Database, target_app: TargetApp) -> Result<()> {
    utils::info(&format!("同步 {} 共享配置...", target_app));

    let adapter = get_adapter(target_app);

    // 读取当前配置
    let config = adapter.read_config()?;

    // 提取共享配置
    let shared = adapter.extract_shared_config(&config);

    // 保存到数据库
    db.save_shared_config(target_app, shared)?;

    utils::success("已同步共享配置到数据库");

    Ok(())
}

fn cmd_export(db_path: &Path, output: PathBuf) -> Result<()> {
    // 快照而非文件拷贝：拷主文件会漏掉还在 -wal 里的已提交数据。
    Database::snapshot_to(db_path, &output)?;

    let size = std::fs::metadata(&output)?.len();
    utils::success(&format!(
        "已导出数据库到: {} ({})",
        output.display(),
        utils::format_size(size)
    ));

    Ok(())
}

fn cmd_import(input: PathBuf, db_path: &Path, force: bool) -> Result<()> {
    // 覆盖现有数据库前先校验：只读地确认这确实是 Helio 库。
    // 旧实现用 `Database::open` 当校验，而 `CREATE TABLE IF NOT EXISTS` 会把任意
    // SQLite 文件补全成"合法"库——实测能把浏览器书签库当备份导入并清空全部档案。
    Database::validate_import_candidate(&input)
        .map_err(|e| anyhow::anyhow!("输入文件不是 Helio 数据库: {}", e))?;

    if db_path.exists() && !force && !utils::confirm("将覆盖现有数据库，是否继续？")?
    {
        utils::info("已取消");
        return Ok(());
    }

    // 备份 + staging + 原子替换 + 清理陈旧 -wal 都在 core 里完成，
    // GUI 与 CLI 共用同一套逻辑；即使 --force 也始终备份以便回退。
    if let Some(backup) = Database::replace_file_from_import(&input, db_path)? {
        utils::success(&format!("已备份现有数据库到: {}", backup.display()));
    }
    utils::success(&format!("已导入数据库从: {}", input.display()));

    Ok(())
}

// ========== Profile multi-key ==========

fn cmd_profile_key_add(
    db: &Database,
    name: String,
    target_app: Option<String>,
    key: String,
    label: String,
    activate: bool,
) -> Result<()> {
    let target = resolve_target_for_name(db, &name, target_app.as_deref())?;
    let mut profile = db.get_profile_by_name_and_target(&name, target)?;
    profile.normalize_keys();
    let key = key.trim().to_string();
    if key.is_empty() {
        anyhow::bail!("key 不能为空");
    }
    let mut keys = profile.api_keys.take().unwrap_or_default();
    if activate {
        for e in keys.iter_mut() {
            e.is_active = false;
        }
    }
    let entry = ApiKeyEntry {
        id: ApiProfile::new_key_id(),
        label: if label.trim().is_empty() {
            format!("key-{}", keys.len() + 1)
        } else {
            label.trim().to_string()
        },
        key: key.clone(),
        is_active: activate || keys.is_empty(),
        last_probe_ok: None,
        last_probed_at: None,
        created_at: Some(chrono::Utc::now().timestamp()),
    };
    let id = entry.id.clone();
    keys.push(entry);
    profile.api_keys = Some(keys);
    profile.normalize_keys();
    if activate {
        let _ = profile.set_active_key_id(&id);
    }
    db.update_profile(&profile)?;
    utils::success(&format!(
        "已添加 key {}（{}）{}",
        id,
        profile
            .api_keys
            .as_ref()
            .and_then(|ks| ks.iter().find(|e| e.id == id))
            .map(|e| e.label.as_str())
            .unwrap_or(""),
        if profile.active_key() == key {
            " [活跃]"
        } else {
            ""
        }
    ));
    Ok(())
}

fn cmd_profile_key_list(db: &Database, name: String, target_app: Option<String>) -> Result<()> {
    let target = resolve_target_for_name(db, &name, target_app.as_deref())?;
    let mut profile = db.get_profile_by_name_and_target(&name, target)?;
    profile.normalize_keys();
    let keys = profile.api_keys.as_deref().unwrap_or(&[]);
    if keys.is_empty() {
        println!("(无 key)");
        return Ok(());
    }
    println!("\n{} {} 的 keys:\n", "Profile".bold(), name.cyan());
    for e in keys {
        let mark = if e.is_active { "●" } else { "○" };
        // 按字符切片（key 可能是多字节 UTF-8，字节切片会 panic）
        let masked = {
            let chars: Vec<char> = e.key.chars().collect();
            if chars.len() > 15 {
                let head: String = chars[..10].iter().collect();
                let tail: String = chars[chars.len() - 5..].iter().collect();
                format!("{head}...{tail}")
            } else {
                "***".into()
            }
        };
        println!(
            "  {} {}  {}  {}",
            mark,
            e.id.dimmed(),
            if e.label.is_empty() {
                "(no label)".into()
            } else {
                e.label.clone()
            },
            masked
        );
    }
    Ok(())
}

fn cmd_profile_key_use(
    db: &Database,
    name: String,
    target_app: Option<String>,
    key_ref: String,
) -> Result<()> {
    let target = resolve_target_for_name(db, &name, target_app.as_deref())?;
    let mut profile = db.get_profile_by_name_and_target(&name, target)?;
    if !profile.set_active_key_ref(&key_ref) {
        anyhow::bail!("未找到 key: {}", key_ref);
    }
    db.update_profile(&profile)?;
    utils::success(&format!(
        "已将活跃 key 设为 {}（switch 将写入此 key）",
        key_ref
    ));
    Ok(())
}

fn cmd_profile_key_remove(
    db: &Database,
    name: String,
    target_app: Option<String>,
    key_ref: String,
) -> Result<()> {
    let target = resolve_target_for_name(db, &name, target_app.as_deref())?;
    let mut profile = db.get_profile_by_name_and_target(&name, target)?;
    profile.normalize_keys();
    let needle = key_ref.trim();
    let Some(keys) = profile.api_keys.as_mut() else {
        anyhow::bail!("没有可删除的 key");
    };
    let before = keys.len();
    keys.retain(|e| e.id != needle && !e.label.eq_ignore_ascii_case(needle));
    if keys.len() == before {
        anyhow::bail!("未找到 key: {}", key_ref);
    }
    if keys.is_empty() {
        anyhow::bail!("不能删除最后一把 key；请先添加备用 key 或改用 profile update --key");
    }
    profile.api_keys = Some(keys.clone());
    profile.normalize_keys();
    db.update_profile(&profile)?;
    utils::success(&format!("已删除 key {}", key_ref));
    Ok(())
}

// ========== CLI probe failover ==========

fn profile_protocol_fields(
    profile: &ApiProfile,
) -> (Option<String>, Option<String>, Option<String>) {
    let wire = None;
    let mode = match profile.target_app {
        Some(TargetApp::Hermes) => profile.hermes.api_mode.clone(),
        Some(TargetApp::OpenClaw) => profile.openclaw.api_mode.clone(),
        _ => profile
            .hermes
            .api_mode
            .clone()
            .or_else(|| profile.openclaw.api_mode.clone()),
    };
    let exp = None;
    (wire, mode, exp)
}

fn model_for_probe(profile: &ApiProfile) -> String {
    profile
        .model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| {
            profile
                .opencode
                .models
                .as_ref()
                .and_then(|m| m.iter().map(|s| s.trim()).find(|s| !s.is_empty()))
                .map(|s| s.to_string())
        })
        .unwrap_or_default()
}

async fn cli_failover(profile: &mut ApiProfile, target: TargetApp) -> Result<bool> {
    use switch_api::probe::probe_with_params;
    if target == TargetApp::Codex
        && switch_api::adapters::codex::CodexAdapter::is_amazon_bedrock_profile(profile)
    {
        anyhow::bail!("Amazon Bedrock uses Codex built-in AWS authentication and cannot use HTTP key failover");
    }
    profile.normalize_keys();
    let model = model_for_probe(profile);
    if model.is_empty() {
        anyhow::bail!("先为该 Profile 填写默认模型再探活/failover");
    }
    let (wire, mode, exp) = profile_protocol_fields(profile);
    let mut keys = profile.api_keys.clone().unwrap_or_default();
    if keys.is_empty() {
        anyhow::bail!("没有可探活的 Key");
    }
    keys.sort_by_key(|e| if e.is_active { 0 } else { 1 });
    let now = chrono::Utc::now().timestamp();
    let app = target.as_str();
    for entry in keys.iter() {
        print!(
            "  试 {} … ",
            if entry.label.is_empty() {
                entry.id.as_str()
            } else {
                entry.label.as_str()
            }
        );
        match probe_with_params(ProbeRequest {
            target_app: app,
            api_url: &profile.api_url,
            api_key: &entry.key,
            model: &model,
            wire_api: wire.as_deref(),
            api_mode: mode.as_deref(),
            experimental_bearer_token: exp.as_deref(),
            key_label: Some(entry.label.clone()),
        })
        .await
        {
            Ok(ok) => {
                println!("OK ({})", ok.protocol);
                if let Some(list) = profile.api_keys.as_mut() {
                    for e in list.iter_mut() {
                        if e.id == entry.id {
                            e.last_probe_ok = Some(true);
                            e.last_probed_at = Some(now);
                        }
                    }
                }
                let _ = profile.set_active_key_id(&entry.id);
                return Ok(true);
            }
            Err(err) => {
                println!("FAIL");
                utils::warning(&err);
                if let Some(list) = profile.api_keys.as_mut() {
                    for e in list.iter_mut() {
                        if e.id == entry.id {
                            e.last_probe_ok = Some(false);
                            e.last_probed_at = Some(now);
                        }
                    }
                }
            }
        }
    }
    Ok(false)
}

fn cmd_profile_key_failover(db: &Database, name: String, target_app: Option<String>) -> Result<()> {
    let target = resolve_target_for_name(db, &name, target_app.as_deref())?;
    let mut profile = db.get_profile_by_name_and_target(&name, target)?;
    let rt = tokio::runtime::Runtime::new().context("创建 tokio runtime 失败")?;
    let ok = rt.block_on(cli_failover(&mut profile, target))?;
    db.update_profile(&profile)?;
    if ok {
        utils::success(&format!(
            "failover 成功，活跃 key = {}",
            profile
                .api_keys
                .as_ref()
                .and_then(|ks| ks.iter().find(|e| e.is_active))
                .map(|e| e.label.as_str())
                .unwrap_or("(active)")
        ));
        // if already active profile, re-switch
        if let Ok(Some(ap)) = db.get_active_profile(target) {
            if profile.id == Some(ap.profile_id) {
                utils::info("该 profile 已是当前活动配置，正在 re-switch 写入…");
                cmd_switch(db, target, name, true, false)?;
            }
        }
    } else {
        anyhow::bail!("所有 key 探活失败");
    }
    Ok(())
}

// ========== 工具函数 ==========

fn parse_target_app(s: &str) -> Result<TargetApp> {
    TargetApp::parse(s).ok_or_else(|| anyhow::anyhow!("未知的目标应用: {}", s))
}

fn default_db_path() -> PathBuf {
    let home = dirs::home_dir().expect("Failed to get home directory");
    home.join(".switch-api").join("db.sqlite")
}
