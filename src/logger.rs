use anyhow::Result;
use chrono::Utc;
use serde::Serialize;
use std::fs::{File, OpenOptions};
use std::io::Write;
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
        Ok(Self {
            log: Arc::new(Mutex::new(log)),
            events: Arc::new(Mutex::new(events)),
            bar,
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
            self.print(&formatted);
        }
    }

    fn print(&self, line: &str) {
        if let Some(bar) = &self.bar {
            bar.println(format!("│ {line}"));
        } else {
            println!("{line}");
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
        AgentEvent::ToolEnd { tool, ok } => Some(if *ok {
            format!("✓ {tool}")
        } else {
            format!("✗ {tool}")
        }),
        AgentEvent::Stop { reason } => Some(format!("(stop: {reason})")),
        AgentEvent::Usage(_) => None,
        AgentEvent::Other(_) => None,
    }
}
