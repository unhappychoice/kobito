/// A normalized event produced by an agent's streaming output.
///
/// Each `Agent` impl maps its native streaming format (Claude Code's
/// `stream-json`, Codex's `--json`, …) into this enum so the rest of
/// kobito can render and persist agent activity uniformly.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum AgentEvent {
    /// A piece of agent-emitted text. May arrive as a complete block or
    /// as an incremental chunk; the consumer concatenates.
    Message(String),
    /// A tool invocation started.
    ToolStart {
        tool: String,
        summary: Option<String>,
    },
    /// A tool invocation completed.
    ToolEnd { tool: String, ok: bool },
    /// Cumulative token usage update.
    Usage(Usage),
    /// The run is wrapping up with a stop reason.
    Stop { reason: String },
    /// An unrecognised line, kept verbatim for debugging.
    Other(String),
}

#[derive(Debug, Clone, Default, Copy)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
}
