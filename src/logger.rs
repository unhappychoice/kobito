use anyhow::Result;
use chrono::Utc;
use serde::Serialize;
use std::fs::{File, OpenOptions};
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use indicatif::ProgressBar;

use crate::agent::AgentEvent;

#[derive(Serialize)]
struct LogLine<'a> {
    ts: String,
    source: &'a str,
    line: &'a str,
}

#[derive(Serialize)]
struct EventLine<'a> {
    ts: String,
    raw: &'a str,
}

#[derive(Clone)]
pub struct LogSink {
    log: Arc<Mutex<File>>,
    events: Arc<Mutex<File>>,
    bar: Option<ProgressBar>,
    color: bool,
}

impl LogSink {
    pub fn open(log_path: &Path, bar: Option<ProgressBar>) -> Result<Self> {
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)?;
        let events_path = log_path.with_file_name("events.ndjson");
        let events = OpenOptions::new()
            .create(true)
            .append(true)
            .open(events_path)?;
        let color = std::io::stdout().is_terminal();
        Ok(Self {
            log: Arc::new(Mutex::new(log)),
            events: Arc::new(Mutex::new(events)),
            bar,
            color,
        })
    }

    pub fn write(&self, source: &str, line: &str) {
        let entry = LogLine {
            ts: Utc::now().to_rfc3339(),
            source,
            line,
        };
        if let Ok(json) = serde_json::to_string(&entry)
            && let Ok(mut f) = self.log.lock()
        {
            let _ = writeln!(f, "{json}");
        }
        self.print(line);
    }

    pub fn note(&self, line: &str) {
        self.write("kobito", line);
    }

    pub fn event(&self, raw: &str, event: &AgentEvent) {
        if let Ok(json) = serde_json::to_string(&EventLine {
            ts: Utc::now().to_rfc3339(),
            raw,
        }) && let Ok(mut f) = self.events.lock()
        {
            let _ = writeln!(f, "{json}");
        }
        if let Some(formatted) = format_event(event) {
            self.write("agent", &formatted);
        }
    }

    fn print(&self, line: &str) {
        let styled = if self.color {
            colorize(line)
        } else {
            line.to_string()
        };
        if let Some(bar) = &self.bar {
            bar.println(format!("│ {styled}"));
        } else {
            println!("{styled}");
        }
    }
}

fn format_event(event: &AgentEvent) -> Option<String> {
    match event {
        AgentEvent::Message(text) => {
            let trimmed = text.trim_end_matches('\n');
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        AgentEvent::ToolStart { tool, summary } => Some(match summary {
            Some(s) => format!("▶ {tool}: {s}"),
            None => format!("▶ {tool}"),
        }),
        AgentEvent::ToolEnd { tool, ok } => {
            if *ok {
                None
            } else {
                Some(format!("✗ {tool}"))
            }
        }
        AgentEvent::Stop { reason } => Some(format!("(stop: {reason})")),
        AgentEvent::Usage(_) => None,
        AgentEvent::Other(_) => None,
    }
}

/// ANSI color the line based on its leading marker. Plain text falls
/// through unchanged. The log.ndjson copy is unaffected — only the
/// terminal-bound `print` path passes through this.
fn colorize(line: &str) -> String {
    if line.starts_with("═══") {
        // major header: bold reverse cyan
        format!("\x1b[1;7;36m {line} \x1b[0m")
    } else if line.starts_with("──") {
        // minor header (per-task iteration): bold cyan
        format!("\x1b[1;36m{line}\x1b[0m")
    } else if line.starts_with("=== task") {
        // iteration-mode task header: bold reverse magenta
        format!("\x1b[1;7;35m {line} \x1b[0m")
    } else if line.starts_with("✓ committed") || line.starts_with("✓ PR") {
        format!("\x1b[1;32m{line}\x1b[0m") // bold green
    } else if line.starts_with("✗") {
        format!("\x1b[31m{line}\x1b[0m") // red
    } else if line.starts_with("▶") {
        format!("\x1b[34m{line}\x1b[0m") // blue
    } else if line.starts_with("✓") {
        format!("\x1b[2;32m{line}\x1b[0m") // dim green
    } else if line.starts_with("(stop:") {
        format!("\x1b[2;3m{line}\x1b[0m") // dim italic
    } else if line.starts_with("kobito ")
        || line.starts_with("project:")
        || line.starts_with("done")
        || line.starts_with("  tokens —")
        || line.starts_with("agent reported")
        || line.starts_with("interrupting")
        || line.starts_with("interrupted")
    {
        format!("\x1b[2m{line}\x1b[0m") // dim
    } else {
        line.to_string()
    }
}
