use tokio::process::Command;

use super::Agent;

pub struct ClaudeCode;

impl Agent for ClaudeCode {
    fn name(&self) -> &str {
        "claude"
    }

    fn build_streaming_command(&self, prompt: &str) -> Command {
        let mut cmd = Command::new("claude");
        cmd.args([
            "-p",
            prompt,
            "--permission-mode",
            "bypassPermissions",
            "--output-format",
            "text",
        ]);
        cmd
    }

    fn build_oneshot_command(&self, prompt: &str) -> Command {
        let mut cmd = Command::new("claude");
        cmd.args(["-p", prompt, "--output-format", "text"]);
        cmd
    }
}
