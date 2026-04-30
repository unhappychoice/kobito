use anyhow::Result;
use clap::Parser;

mod cli;
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
