use anyhow::{Result, bail};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::logger::LogSink;

mod claude_code;
mod codex;
mod event;
mod stream;

pub use event::{AgentEvent, Usage};
pub use stream::{AgentOutcome, strip_code_fence};

pub trait Agent: Send + Sync {
    fn name(&self) -> &str;
    fn build_streaming_command(&self, prompt: &str) -> tokio::process::Command;
    fn build_oneshot_command(&self, prompt: &str) -> tokio::process::Command;
    /// One stdout line may produce zero, one, or several events.
    fn parse_event(&self, line: &str) -> Vec<AgentEvent>;
}

pub fn from_name(name: &str) -> Result<Box<dyn Agent>> {
    match name {
        "claude" | "claude-code" => Ok(Box::new(claude_code::ClaudeCode::new())),
        "codex" => Ok(Box::new(codex::Codex)),
        other => bail!("unknown agent `{other}` — supported: claude, claude-code, codex"),
    }
}

pub async fn run(
    agent: &dyn Agent,
    repo: &Path,
    prompt: &str,
    sink: &LogSink,
    cancelled: Arc<AtomicBool>,
) -> Result<AgentOutcome> {
    let mut cmd = agent.build_streaming_command(prompt);
    cmd.current_dir(repo);
    stream::run_streamed(cmd, agent, sink, cancelled).await
}

pub async fn run_oneshot(agent: &dyn Agent, repo: &Path, prompt: &str) -> Result<String> {
    let mut cmd = agent.build_oneshot_command(prompt);
    cmd.current_dir(repo);
    stream::run_oneshot(cmd, agent.name()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_name_returns_claude_code_for_claude_alias() {
        let agent = from_name("claude").expect("claude alias should resolve");
        assert_eq!(agent.name(), "claude");
    }

    #[test]
    fn from_name_returns_claude_code_for_full_name() {
        let agent = from_name("claude-code").expect("claude-code should resolve");
        assert_eq!(agent.name(), "claude");
    }

    #[test]
    fn from_name_returns_codex() {
        let agent = from_name("codex").expect("codex should resolve");
        assert_eq!(agent.name(), "codex");
    }

    #[test]
    fn from_name_errors_for_unknown_agent() {
        let err = from_name("ghost")
            .err()
            .expect("unknown agent should error");
        let msg = err.to_string();
        assert!(
            msg.contains("ghost"),
            "error should mention the bad name: {msg}"
        );
        assert!(
            msg.contains("claude") && msg.contains("codex"),
            "error should list supported agents: {msg}",
        );
    }

    #[test]
    fn from_name_errors_for_empty_name() {
        assert!(from_name("").is_err());
    }
}
