use anyhow::Result;
use chrono::Utc;
use serde::Serialize;
use std::fs::{File, OpenOptions};
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use indicatif::ProgressBar;

use crate::agent::{AgentEvent, Usage};

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
    state: Arc<Mutex<StatusState>>,
}

#[derive(Default, Clone)]
struct StatusState {
    retries: u32,
    state_label: String,
    usage: Usage,
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
            state: Arc::new(Mutex::new(StatusState::default())),
        })
    }

    /// Update the iteration / retry / state label and refresh the bar.
    /// Token usage is tracked separately and updated whenever a
    /// Usage event arrives.
    pub fn set_iteration_status(&self, iteration: u32, retries: u32, state_label: &str) {
        if let Some(bar) = &self.bar {
            bar.set_prefix(iteration.to_string());
        }
        if let Ok(mut s) = self.state.lock() {
            s.retries = retries;
            s.state_label = state_label.to_string();
        }
        self.refresh_bar();
    }

    fn refresh_bar(&self) {
        let Some(bar) = &self.bar else { return };
        let s = match self.state.lock() {
            Ok(g) => g.clone(),
            Err(_) => return,
        };
        bar.set_message(format!(
            "in {}  ·  out {}  ·  retry {}  ·  {}",
            format_count(s.usage.input_tokens),
            format_count(s.usage.output_tokens),
            s.retries,
            s.state_label,
        ));
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
        self.print(source, line);
    }

    pub fn note(&self, line: &str) {
        self.write("kobito", line);
    }

    /// Persist one raw stdout line to events.ndjson.
    pub fn record_raw_event(&self, raw: &str) {
        if let Ok(json) = serde_json::to_string(&EventLine {
            ts: Utc::now().to_rfc3339(),
            raw,
        }) && let Ok(mut f) = self.events.lock()
        {
            let _ = writeln!(f, "{json}");
        }
    }

    /// Render one parsed event to the terminal + log.ndjson and
    /// update the in-memory status state.
    pub fn event(&self, event: &AgentEvent) {
        if let AgentEvent::Usage(u) = event {
            if let Ok(mut s) = self.state.lock() {
                s.usage = *u;
            }
            self.refresh_bar();
        }
        if let Some(formatted) = format_event(event) {
            self.write("agent", &formatted);
        }
    }

    fn print(&self, source: &str, line: &str) {
        let raw = if self.bar.is_some() {
            format!(" │ {line}")
        } else {
            line.to_string()
        };
        let styled = if self.color {
            colorize(source, line, &raw)
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

fn format_count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn format_event(event: &AgentEvent) -> Option<String> {
    match event {
        AgentEvent::Message(text) => {
            let trimmed = text.trim_end_matches('\n');
            if trimmed.is_empty() {
                return None;
            }
            // The final message of every iteration is a structured stop
            // signal (`{natural_stop|task_complete, summary}`). Showing
            // the raw JSON to the user is just noise — render the
            // summary line on its own.
            if let Some(summary) = extract_iteration_summary(trimmed) {
                return Some(format!("summary: {summary}"));
            }
            Some(trimmed.to_string())
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
        // Stop reasons come from the agent CLI's transport layer
        // (Claude Code's `result.subtype`, etc.) and are too internal
        // to expose to the user — they leak terms like
        // "error_during_execution" on plain Ctrl+C.
        AgentEvent::Stop { .. } => None,
        AgentEvent::Usage(_) => None,
        AgentEvent::Other(_) => None,
    }
}

fn extract_iteration_summary(text: &str) -> Option<String> {
    let body = crate::agent::strip_code_fence(text.trim());
    let v: serde_json::Value = serde_json::from_str(&body).ok()?;
    // Only treat as a stop signal if at least one of the recognised
    // boolean fields is present. Otherwise a stray JSON message
    // earlier in the conversation could swallow itself.
    let is_stop = v.get("natural_stop").and_then(|x| x.as_bool()).is_some()
        || v.get("task_complete").and_then(|x| x.as_bool()).is_some();
    if !is_stop {
        return None;
    }
    let summary = v.get("summary")?.as_str()?.trim();
    if summary.is_empty() {
        None
    } else {
        Some(summary.to_string())
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
fn colorize(source: &str, judge: &str, full: &str) -> String {
    let (bg, fg, bold) = section_for(source, judge);
    let mut codes = bg.to_string();
    if let Some(fg) = fg {
        codes.push(';');
        codes.push_str(fg);
    }
    if bold {
        codes.push_str(";1");
    }
    apply_bg_per_line(&codes, full)
}

/// Apply the same SGR escape to every line of `full`, so the chosen
/// background extends to end-of-line even when the content contains
/// embedded newlines (a single `\x1b[K` only paints up to the first
/// newline).
fn apply_bg_per_line(codes: &str, full: &str) -> String {
    let mut out = String::new();
    let mut first = true;
    for line in full.split('\n') {
        if !first {
            out.push('\n');
        }
        first = false;
        out.push_str("\x1b[");
        out.push_str(codes);
        out.push('m');
        out.push_str(line);
        out.push_str("\x1b[K");
    }
    out.push_str("\x1b[0m");
    out
}

const KOBITO_BG: &str = "48;2;25;30;42"; // cool slate
const KOBITO_FG_BRIGHT: &str = "38;2;180;200;225";
const KOBITO_FG_MID: &str = "38;2;130;150;180";
const BODY_BG: &str = "48;2;18;19;22"; // near-black
const BODY_FG_DIM: &str = "38;2;90;95;105";
const ERROR_BG: &str = "48;2;50;22;26"; // muted maroon
const ERROR_FG: &str = "38;2;200;165;170";

/// Pick a (bg, fg, bold) triple for one rendered line. Routing is
/// **source-first**: anything kobito itself emitted lives on the cool
/// slate background; agent stream lives on near-black. Within each
/// channel a few prefix patterns adjust intensity / pick the maroon
/// accent for failures.
fn section_for(source: &str, line: &str) -> (&'static str, Option<&'static str>, bool) {
    // Failures are always maroon, regardless of source.
    if line.starts_with('✗') || line.starts_with("interrupting") {
        return (ERROR_BG, Some(ERROR_FG), true);
    }
    if source == "kobito" {
        let major = line.starts_with("kobito start")
            || line.starts_with("kobito resume")
            || line.starts_with("project:")
            || line.starts_with("═══")
            || line.starts_with("──")
            || line.starts_with("=== task")
            || line.starts_with("✓ ");
        let fg = if major {
            KOBITO_FG_BRIGHT
        } else {
            KOBITO_FG_MID
        };
        (KOBITO_BG, Some(fg), major)
    } else if line.starts_with('▶') {
        // tool call — body BG, dim FG
        (BODY_BG, Some(BODY_FG_DIM), false)
    } else {
        // agent body — body BG only, terminal default FG
        (BODY_BG, None, false)
    }
}

/// Wrap a string in the kobito-channel SGR codes for use as an inline
/// prompt (e.g. dialoguer-replacement). Unlike `colorize`, this does
/// not emit `\x1b[K`, so the user's typed input is not painted.
pub fn style_kobito_prompt(s: &str) -> String {
    format!("\x1b[{KOBITO_BG};{KOBITO_FG_BRIGHT};1m{s}\x1b[0m")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::process;

    fn unique_tmp(label: &str) -> PathBuf {
        let ts = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let p = std::env::temp_dir().join(format!("kobito-logger-{label}-{}-{ts}", process::id()));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn format_count_renders_compact_units() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1_000), "1.0k");
        assert_eq!(format_count(1_500), "1.5k");
        assert_eq!(format_count(1_000_000), "1.0M");
        assert_eq!(format_count(1_500_000), "1.5M");
    }

    #[test]
    fn format_event_returns_none_for_blank_message() {
        assert!(format_event(&AgentEvent::Message(String::new())).is_none());
        assert!(format_event(&AgentEvent::Message("\n\n".into())).is_none());
    }

    #[test]
    fn format_event_trims_trailing_newlines_on_message() {
        let s = format_event(&AgentEvent::Message("hello\n\n".into())).unwrap();
        assert_eq!(s, "hello");
    }

    #[test]
    fn format_event_renders_tool_start_with_and_without_summary() {
        let with = format_event(&AgentEvent::ToolStart {
            tool: "Read".into(),
            summary: Some("file.rs".into()),
        })
        .unwrap();
        assert_eq!(with, "▶ Read: file.rs");

        let without = format_event(&AgentEvent::ToolStart {
            tool: "Read".into(),
            summary: None,
        })
        .unwrap();
        assert_eq!(without, "▶ Read");
    }

    #[test]
    fn format_event_only_renders_failed_tool_end() {
        let ok = format_event(&AgentEvent::ToolEnd {
            tool: "Read".into(),
            ok: true,
        });
        assert!(ok.is_none());
        let failed = format_event(&AgentEvent::ToolEnd {
            tool: "Read".into(),
            ok: false,
        })
        .unwrap();
        assert_eq!(failed, "✗ Read");
    }

    #[test]
    fn format_event_drops_stop_reason_entirely() {
        assert!(
            format_event(&AgentEvent::Stop {
                reason: "natural".into()
            })
            .is_none()
        );
        assert!(
            format_event(&AgentEvent::Stop {
                reason: "error_during_execution".into()
            })
            .is_none()
        );
    }

    #[test]
    fn format_event_renders_only_summary_when_message_is_stop_signal_json() {
        let s = format_event(&AgentEvent::Message(
            r#"{"natural_stop":false,"summary":"added 5 tests"}"#.into(),
        ))
        .unwrap();
        assert_eq!(s, "summary: added 5 tests");
    }

    #[test]
    fn format_event_renders_summary_for_task_complete_signal() {
        let s = format_event(&AgentEvent::Message(
            r#"{"task_complete":true,"summary":"wired up X"}"#.into(),
        ))
        .unwrap();
        assert_eq!(s, "summary: wired up X");
    }

    #[test]
    fn format_event_passes_through_non_stop_json_messages() {
        let s = format_event(&AgentEvent::Message(
            r#"{"foo":1,"summary":"unrelated"}"#.into(),
        ))
        .unwrap();
        assert!(s.contains("\"foo\":1"));
    }

    #[test]
    fn format_event_drops_usage_and_other() {
        assert!(format_event(&AgentEvent::Usage(Usage::default())).is_none());
        assert!(format_event(&AgentEvent::Other("debug".into())).is_none());
    }

    #[test]
    fn section_for_kobito_source_uses_kobito_bg() {
        for line in [
            "kobito start: thing",
            "  tokens — 100",
            "summary: did stuff",
            "agent reported natural_stop",
            "asking agent for something",
        ] {
            let (bg, fg, _) = section_for("kobito", line);
            assert!(bg.contains("25;30;42"), "expected kobito bg for {line:?}");
            assert!(fg.is_some());
        }
    }

    #[test]
    fn section_for_kobito_major_lines_are_bold() {
        for line in [
            "kobito start",
            "kobito resume foo",
            "project: kobito",
            "═══════",
            "── line",
            "=== task 1 ===",
            "✓ committed abc",
            "✓ PR opened",
            "✓ draft PR: https://...",
        ] {
            let (_, fg, bold) = section_for("kobito", line);
            assert!(bold, "expected bold for {line:?}");
            assert!(fg.is_some());
        }
    }

    #[test]
    fn section_for_routes_errors_to_maroon_bg_regardless_of_source() {
        for (source, line) in [
            ("kobito", "✗ push failed"),
            ("agent", "✗ Read"),
            ("kobito", "interrupting agent"),
        ] {
            let (bg, _, bold) = section_for(source, line);
            assert!(bold);
            assert!(
                bg.contains("50;22;26"),
                "expected maroon bg for ({source:?}, {line:?})"
            );
        }
    }

    #[test]
    fn section_for_classifies_agent_tool_calls_and_body() {
        let (_, fg, bold) = section_for("agent", "▶ Read");
        assert!(!bold);
        assert!(fg.is_some());

        let (bg, fg, bold) = section_for("agent", "plain agent body");
        assert!(!bold);
        assert!(fg.is_none());
        assert!(bg.contains("18;19;22"));
    }

    #[test]
    fn colorize_wraps_with_reset_sequence() {
        let s = colorize("kobito", "hello", "padding hello");
        assert!(s.starts_with("\x1b["));
        assert!(s.ends_with("\x1b[K\x1b[0m"));
        assert!(s.contains("padding hello"));
    }

    #[test]
    fn colorize_paints_each_line_for_multi_line_input() {
        let s = colorize("kobito", "first", "first\nsecond\nthird");
        // Three lines → three opening SGR escapes (one per line).
        let opens = s.matches("\x1b[48;2;25;30;42").count();
        assert_eq!(opens, 3);
        // Three \x1b[K end-of-line clears, one final reset.
        assert_eq!(s.matches("\x1b[K").count(), 3);
        assert!(s.ends_with("\x1b[0m"));
    }

    #[test]
    fn style_kobito_prompt_does_not_clear_to_eol() {
        let s = style_kobito_prompt("Continue?");
        assert!(s.contains("\x1b[48;2;25;30;42"));
        assert!(!s.contains("\x1b[K"));
        assert!(s.ends_with("\x1b[0m"));
    }

    #[test]
    fn open_creates_log_and_events_files() {
        let dir = unique_tmp("open");
        let log_path = dir.join("log.ndjson");
        let _sink = LogSink::open(&log_path, None).expect("open should succeed");
        assert!(log_path.exists());
        assert!(dir.join("events.ndjson").exists());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn write_appends_a_json_line_to_log() {
        let dir = unique_tmp("write");
        let log_path = dir.join("log.ndjson");
        let sink = LogSink::open(&log_path, None).unwrap();
        sink.write("agent", "hello");
        let body = fs::read_to_string(&log_path).unwrap();
        assert!(body.contains("\"source\":\"agent\""));
        assert!(body.contains("\"line\":\"hello\""));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn note_records_under_kobito_source() {
        let dir = unique_tmp("note");
        let log_path = dir.join("log.ndjson");
        let sink = LogSink::open(&log_path, None).unwrap();
        sink.note("starting up");
        let body = fs::read_to_string(&log_path).unwrap();
        assert!(body.contains("\"source\":\"kobito\""));
        assert!(body.contains("starting up"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn record_raw_event_appends_to_events_file() {
        let dir = unique_tmp("raw");
        let log_path = dir.join("log.ndjson");
        let sink = LogSink::open(&log_path, None).unwrap();
        sink.record_raw_event("{\"k\":1}");
        let body = fs::read_to_string(dir.join("events.ndjson")).unwrap();
        assert!(body.contains("\"raw\""));
        assert!(body.contains("{\\\"k\\\":1}"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn event_writes_message_and_skips_silent_variants() {
        let dir = unique_tmp("event");
        let log_path = dir.join("log.ndjson");
        let sink = LogSink::open(&log_path, None).unwrap();
        sink.event(&AgentEvent::Message("payload".into()));
        sink.event(&AgentEvent::Usage(Usage {
            input_tokens: 5,
            output_tokens: 7,
            cached_input_tokens: 0,
        }));
        sink.event(&AgentEvent::Other("debug".into()));
        let body = fs::read_to_string(&log_path).unwrap();
        assert!(body.contains("payload"));
        let lines = body.lines().count();
        assert_eq!(lines, 1, "only the Message event should be persisted");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn event_usage_updates_internal_state() {
        let dir = unique_tmp("usage");
        let log_path = dir.join("log.ndjson");
        let sink = LogSink::open(&log_path, None).unwrap();
        sink.event(&AgentEvent::Usage(Usage {
            input_tokens: 11,
            output_tokens: 22,
            cached_input_tokens: 0,
        }));
        let s = sink.state.lock().unwrap();
        assert_eq!(s.usage.input_tokens, 11);
        assert_eq!(s.usage.output_tokens, 22);
        drop(s);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn set_iteration_status_updates_state_without_a_bar() {
        let dir = unique_tmp("status");
        let log_path = dir.join("log.ndjson");
        let sink = LogSink::open(&log_path, None).unwrap();
        sink.set_iteration_status(2, 3, "running");
        let s = sink.state.lock().unwrap();
        assert_eq!(s.retries, 3);
        assert_eq!(s.state_label, "running");
        drop(s);
        fs::remove_dir_all(&dir).ok();
    }
}
