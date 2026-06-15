mod adapters;
mod cli;
mod db;
mod models;
mod utils;

use anyhow::Result;
use clap::Parser;

fn main() -> Result<()> {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // 解析命令行参数
    let cli = cli::Cli::parse();

    // 执行命令
    if let Err(e) = cli::execute(cli) {
        utils::error(&format!("{}", e));
        std::process::exit(1);
    }

    Ok(())
}
