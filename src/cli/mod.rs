use switch_api::adapters::get_adapter;
use switch_api::db::Database;
use switch_api::models::{ApiProfile, ClaudeProfileFields, CodexProfileFields, OpenCodeProfileFields, TargetApp};
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
        /// 目标应用 (claude-code, codex, gemini, opencode)
        target_app: String,
    },

    /// 管理 API Profiles
    #[command(subcommand)]
    Profile(ProfileCommands),

    /// 切换 API Profile
    Switch {
        /// 目标应用 (claude-code, codex, gemini, opencode)
        target_app: String,
        /// Profile 名称
        profile_name: String,
        /// 跳过备份
        #[arg(long)]
        no_backup: bool,
    },

    /// 查看当前状态
    Status {
        /// 显示详细信息
        #[arg(short, long)]
        verbose: bool,
    },

    /// 同步共享配置（从配置文件回填到数据库）
    Sync {
        /// 目标应用 (claude-code, codex, gemini, opencode)
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
        /// 目标应用 (claude-code, codex, gemini, opencode)
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
        /// 目标应用 (claude-code, codex, gemini, opencode)
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
        /// 目标应用 (claude-code, codex, gemini, opencode)
        #[arg(long)]
        target_app: Option<String>,
    },

    /// 更新 Profile
    Update {
        /// Profile 名称
        name: String,
        /// 目标应用 (claude-code, codex, gemini, opencode)
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
                )?;
            }
        },
        Commands::Switch {
            target_app,
            profile_name,
            no_backup,
        } => {
            let target = parse_target_app(&target_app)?;
            cmd_switch(&db, target, profile_name, !no_backup)?;
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

    if !updated {
        utils::warning("没有提供任何更新");
        return Ok(());
    }

    db.update_profile(&profile)?;
    utils::success(&format!("已更新 Profile: {}", name));

    Ok(())
}

fn cmd_switch(db: &Database, target_app: TargetApp, profile_name: String, backup: bool) -> Result<()> {
    utils::info(&format!("切换 {} 到 Profile: {}...", target_app, profile_name));

    // 1. 获取 API Profile
    let api_profile = db.get_profile_by_name_and_target(&profile_name, target_app)?;

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

// ========== 工具函数 ==========

fn parse_target_app(s: &str) -> Result<TargetApp> {
    TargetApp::from_str(s).ok_or_else(|| anyhow::anyhow!("未知的目标应用: {}", s))
}

fn default_db_path() -> PathBuf {
    let home = dirs::home_dir().expect("Failed to get home directory");
    home.join(".switch-api").join("db.sqlite")
}
