use switch_api::adapters::get_adapter;
use switch_api::db::Database;
use switch_api::models::{
    ApiKeyEntry, ApiProfile, HermesProfileFields, OpenClawProfileFields, TargetApp,
};
use switch_api::utils;
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use colored::Colorize;
use std::collections::HashMap;
use std::path::PathBuf;

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
        /// 目标应用 (claude-code, codex, gemini, opencode, hermes, openclaw)
        target_app: String,
    },

    /// 管理 API Profiles
    #[command(subcommand)]
    Profile(ProfileCommands),

    /// 切换 API Profile
    Switch {
        /// 目标应用 (claude-code, codex, gemini, opencode, hermes, openclaw)
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
        /// 目标应用 (claude-code, codex, gemini, opencode, hermes, openclaw)
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
        url: String,
        /// API Key
        #[arg(long)]
        key: String,
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
        /// 启用 1M 上下文窗口
        #[arg(long)]
        context_1m: bool,
        /// 协议模式（Hermes/OpenClaw：chat_completions / anthropic_messages / codex_responses）
        #[arg(long)]
        api_mode: Option<String>,
        /// OpenClaw models[].maxTokens
        #[arg(long)]
        max_tokens: Option<i64>,
        /// 目标应用 (claude-code, codex, gemini, opencode, hermes, openclaw)
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
        /// 目标应用 (claude-code, codex, gemini, opencode, hermes, openclaw)
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
        /// 目标应用 (claude-code, codex, gemini, opencode, hermes, openclaw)
        #[arg(long)]
        target_app: Option<String>,
    },

    /// 更新 Profile
    Update {
        /// Profile 名称
        name: String,
        /// 目标应用 (claude-code, codex, gemini, opencode, hermes, openclaw)
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
    let db = Database::open(&cli.db_path)?;

    match cli.command {
        Commands::Init { target_app } => {
            let target = parse_target_app(&target_app)?;
            cmd_init(&db, target)?;
        }
        Commands::Profile(profile_cmd) => match profile_cmd {
            ProfileCommands::Add {
                name,
                url,
                key,
                provider,
                model_mapping,
                model,
                reasoning_effort,
                context_1m,
                api_mode,
                max_tokens,
                target_app,
            } => {
                cmd_profile_add(
                    &db,
                    name,
                    url,
                    key,
                    provider,
                    model_mapping,
                    model,
                    reasoning_effort,
                    context_1m,
                    api_mode,
                    max_tokens,
                    target_app,
                )?;
            }
            ProfileCommands::List { verbose } => {
                cmd_profile_list(&db, verbose)?;
            }
            ProfileCommands::Delete { name, target_app, force } => {
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
                context_1m,
                api_mode,
                max_tokens,
            } => {
                cmd_profile_update(
                    &db,
                    name,
                    target_app,
                    url,
                    key,
                    provider,
                    model_mapping,
                    model,
                    reasoning_effort,
                    context_1m,
                    api_mode,
                    max_tokens,
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
            let target = parse_target_app(&target_app)?;
            cmd_sync(&db, target)?;
        }
        Commands::Export { output } => {
            cmd_export(&cli.db_path, output)?;
        }
        Commands::Import { input, force } => {
            cmd_import(input, &cli.db_path, force)?;
        }
    }

    Ok(())
}

// ========== 命令实现 ==========

fn cmd_init(db: &Database, target_app: TargetApp) -> Result<()> {
    utils::info(&format!("初始化 {} 配置...", target_app));

    let adapter = get_adapter(target_app);

    // 读取当前配置
    let config = adapter.read_config()?;

    // 提取共享配置
    let shared = adapter.extract_shared_config(&config);

    // 保存到数据库
    db.save_shared_config(target_app, shared)?;

    utils::success(&format!("已从 {} 导入共享配置", adapter.config_path().display()));
    utils::info("现在可以添加 API Profile:");
    println!("   switch-api profile add official --url https://api.anthropic.com --key sk-ant-xxx");

    Ok(())
}

fn cmd_profile_add(
    db: &Database,
    name: String,
    url: String,
    key: String,
    provider: String,
    model_mapping: Option<String>,
    model: Option<String>,
    reasoning_effort: Option<String>,
    context_1m: bool,
    api_mode: Option<String>,
    max_tokens: Option<i64>,
    target_app: Option<String>,
) -> Result<()> {
    let model_mapping_map = if let Some(json) = model_mapping {
        Some(serde_json::from_str::<HashMap<String, String>>(&json)
            .context("Invalid model mapping JSON")?)
    } else {
        None
    };

    let mut profile = ApiProfile::new(name.clone(), provider, url, key, model_mapping_map);
    profile.model = model.filter(|value| !value.trim().is_empty());
    profile.codex.reasoning_effort = reasoning_effort.filter(|value| !value.trim().is_empty());
    if context_1m {
        profile.context_1m = Some(true);
    }

    if let Some(t) = target_app.as_deref() {
        profile.target_app = Some(parse_target_app(t)?);
    }

    let mode = api_mode.filter(|v| !v.trim().is_empty());
    match profile.target_app {
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

fn cmd_profile_delete(db: &Database, name: String, target_app: Option<String>, force: bool) -> Result<()> {
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
        if crate::adapters::opencode::OpenCodeAdapter::delete_profile_and_cleanup_local(db, &name)?
        {
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
fn resolve_target_for_name(db: &Database, name: &str, target_app: Option<&str>) -> Result<TargetApp> {
    if let Some(s) = target_app {
        return parse_target_app(s);
    }
    let matches: Vec<TargetApp> = db.list_profiles()?
        .into_iter()
        .filter(|p| p.name == name)
        .filter_map(|p| p.target_app)
        .collect();
    match matches.as_slice() {
        [t] => Ok(*t),
        [] => Err(anyhow::anyhow!("未找到 Profile: {name}")),
        _ => Err(anyhow::anyhow!("存在多个同名 Profile「{name}」,请用 --target-app 指定工具")),
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
        println!("  1M Context: {}", if context_1m { "enabled" } else { "disabled" });
    }
    match profile.target_app {
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
        let dt = chrono::DateTime::from_timestamp(created_at, 0)
            .unwrap_or_default();
        println!("  Created: {}", dt.format("%Y-%m-%d %H:%M:%S"));
    }

    Ok(())
}

fn cmd_profile_update(
    db: &Database,
    name: String,
    target_app: Option<String>,
    url: Option<String>,
    key: Option<String>,
    provider: Option<String>,
    model_mapping: Option<String>,
    model: Option<String>,
    reasoning_effort: Option<String>,
    context_1m: Option<bool>,
    api_mode: Option<String>,
    max_tokens: Option<i64>,
) -> Result<()> {
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

    if let Some(new_context_1m) = context_1m {
        profile.context_1m = Some(new_context_1m);
        updated = true;
    }

    if let Some(mode) = api_mode {
        let mode = mode.trim().to_string();
        match target {
            TargetApp::Hermes => {
                profile.hermes.api_mode = if mode.is_empty() { None } else { Some(mode) };
                updated = true;
            }
            TargetApp::OpenClaw => {
                profile.openclaw.api_mode = if mode.is_empty() { None } else { Some(mode) };
                updated = true;
            }
            _ => {
                utils::warning("--api-mode 仅对 hermes / openclaw 生效，已忽略");
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

    db.update_profile(&profile)?;
    utils::success(&format!("已更新 Profile: {}", name));

    Ok(())
}

fn cmd_switch(db: &Database, target_app: TargetApp, profile_name: String, backup: bool, probe: bool) -> Result<()> {
    utils::info(&format!("切换 {} 到 Profile: {}...", target_app, profile_name));

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


    // 2. 获取适配器
    let adapter = get_adapter(target_app);

    // 3. 读取当前配置文件（如果存在）并提取共享部分
    let shared_config = if adapter.config_path().exists() {
        let current_config = adapter.read_config()?;
        let extracted = adapter.extract_shared_config(&current_config);

        // 同步到数据库（自动保存）
        db.save_shared_config(target_app, extracted.clone())?;

        extracted
    } else {
        // 如果配置文件不存在，从数据库获取
        db.get_shared_config(target_app)?
            .map(|c| c.config)
            .unwrap_or_else(|| serde_json::json!({}))
    };

    // 4. 备份现有配置
    if backup && adapter.config_path().exists() {
        let backup_path = adapter.backup_config()?;
        utils::success(&format!("已备份到: {}", backup_path.display()));
    }

    // 5. 合并配置（只替换 API 字段）
    let merged = adapter.merge_config(&api_profile, &shared_config);

    // 6. 写入配置
    adapter.write_config(&merged)?;

    // 6.5 应用工具特定的 API 凭据（如 Gemini 的 .env）
    adapter.apply_api_credentials(&api_profile)?;

    // 7. 更新活动记录
    db.set_active_profile(target_app, api_profile.id.unwrap())?;

    utils::success(&format!("已切换到 {}", profile_name));
    println!("  配置文件: {}", adapter.config_path().display());

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

fn cmd_export(db_path: &PathBuf, output: PathBuf) -> Result<()> {
    std::fs::copy(db_path, &output)?;

    let size = std::fs::metadata(&output)?.len();
    utils::success(&format!("已导出数据库到: {} ({})",
        output.display(),
        utils::format_size(size)
    ));

    Ok(())
}

fn cmd_import(input: PathBuf, db_path: &PathBuf, force: bool) -> Result<()> {
    // 检查输入文件是否存在
    if !input.exists() {
        anyhow::bail!("输入文件不存在: {}", input.display());
    }

    // 覆盖现有数据库前，先验证输入文件是合法 SQLite 库，
    // 避免选错文件（损坏/非数据库）时把现有数据覆盖掉才发现。
    Database::open(&input).map_err(|e| anyhow::anyhow!("输入文件不是合法的数据库: {}", e))?;

    if db_path.exists() {
        if !force && !utils::confirm("将覆盖现有数据库，是否继续？")? {
            utils::info("已取消");
            return Ok(());
        }

        // 备份带时间戳，保留历史备份不互相覆盖；即使 --force 也始终备份以便回退。
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let backup = db_path.with_file_name(format!("db.backup.{}.sqlite", timestamp));
        std::fs::copy(db_path, &backup)?;
        utils::success(&format!("已备份现有数据库到: {}", backup.display()));
    }

    std::fs::copy(&input, db_path)?;
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
    let keys = profile.api_keys.as_ref().map(|v| v.as_slice()).unwrap_or(&[]);
    if keys.is_empty() {
        println!("(无 key)");
        return Ok(());
    }
    println!(
        "\n{} {} 的 keys:\n",
        "Profile".bold(),
        name.cyan()
    );
    for e in keys {
        let mark = if e.is_active { "●" } else { "○" };
        let masked = if e.key.len() > 15 {
            format!("{}...{}", &e.key[..10], &e.key[e.key.len() - 5..])
        } else {
            "***".into()
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

fn profile_protocol_fields(profile: &ApiProfile) -> (Option<String>, Option<String>, Option<String>) {
    let wire = profile.codex.wire_api.clone();
    let mode = match profile.target_app {
        Some(TargetApp::Hermes) => profile.hermes.api_mode.clone(),
        Some(TargetApp::OpenClaw) => profile.openclaw.api_mode.clone(),
        _ => profile
            .hermes
            .api_mode
            .clone()
            .or_else(|| profile.openclaw.api_mode.clone()),
    };
    let exp = profile.codex.experimental_bearer_token.clone();
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
        print!("  试 {} … ", if entry.label.is_empty() { entry.id.as_str() } else { entry.label.as_str() });
        match probe_with_params(
            app,
            &profile.api_url,
            &entry.key,
            &model,
            wire.as_deref(),
            mode.as_deref(),
            exp.as_deref(),
            Some(entry.label.clone()),
        )
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
    TargetApp::from_str(s).ok_or_else(|| anyhow::anyhow!("未知的目标应用: {}", s))
}

fn default_db_path() -> PathBuf {
    let home = dirs::home_dir().expect("Failed to get home directory");
    home.join(".switch-api").join("db.sqlite")
}
