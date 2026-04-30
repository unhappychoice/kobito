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

/// Muted dark theme using 24-bit color.
///
/// Two near-grey backgrounds split the stream:
///
/// - **kobito channel** (#191e2a, slightly cool slate) — everything
///   kobito itself emits: start banner, iteration / task boundaries,
///   commit landed, tokens / summary, sentinel notices.
/// - **agent stream** (#121316, near-black) — the agent's own
///   message text and tool calls.
///
/// Within each channel only the foreground tone changes — no extra
/// hues. A muted maroon (#32161a) is the lone exception, reserved
/// for failures and cancellation.
///
/// Routed only through the terminal `print` path; `log.ndjson` and
/// `events.ndjson` get the plain string.
fn colorize(judge: &str, full: &str) -> String {
    let (bg, fg, bold) = section_for(judge);
    let mut codes = bg.to_string();
    if let Some(fg) = fg {
        codes.push(';');
        codes.push_str(fg);
    }
    if bold {
        codes.push_str(";1");
    }
    format!("\x1b[{codes}m{full}\x1b[K\x1b[0m")
}

fn section_for(line: &str) -> (&'static str, Option<&'static str>, bool) {
    // 24-bit truecolor: 48;2;<r>;<g>;<b> for BG, 38;2;... for FG.
    // Two BGs + one accent BG. Three FG tones.
    const KOBITO_BG: &str = "48;2;25;30;42"; // cool slate
    const KOBITO_FG_BRIGHT: &str = "38;2;180;200;225";
    const KOBITO_FG_MID: &str = "38;2;130;150;180";
    const BODY_BG: &str = "48;2;18;19;22"; // near-black
    const BODY_FG_DIM: &str = "38;2;90;95;105";
    const ERROR_BG: &str = "48;2;50;22;26"; // muted maroon
    const ERROR_FG: &str = "38;2;200;165;170";

    if line.starts_with("kobito start")
        || line.starts_with("kobito resume")
        || line.starts_with("project:")
        || line.starts_with("═══")
        || line.starts_with("──")
        || line.starts_with("=== task")
        || line.starts_with("✓ committed")
        || line.starts_with("✓ PR")
    {
        // major header — kobito BG, bright FG, bold
        (KOBITO_BG, Some(KOBITO_FG_BRIGHT), true)
    } else if line.starts_with("done ")
        || line.starts_with("  tokens —")
        || line.starts_with("agent reported")
    {
        // summary / tokens / sentinel — kobito BG, mid FG
        (KOBITO_BG, Some(KOBITO_FG_MID), false)
    } else if line.starts_with("✗") || line.starts_with("interrupting") {
        // error / cancellation — muted maroon
        (ERROR_BG, Some(ERROR_FG), true)
    } else if line.starts_with("▶") {
        // tool call — body BG, dim FG
        (BODY_BG, Some(BODY_FG_DIM), false)
    } else {
        // agent body — body BG only, terminal default FG
        (BODY_BG, None, false)
    }
}
