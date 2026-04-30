use anyhow::{Result, bail};
use std::path::Path;

use crate::logger::LogSink;

mod claude_code;
mod codex;
mod stream;

pub use stream::AgentOutcome;

pub trait Agent: Send + Sync {
    fn name(&self) -> &str;
    fn build_streaming_command(&self, prompt: &str) -> tokio::process::Command;
    fn build_oneshot_command(&self, prompt: &str) -> tokio::process::Command;
}

pub fn from_name(name: &str) -> Result<Box<dyn Agent>> {
    match name {
        "claude" | "claude-code" => Ok(Box::new(claude_code::ClaudeCode)),
        "codex" => Ok(Box::new(codex::Codex)),
        other => bail!("unknown agent `{other}` — supported: claude, claude-code, codex"),
    }
}

pub async fn run(
    agent: &dyn Agent,
    repo: &Path,
    prompt: &str,
    sink: &LogSink,
) -> Result<AgentOutcome> {
    let mut cmd = agent.build_streaming_command(prompt);
    cmd.current_dir(repo);
    stream::run_streamed(cmd, agent.name(), sink).await
}

pub async fn run_oneshot(agent: &dyn Agent, repo: &Path, prompt: &str) -> Result<String> {
    let mut cmd = agent.build_oneshot_command(prompt);
    cmd.current_dir(repo);
    stream::run_oneshot(cmd, agent.name()).await
}
