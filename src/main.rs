use anyhow::Result;
use clap::Parser;

mod agent;
mod branch;
mod cli;
mod commit;
mod git;
mod iteration;
mod logger;
mod preset;
mod prompt;
mod runner;
mod state;
mod tasks;
mod ui;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = cli::Cli::parse();
    cli::dispatch(cli).await
}
