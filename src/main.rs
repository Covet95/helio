mod cli;

use anyhow::Result;
use clap::Parser;
use switch_api::utils;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = cli::Cli::parse();

    if let Err(e) = cli::execute(cli) {
        utils::error(&format!("{}", e));
        std::process::exit(1);
    }

    Ok(())
}
