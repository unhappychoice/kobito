use tokio::process::Command;

use super::Agent;
use super::event::AgentEvent;

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
            "--json",
            prompt,
        ]);
        cmd
    }

    fn build_oneshot_command(&self, prompt: &str) -> Command {
        let mut cmd = Command::new("codex");
        cmd.args(["exec", "--color", "never", prompt]);
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
        "item.completed" => {
            let item = v.get("item")?;
            let item_type = item.get("type")?.as_str()?;
            match item_type {
                "agent_message" => {
                    let text = item.get("text")?.as_str()?.to_string();
                    Some(AgentEvent::Message(text))
                }
                "command_execution" | "tool_call" | "function_call" => {
                    let tool = item
                        .get("name")
                        .or_else(|| item.get("command"))
                        .and_then(|s| s.as_str())
                        .unwrap_or(item_type)
                        .to_string();
                    let summary = item
                        .get("command")
                        .and_then(|s| s.as_str())
                        .map(|s| s.to_string());
                    Some(AgentEvent::ToolEnd {
                        tool: format!("{tool}: {}", summary.clone().unwrap_or_default())
                            .trim_end_matches(": ")
                            .to_string(),
                        ok: item
                            .get("exit_code")
                            .and_then(|n| n.as_i64())
                            .map(|c| c == 0)
                            .unwrap_or(true),
                    })
                }
                _ => None,
            }
        }
        "item.started" => {
            let item = v.get("item")?;
            let item_type = item.get("type")?.as_str()?;
            if matches!(
                item_type,
                "command_execution" | "tool_call" | "function_call"
            ) {
                let tool = item
                    .get("name")
                    .or_else(|| item.get("command"))
                    .and_then(|s| s.as_str())
                    .unwrap_or(item_type)
                    .to_string();
                Some(AgentEvent::ToolStart {
                    tool,
                    summary: item
                        .get("command")
                        .and_then(|s| s.as_str())
                        .map(|s| s.to_string()),
                })
            } else {
                None
            }
        }
        "turn.completed" => {
            let usage = v.get("usage")?;
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
                    .get("cached_input_tokens")
                    .and_then(|n| n.as_u64())
                    .unwrap_or(0),
            }))
        }
        _ => None,
    }
}
