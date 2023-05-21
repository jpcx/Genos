use anyhow::Result;
use config::{Cli, FromConfigFile, HwConfig};
use context::Context;
use tracing::error;

mod config;
mod context;
mod finder;
mod stage;

async fn run_grader(cli_config: Cli) -> Result<()> {
    let hw_config = HwConfig::from_file(&cli_config.config).await?;
    let context = Context::new(cli_config, hw_config).await;
    context.run_grader().await
}

#[tokio::main]
async fn main() {
    let cli_config: Cli = argh::from_env();

    if let Err(e) = run_grader(cli_config).await {
        error!("Error running grader: {e}");
    }
}
