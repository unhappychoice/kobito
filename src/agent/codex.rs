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

    fn parse_event(&self, line: &str) -> Vec<AgentEvent> {
        parse(line).map(|e| vec![e]).unwrap_or_default()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_one(line: &str) -> Vec<AgentEvent> {
        Codex.parse_event(line)
    }

    #[test]
    fn name_returns_codex() {
        assert_eq!(Codex.name(), "codex");
    }

    #[test]
    fn streaming_command_uses_codex_exec_json() {
        let cmd = Codex.build_streaming_command("hi");
        let std_cmd = cmd.as_std();
        assert_eq!(std_cmd.get_program(), "codex");
        let args: Vec<&str> = std_cmd.get_args().filter_map(|a| a.to_str()).collect();
        assert_eq!(
            args,
            vec![
                "exec",
                "--dangerously-bypass-approvals-and-sandbox",
                "--json",
                "hi"
            ]
        );
    }

    #[test]
    fn oneshot_command_uses_codex_exec_text() {
        let cmd = Codex.build_oneshot_command("hello");
        let std_cmd = cmd.as_std();
        assert_eq!(std_cmd.get_program(), "codex");
        let args: Vec<&str> = std_cmd.get_args().filter_map(|a| a.to_str()).collect();
        assert_eq!(args, vec!["exec", "--color", "never", "hello"]);
    }

    #[test]
    fn parse_event_returns_empty_for_invalid_json() {
        assert!(parse_one("not json").is_empty());
    }

    #[test]
    fn parse_event_returns_empty_when_type_field_missing() {
        assert!(parse_one("{\"foo\":1}").is_empty());
    }

    #[test]
    fn parse_event_returns_empty_for_unknown_type() {
        assert!(parse_one("{\"type\":\"some.other\"}").is_empty());
    }

    #[test]
    fn item_completed_agent_message_emits_message() {
        let line =
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"hello there"}}"#;
        let evs = parse_one(line);
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            AgentEvent::Message(m) => assert_eq!(m, "hello there"),
            other => panic!("expected Message, got {other:?}"),
        }
    }

    #[test]
    fn item_completed_agent_message_without_text_returns_empty() {
        let line = r#"{"type":"item.completed","item":{"type":"agent_message"}}"#;
        assert!(parse_one(line).is_empty());
    }

    #[test]
    fn item_completed_command_execution_emits_tool_end_ok_when_exit_zero() {
        let line = r#"{"type":"item.completed","item":{"type":"command_execution","command":"ls -al","exit_code":0}}"#;
        let evs = parse_one(line);
        match &evs[0] {
            AgentEvent::ToolEnd { tool, ok } => {
                assert!(tool.starts_with("ls -al"));
                assert!(*ok);
            }
            other => panic!("expected ToolEnd, got {other:?}"),
        }
    }

    #[test]
    fn item_completed_command_execution_marks_failure_on_nonzero_exit() {
        let line = r#"{"type":"item.completed","item":{"type":"command_execution","command":"false","exit_code":1}}"#;
        let evs = parse_one(line);
        match &evs[0] {
            AgentEvent::ToolEnd { ok, .. } => assert!(!ok),
            other => panic!("expected ToolEnd, got {other:?}"),
        }
    }

    #[test]
    fn item_completed_tool_call_defaults_ok_true_when_exit_code_missing() {
        let line = r#"{"type":"item.completed","item":{"type":"tool_call","name":"shell"}}"#;
        let evs = parse_one(line);
        match &evs[0] {
            AgentEvent::ToolEnd { tool, ok } => {
                assert_eq!(tool, "shell");
                assert!(*ok);
            }
            other => panic!("expected ToolEnd, got {other:?}"),
        }
    }

    #[test]
    fn item_completed_function_call_falls_back_to_item_type_for_tool_label() {
        let line = r#"{"type":"item.completed","item":{"type":"function_call"}}"#;
        let evs = parse_one(line);
        match &evs[0] {
            AgentEvent::ToolEnd { tool, .. } => assert_eq!(tool, "function_call"),
            other => panic!("expected ToolEnd, got {other:?}"),
        }
    }

    #[test]
    fn item_completed_unknown_item_type_returns_empty() {
        let line = r#"{"type":"item.completed","item":{"type":"weird"}}"#;
        assert!(parse_one(line).is_empty());
    }

    #[test]
    fn item_started_command_execution_emits_tool_start_with_summary() {
        let line = r#"{"type":"item.started","item":{"type":"command_execution","name":"shell","command":"echo hi"}}"#;
        let evs = parse_one(line);
        match &evs[0] {
            AgentEvent::ToolStart { tool, summary } => {
                assert_eq!(tool, "shell");
                assert_eq!(summary.as_deref(), Some("echo hi"));
            }
            other => panic!("expected ToolStart, got {other:?}"),
        }
    }

    #[test]
    fn item_started_tool_call_falls_back_to_command_when_no_name() {
        let line = r#"{"type":"item.started","item":{"type":"tool_call","command":"cat README"}}"#;
        let evs = parse_one(line);
        match &evs[0] {
            AgentEvent::ToolStart { tool, summary } => {
                assert_eq!(tool, "cat README");
                assert_eq!(summary.as_deref(), Some("cat README"));
            }
            other => panic!("expected ToolStart, got {other:?}"),
        }
    }

    #[test]
    fn item_started_for_non_tool_item_returns_empty() {
        let line = r#"{"type":"item.started","item":{"type":"agent_message"}}"#;
        assert!(parse_one(line).is_empty());
    }

    #[test]
    fn turn_completed_emits_usage_with_all_fields() {
        let line = r#"{"type":"turn.completed","usage":{"input_tokens":12,"output_tokens":34,"cached_input_tokens":5}}"#;
        let evs = parse_one(line);
        match &evs[0] {
            AgentEvent::Usage(u) => {
                assert_eq!(u.input_tokens, 12);
                assert_eq!(u.output_tokens, 34);
                assert_eq!(u.cached_input_tokens, 5);
            }
            other => panic!("expected Usage, got {other:?}"),
        }
    }

    #[test]
    fn turn_completed_defaults_missing_usage_fields_to_zero() {
        let line = r#"{"type":"turn.completed","usage":{}}"#;
        let evs = parse_one(line);
        match &evs[0] {
            AgentEvent::Usage(u) => {
                assert_eq!(u.input_tokens, 0);
                assert_eq!(u.output_tokens, 0);
                assert_eq!(u.cached_input_tokens, 0);
            }
            other => panic!("expected Usage, got {other:?}"),
        }
    }

    #[test]
    fn turn_completed_without_usage_returns_empty() {
        let line = r#"{"type":"turn.completed"}"#;
        assert!(parse_one(line).is_empty());
    }
}
