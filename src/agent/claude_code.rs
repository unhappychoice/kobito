use std::collections::HashMap;
use std::sync::Mutex;
use tokio::process::Command;

use super::Agent;
use super::event::{AgentEvent, Usage};

/// Claude Code's `--output-format stream-json` emits `assistant` /
/// `user` / `result` envelope lines. tool_use entries inside an
/// `assistant` message carry a unique id, and the matching
/// `tool_result` inside the next `user` message references that id —
/// not the tool name. We track the mapping here so a tool_result can
/// be reported with the original tool name.
pub struct ClaudeCode {
    tools: Mutex<HashMap<String, String>>,
}

impl Default for ClaudeCode {
    fn default() -> Self {
        Self {
            tools: Mutex::new(HashMap::new()),
        }
    }
}

impl ClaudeCode {
    pub fn new() -> Self {
        Self::default()
    }

    fn remember_tool(&self, id: &str, name: &str) {
        if let Ok(mut m) = self.tools.lock() {
            m.insert(id.to_string(), name.to_string());
        }
    }

    fn resolve_tool(&self, id: &str) -> String {
        // Remove rather than read so the map doesn't grow unbounded
        // across iterations and a stale tool_use_id can't be matched
        // against a freshly issued one with the same suffix.
        if let Ok(mut m) = self.tools.lock() {
            m.remove(id).unwrap_or_else(|| "tool".to_string())
        } else {
            "tool".to_string()
        }
    }
}

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

    fn parse_event(&self, line: &str) -> Vec<AgentEvent> {
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => return vec![AgentEvent::Other(line.to_string())],
        };
        let kind = match v.get("type").and_then(|t| t.as_str()) {
            Some(k) => k,
            None => return vec![],
        };
        match kind {
            "assistant" => self.parse_assistant(v.get("message")),
            "user" => self.parse_user(v.get("message")),
            "result" => parse_result(&v),
            "stream_event" => parse_stream_event(v.get("event")),
            _ => vec![],
        }
    }
}

