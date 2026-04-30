use tokio::process::Command;

use super::Agent;
use super::event::{AgentEvent, Usage};

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
        "stream_event" => parse_stream_event(v.get("event")?),
        "user" => parse_user_message(v.get("message")?),
        "assistant" => parse_assistant_message(v.get("message")?),
        "result" => Some(AgentEvent::Stop {
            reason: v
                .get("subtype")
                .and_then(|s| s.as_str())
                .unwrap_or("end")
                .to_string(),
        }),
        _ => None,
    }
}

fn parse_stream_event(event: &serde_json::Value) -> Option<AgentEvent> {
    let kind = event.get("type")?.as_str()?;
    match kind {
        "message_start" => {
            let usage = event.get("message")?.get("usage")?;
            Some(AgentEvent::Usage(usage_from(usage)))
        }
        "content_block_start" => {
            let block = event.get("content_block")?;
            let block_type = block.get("type")?.as_str()?;
            match block_type {
                "tool_use" | "server_tool_use" => {
                    let tool = block
                        .get("name")
                        .and_then(|s| s.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    Some(AgentEvent::ToolStart {
                        tool,
                        summary: None,
                    })
                }
                _ => None,
            }
        }
        "content_block_delta" => None,
        "message_delta" => event.get("usage").map(|u| AgentEvent::Usage(usage_from(u))),
        "message_stop" => Some(AgentEvent::Stop {
            reason: "end".to_string(),
        }),
        _ => None,
    }
}

fn parse_user_message(message: &serde_json::Value) -> Option<AgentEvent> {
    let content = message.get("content")?.as_array()?;
    for item in content {
        if item.get("type").and_then(|t| t.as_str()) == Some("tool_result") {
            let is_error = item
                .get("is_error")
                .and_then(|b| b.as_bool())
                .unwrap_or(false);
            return Some(AgentEvent::ToolEnd {
                tool: "tool".to_string(),
                ok: !is_error,
            });
        }
    }
    None
}

fn parse_assistant_message(message: &serde_json::Value) -> Option<AgentEvent> {
    let content = message.get("content")?.as_array()?;
    for item in content {
        if item.get("type").and_then(|t| t.as_str()) == Some("text")
            && let Some(text) = item.get("text").and_then(|s| s.as_str())
        {
            return Some(AgentEvent::Message(text.to_string()));
        }
    }
    None
}

fn usage_from(v: &serde_json::Value) -> Usage {
    Usage {
        input_tokens: v.get("input_tokens").and_then(|n| n.as_u64()).unwrap_or(0),
        output_tokens: v.get("output_tokens").and_then(|n| n.as_u64()).unwrap_or(0),
        cached_input_tokens: v
            .get("cache_read_input_tokens")
            .and_then(|n| n.as_u64())
            .unwrap_or(0),
    }
}
