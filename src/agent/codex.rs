use tokio::process::Command;

use super::Agent;

pub struct Codex;

impl Agent for Codex {
    fn name(&self) -> &str {
        "codex"
    }

    fn build_streaming_command(&self, prompt: &str) -> Command {
        let mut cmd = Command::new("codex");
        cmd.args([
            "exec",
            "--dangerously-bypass-approvals-and-sandbox",
            "--color",
            "never",
            prompt,
        ]);
        cmd
    }

    fn build_oneshot_command(&self, prompt: &str) -> Command {
        let mut cmd = Command::new("codex");
        cmd.args(["exec", "--color", "never", prompt]);
        cmd
    }
}