impl ClaudeCode {
    fn parse_assistant(&self, message: Option<&serde_json::Value>) -> Vec<AgentEvent> {
        let content = match message
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
        {
            Some(c) => c,
            None => return vec![],
        };
        let mut out = vec![];
        for item in content {
            let item_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match item_type {
                "text" => {
                    if let Some(text) = item.get("text").and_then(|s| s.as_str())
                        && !text.trim().is_empty()
                    {
                        out.push(AgentEvent::Message(text.to_string()));
                    }
                }
                "tool_use" => {
                    let id = item
                        .get("id")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_string();
                    let tool = item
                        .get("name")
                        .and_then(|s| s.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    if !id.is_empty() {
                        self.remember_tool(&id, &tool);
                    }
                    let summary = summarize_tool_input(item.get("input"));
                    out.push(AgentEvent::ToolStart { tool, summary });
                }
                _ => {}
            }
        }
        out
    }

    fn parse_user(&self, message: Option<&serde_json::Value>) -> Vec<AgentEvent> {
        let content = match message
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
        {
            Some(c) => c,
            None => return vec![],
        };
        let mut out = vec![];
        for item in content {
            if item.get("type").and_then(|t| t.as_str()) == Some("tool_result") {
                let id = item
                    .get("tool_use_id")
                    .and_then(|s| s.as_str())
                    .unwrap_or("");
                let tool = self.resolve_tool(id);
                let is_error = item
                    .get("is_error")
                    .and_then(|b| b.as_bool())
                    .unwrap_or(false);
                out.push(AgentEvent::ToolEnd {
                    tool,
                    ok: !is_error,
                });
            }
        }
        out
    }
}

fn parse_result(v: &serde_json::Value) -> Vec<AgentEvent> {
    let mut out = vec![];
    if let Some(usage) = v.get("usage") {
        out.push(AgentEvent::Usage(usage_from(usage)));
    }
    let reason = v
        .get("subtype")
        .and_then(|s| s.as_str())
        .unwrap_or("end")
        .to_string();
    out.push(AgentEvent::Stop { reason });
    out
}

fn parse_stream_event(event: Option<&serde_json::Value>) -> Vec<AgentEvent> {
    let event = match event {
        Some(e) => e,
        None => return vec![],
    };
    let kind = match event.get("type").and_then(|t| t.as_str()) {
        Some(k) => k,
        None => return vec![],
    };
    match kind {
        "message_start" => event
            .get("message")
            .and_then(|m| m.get("usage"))
            .map(|u| vec![AgentEvent::Usage(usage_from(u))])
            .unwrap_or_default(),
        "message_delta" => event
            .get("usage")
            .map(|u| vec![AgentEvent::Usage(usage_from(u))])
            .unwrap_or_default(),
        _ => vec![],
    }
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

fn summarize_tool_input(input: Option<&serde_json::Value>) -> Option<String> {
    let input = input?;
    for key in [
        "command",
        "file_path",
        "path",
        "pattern",
        "url",
        "query",
        "description",
    ] {
        if let Some(s) = input.get(key).and_then(|v| v.as_str()) {
            let trimmed: String = s.chars().take(80).collect();
            return Some(trimmed.replace('\n', " "));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_one(agent: &ClaudeCode, line: &str) -> Vec<AgentEvent> {
        agent.parse_event(line)
    }

    #[test]
    fn name_returns_claude() {
        assert_eq!(ClaudeCode::new().name(), "claude");
    }

    #[test]
    fn streaming_command_uses_stream_json_with_bypass() {
        let agent = ClaudeCode::new();
        let cmd = agent.build_streaming_command("hi");
        let std_cmd = cmd.as_std();
        assert_eq!(std_cmd.get_program(), "claude");
        let args: Vec<&str> = std_cmd.get_args().filter_map(|a| a.to_str()).collect();
        assert_eq!(
            args,
            vec![
                "-p",
                "hi",
                "--permission-mode",
                "bypassPermissions",
                "--output-format",
                "stream-json",
                "--verbose",
            ]
        );
    }

    #[test]
    fn oneshot_command_uses_text_output() {
        let agent = ClaudeCode::new();
        let cmd = agent.build_oneshot_command("hello");
        let std_cmd = cmd.as_std();
        assert_eq!(std_cmd.get_program(), "claude");
        let args: Vec<&str> = std_cmd.get_args().filter_map(|a| a.to_str()).collect();
        assert_eq!(args, vec!["-p", "hello", "--output-format", "text"]);
    }

    #[test]
    fn parse_event_returns_other_for_invalid_json() {
        let agent = ClaudeCode::new();
        let evs = parse_one(&agent, "not json");
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            AgentEvent::Other(s) => assert_eq!(s, "not json"),
            other => panic!("expected Other, got {other:?}"),
        }
    }

    #[test]
    fn parse_event_returns_empty_when_type_missing() {
        let agent = ClaudeCode::new();
        assert!(parse_one(&agent, r#"{"foo":1}"#).is_empty());
    }

    #[test]
    fn parse_event_returns_empty_for_unknown_kind() {
        let agent = ClaudeCode::new();
        assert!(parse_one(&agent, r#"{"type":"mystery"}"#).is_empty());
    }

    #[test]
    fn assistant_text_emits_message_event() {
        let agent = ClaudeCode::new();
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi there"}]}}"#;
        let evs = parse_one(&agent, line);
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            AgentEvent::Message(m) => assert_eq!(m, "hi there"),
            other => panic!("expected Message, got {other:?}"),
        }
    }

    #[test]
    fn assistant_blank_text_is_skipped() {
        let agent = ClaudeCode::new();
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"   "}]}}"#;
        assert!(parse_one(&agent, line).is_empty());
    }

    #[test]
    fn assistant_tool_use_emits_tool_start_with_summary() {
        let agent = ClaudeCode::new();
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls -al"}}]}}"#;
        let evs = parse_one(&agent, line);
        match &evs[0] {
            AgentEvent::ToolStart { tool, summary } => {
                assert_eq!(tool, "Bash");
                assert_eq!(summary.as_deref(), Some("ls -al"));
            }
            other => panic!("expected ToolStart, got {other:?}"),
        }
    }

    #[test]
    fn assistant_tool_use_without_input_has_no_summary() {
        let agent = ClaudeCode::new();
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"Read"}]}}"#;
        let evs = parse_one(&agent, line);
        match &evs[0] {
            AgentEvent::ToolStart { tool, summary } => {
                assert_eq!(tool, "Read");
                assert!(summary.is_none());
            }
            other => panic!("expected ToolStart, got {other:?}"),
        }
    }

    #[test]
    fn assistant_tool_use_missing_name_falls_back_to_unknown() {
        let agent = ClaudeCode::new();
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"x"}]}}"#;
        let evs = parse_one(&agent, line);
        match &evs[0] {
            AgentEvent::ToolStart { tool, .. } => assert_eq!(tool, "unknown"),
            other => panic!("expected ToolStart, got {other:?}"),
        }
    }

    #[test]
    fn assistant_skips_unknown_content_item_types() {
        let agent = ClaudeCode::new();
        let line =
            r#"{"type":"assistant","message":{"content":[{"type":"image","data":"abc"}]}}"#;
        assert!(parse_one(&agent, line).is_empty());
    }

    #[test]
    fn assistant_returns_empty_when_content_missing() {
        let agent = ClaudeCode::new();
        let line = r#"{"type":"assistant","message":{}}"#;
        assert!(parse_one(&agent, line).is_empty());
    }

    #[test]
    fn user_tool_result_resolves_remembered_tool_name() {
        let agent = ClaudeCode::new();
        let start = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"abc","name":"Read","input":{"file_path":"/etc/hosts"}}]}}"#;
        let _ = parse_one(&agent, start);
        let end = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"abc"}]}}"#;
        let evs = parse_one(&agent, end);
        match &evs[0] {
            AgentEvent::ToolEnd { tool, ok } => {
                assert_eq!(tool, "Read");
                assert!(*ok);
            }
            other => panic!("expected ToolEnd, got {other:?}"),
        }
    }

    #[test]
    fn user_tool_result_marks_error_when_is_error_true() {
        let agent = ClaudeCode::new();
        let line = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"unknown","is_error":true}]}}"#;
        let evs = parse_one(&agent, line);
        match &evs[0] {
            AgentEvent::ToolEnd { tool, ok } => {
                assert_eq!(tool, "tool");
                assert!(!ok);
            }
            other => panic!("expected ToolEnd, got {other:?}"),
        }
    }

    #[test]
    fn user_tool_result_id_is_consumed_on_resolve() {
        let agent = ClaudeCode::new();
        let start = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"once","name":"Grep"}]}}"#;
        let _ = parse_one(&agent, start);
        let end = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"once"}]}}"#;
        let _ = parse_one(&agent, end);
        let again = parse_one(&agent, end);
        match &again[0] {
            AgentEvent::ToolEnd { tool, .. } => assert_eq!(tool, "tool"),
            other => panic!("expected ToolEnd, got {other:?}"),
        }
    }

    #[test]
    fn user_returns_empty_when_content_missing() {
        let agent = ClaudeCode::new();
        let line = r#"{"type":"user","message":{}}"#;
        assert!(parse_one(&agent, line).is_empty());
    }

    #[test]
    fn user_skips_non_tool_result_items() {
        let agent = ClaudeCode::new();
        let line = r#"{"type":"user","message":{"content":[{"type":"text","text":"hi"}]}}"#;
        assert!(parse_one(&agent, line).is_empty());
    }

    #[test]
    fn result_emits_usage_then_stop() {
        let agent = ClaudeCode::new();
        let line = r#"{"type":"result","subtype":"success","usage":{"input_tokens":10,"output_tokens":20,"cache_read_input_tokens":3}}"#;
        let evs = parse_one(&agent, line);
        assert_eq!(evs.len(), 2);
        match &evs[0] {
            AgentEvent::Usage(u) => {
                assert_eq!(u.input_tokens, 10);
                assert_eq!(u.output_tokens, 20);
                assert_eq!(u.cached_input_tokens, 3);
            }
            other => panic!("expected Usage, got {other:?}"),
        }
        match &evs[1] {
            AgentEvent::Stop { reason } => assert_eq!(reason, "success"),
            other => panic!("expected Stop, got {other:?}"),
        }
    }

    #[test]
    fn result_without_usage_emits_only_stop_with_default_reason() {
        let agent = ClaudeCode::new();
        let line = r#"{"type":"result"}"#;
        let evs = parse_one(&agent, line);
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            AgentEvent::Stop { reason } => assert_eq!(reason, "end"),
            other => panic!("expected Stop, got {other:?}"),
        }
    }

    #[test]
    fn stream_event_message_start_emits_usage_from_message() {
        let agent = ClaudeCode::new();
        let line = r#"{"type":"stream_event","event":{"type":"message_start","message":{"usage":{"input_tokens":7}}}}"#;
        let evs = parse_one(&agent, line);
        match &evs[0] {
            AgentEvent::Usage(u) => assert_eq!(u.input_tokens, 7),
            other => panic!("expected Usage, got {other:?}"),
        }
    }

    #[test]
    fn stream_event_message_delta_emits_usage_directly() {
        let agent = ClaudeCode::new();
        let line = r#"{"type":"stream_event","event":{"type":"message_delta","usage":{"output_tokens":42}}}"#;
        let evs = parse_one(&agent, line);
        match &evs[0] {
            AgentEvent::Usage(u) => assert_eq!(u.output_tokens, 42),
            other => panic!("expected Usage, got {other:?}"),
        }
    }

    #[test]
    fn stream_event_unknown_subtype_returns_empty() {
        let agent = ClaudeCode::new();
        let line = r#"{"type":"stream_event","event":{"type":"content_block_stop"}}"#;
        assert!(parse_one(&agent, line).is_empty());
    }

    #[test]
    fn stream_event_without_inner_event_returns_empty() {
        let agent = ClaudeCode::new();
        assert!(parse_one(&agent, r#"{"type":"stream_event"}"#).is_empty());
    }

    #[test]
    fn stream_event_message_start_without_usage_returns_empty() {
        let agent = ClaudeCode::new();
        let line =
            r#"{"type":"stream_event","event":{"type":"message_start","message":{}}}"#;
        assert!(parse_one(&agent, line).is_empty());
    }

    #[test]
    fn summarize_tool_input_picks_first_known_key_and_replaces_newlines() {
        let v = serde_json::json!({"file_path": "a\nb"});
        assert_eq!(summarize_tool_input(Some(&v)).as_deref(), Some("a b"));
    }

    #[test]
    fn summarize_tool_input_truncates_to_80_chars() {
        let long = "x".repeat(200);
        let v = serde_json::json!({"command": long});
        let summary = summarize_tool_input(Some(&v)).expect("should produce summary");
        assert_eq!(summary.chars().count(), 80);
    }

    #[test]
    fn summarize_tool_input_returns_none_for_missing_input() {
        assert!(summarize_tool_input(None).is_none());
    }

    #[test]
    fn summarize_tool_input_returns_none_when_no_known_key_present() {
        let v = serde_json::json!({"unrelated": "value"});
        assert!(summarize_tool_input(Some(&v)).is_none());
    }
}
