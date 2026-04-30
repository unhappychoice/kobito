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
        self.tools
            .lock()
            .ok()
            .and_then(|m| m.get(id).cloned())
            .unwrap_or_else(|| "tool".to_string())
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
