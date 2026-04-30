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
        let raw = if self.bar.is_some() {
            format!(" │ {line}")
        } else {
            line.to_string()
        };
        let styled = if self.color {
            colorize(line, &raw)
        } else {
            raw
        };
        if let Some(bar) = &self.bar {
            bar.println(styled);
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

/// Dark theme. Body text gets a single dark grey BG so the kobito
/// log channel is visually distinct from anything else in the
/// terminal; section headers (Kobito Start / Iteration / Task /
/// Commit / Summary / Error) get a deeper saturated BG with a
/// matching FG so they cut the stream into scannable chunks.
///
/// Routed only through the terminal `print` path; `log.ndjson` and
/// `events.ndjson` get the plain string.
fn colorize(judge: &str, full: &str) -> String {
    // ANSI 256-color palette. fg=None means "keep terminal default".
    // \x1b[K paints the BG out to the end of the line.
    let (bg, fg, bold) = section_for(judge);
    let mut codes = format!("48;5;{bg}");
    if let Some(fg) = fg {
        codes.push_str(&format!(";38;5;{fg}"));
    }
    if bold {
        codes.push_str(";1");
    }
    format!("\x1b[{codes}m{full}\x1b[K\x1b[0m")
}

fn section_for(line: &str) -> (u8, Option<u8>, bool) {
    // Two backgrounds: deep desaturated blue 17 for everything
    // kobito itself emits, neutral slate 234 for the agent stream.
    // Red 52 only for failures. Within the blue channel the
    // foreground tone separates header weight from supporting
    // metadata; bold only on the major boundaries.
    if line.starts_with("kobito start")
        || line.starts_with("kobito resume")
        || line.starts_with("project:")
    {
        // run begins — kobito blue, brightest FG, bold
        (17, Some(153), true)
    } else if line.starts_with("═══")
        || line.starts_with("──")
        || line.starts_with("=== task")
    {
        // iteration / task boundary — kobito blue, mid FG, bold
        (17, Some(117), true)
    } else if line.starts_with("✓ committed") || line.starts_with("✓ PR") {
        // commit / PR landed — kobito blue, brightest FG, bold
        (17, Some(153), true)
    } else if line.starts_with("done ")
        || line.starts_with("  tokens —")
        || line.starts_with("agent reported")
    {
        // summary / tokens / sentinel — kobito blue, mid FG, no bold
        (17, Some(117), false)
    } else if line.starts_with("✗") || line.starts_with("interrupting") {
        // error / cancellation — red
        (52, Some(210), true)
    } else if line.starts_with("▶") {
        // tool call — neutral slate, dimmest FG
        (234, Some(240), false)
    } else {
        // agent body — neutral slate, default FG
        (234, None, false)
    }
}
