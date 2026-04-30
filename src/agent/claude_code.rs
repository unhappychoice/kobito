use tokio::process::Command;

use super::Agent;
use super::event::AgentEvent;

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
            "stream-json",
            "--include-partial-messages",
            "--verbose",
        ]);
        cmd
    }

    fn build_oneshot_command(&self, prompt: &str) -> Command {
        let mut cmd = Command::new("claude");
        cmd.args(["-p", prompt, "--output-format", "text"]);
        cmd
    }

    fn parse_event(&self, line: &str) -> AgentEvent {
        parse(line).unwrap_or_else(|| AgentEvent::Other(line.to_string()))
    }
}

fn parse(line: &str) -> Option<AgentEvent> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let kind = v.get("type")?.as_str()?;
    match kind {
        "message_start" => {
            let usage = v.get("message")?.get("usage")?;
            Some(AgentEvent::Usage(super::event::Usage {
                input_tokens: usage
                    .get("input_tokens")
                    .and_then(|n| n.as_u64())
                    .unwrap_or(0),
                output_tokens: usage
                    .get("output_tokens")
                    .and_then(|n| n.as_u64())
                    .unwrap_or(0),
                cached_input_tokens: usage
                    .get("cache_read_input_tokens")
                    .and_then(|n| n.as_u64())
                    .unwrap_or(0),
            }))
        }
        "content_block_start" => {
            let block = v.get("content_block")?;
            let block_type = block.get("type")?.as_str()?;
            if block_type == "tool_use" || block_type == "server_tool_use" {
                let tool = block
                    .get("name")
                    .and_then(|s| s.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                Some(AgentEvent::ToolStart {
                    tool,
                    summary: None,
                })
            } else {
                None
            }
        }
        "content_block_delta" => {
            let delta = v.get("delta")?;
            let delta_type = delta.get("type")?.as_str()?;
            if delta_type == "text_delta" {
                let text = delta.get("text")?.as_str()?.to_string();
                Some(AgentEvent::Message(text))
            } else {
                None
            }
        }
        "message_delta" => {
            let usage = v.get("usage")?;
            let output_tokens = usage.get("output_tokens").and_then(|n| n.as_u64())?;
            Some(AgentEvent::Usage(super::event::Usage {
                input_tokens: 0,
                output_tokens,
                cached_input_tokens: 0,
            }))
        }
        "message_stop" => Some(AgentEvent::Stop {
            reason: "end".to_string(),
        }),
        _ => None,
    }
}
