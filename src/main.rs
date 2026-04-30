use anyhow::Result;
use clap::Parser;

mod agent;
mod cli;
mod commit;
mod config;
mod git;
mod logger;
mod prompt;
mod state;
mod ui;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = cli::Cli::parse();
    cli::dispatch(cli).await
}
