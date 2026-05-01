use anyhow::{Context, Result, bail};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use super::Agent;
use super::event::{AgentEvent, Usage};
use crate::logger::LogSink;

pub struct AgentOutcome {
    #[allow(dead_code)]
    pub stdout: String,
    pub natural_stop: bool,
    pub task_complete: bool,
    pub usage: Usage,
}

pub async fn run_streamed(
    mut cmd: Command,
    agent: &dyn Agent,
    sink: &LogSink,
    cancelled: Arc<AtomicBool>,
) -> Result<AgentOutcome> {
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let name = agent.name().to_string();
    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawn {name} — is the `{name}` CLI installed and on PATH?"))?;

    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();

    let stderr_sink = sink.clone();
    let stderr_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            stderr_sink.write("stderr", &line);
        }
    });

    let mut accumulated = String::new();
    let mut last_message: Option<String> = None;
    let mut usage = Usage::default();
    let mut reader = BufReader::new(stdout).lines();
    let mut interrupted = false;
    loop {
        if cancelled.load(Ordering::SeqCst) && !interrupted {
            interrupted = true;
            sink.note(&format!("interrupting {name} (Ctrl-C)"));
            let _ = child.kill().await;
        }
        tokio::select! {
            line = reader.next_line() => {
                match line {
                    Ok(Some(line)) => {
                        sink.record_raw_event(&line);
                        for event in agent.parse_event(&line) {
                            sink.event(&event);
                            match &event {
                                AgentEvent::Message(text) => {
                                    accumulated.push_str(text);
                                    accumulated.push('\n');
                                    last_message = Some(text.clone());
                                }
                                AgentEvent::Usage(u) => {
                                    usage = *u;
                                }
                                _ => {}
                            }
                        }
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(200)) => {
                // periodic poll so we notice cancellation even if the agent goes silent
            }
        }
    }

    let status = child.wait().await?;
    let _ = stderr_task.await;

    if interrupted {
        bail!("{name} cancelled");
    }
    if !status.success() {
        bail!("{name} exited with status {status}");
    }
    let (natural_stop, task_complete) = parse_stop_signal(last_message.as_deref());
    Ok(AgentOutcome {
        stdout: accumulated,
        natural_stop,
        task_complete,
        usage,
    })
}

/// Parse the agent's *final* `Message` text as a JSON object describing
/// loop termination. Anything that is not a valid JSON object, or lacks
/// the recognised boolean fields, is treated as "keep going".
///
/// We deliberately look only at the final Message (and only at *its*
/// content), so source code, diffs, fixtures, or commentary that mention
/// the field names earlier in the response cannot accidentally end the
/// loop. See issue #28.
fn parse_stop_signal(text: Option<&str>) -> (bool, bool) {
    let Some(text) = text else {
        return (false, false);
    };
    let body = strip_code_fence(text.trim());
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else {
        return (false, false);
    };
    (
        v.get("natural_stop")
            .and_then(|x| x.as_bool())
            .unwrap_or(false),
        v.get("task_complete")
            .and_then(|x| x.as_bool())
            .unwrap_or(false),
    )
}

fn strip_code_fence(s: &str) -> String {
    let mut t = s.trim();
    if let Some(rest) = t.strip_prefix("```json").or_else(|| t.strip_prefix("```")) {
        t = rest.trim();
    }
    if let Some(rest) = t.strip_suffix("```") {
        t = rest.trim();
    }
    t.to_string()
}

pub async fn run_oneshot(mut cmd: Command, name: &str) -> Result<String> {
    let out = cmd
        .output()
        .await
        .with_context(|| format!("spawn {name}"))?;
    if !out.status.success() {
        bail!(
            "{name} exited with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8(out.stdout)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process;

    struct TempDir(PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn unique_tmp(prefix: &str) -> TempDir {
        let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let pid = process::id();
        let mut path = std::env::temp_dir();
        path.push(format!("kobito-stream-{prefix}-{pid}-{nanos}"));
        std::fs::create_dir_all(&path).unwrap();
        TempDir(path)
    }

    fn open_sink(prefix: &str) -> (LogSink, TempDir) {
        let dir = unique_tmp(prefix);
        let log = dir.0.join("log.ndjson");
        let sink = LogSink::open(&log, None).unwrap();
        (sink, dir)
    }

    struct FakeAgent;
    impl Agent for FakeAgent {
        fn name(&self) -> &str {
            "fake"
        }
        fn build_streaming_command(&self, _: &str) -> Command {
            Command::new("true")
        }
        fn build_oneshot_command(&self, _: &str) -> Command {
            Command::new("true")
        }
        fn parse_event(&self, line: &str) -> Vec<AgentEvent> {
            vec![AgentEvent::Message(line.to_string())]
        }
    }

    #[tokio::test]
    async fn run_oneshot_returns_stdout_on_success() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("printf 'branch-name'");
        let out = run_oneshot(cmd, "fake").await.unwrap();
        assert_eq!(out, "branch-name");
    }

    #[tokio::test]
    async fn run_oneshot_errors_on_nonzero_exit_with_stderr() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("printf 'boom' >&2; exit 7");
        let err = run_oneshot(cmd, "fake").await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("fake"), "msg should mention agent: {msg}");
        assert!(msg.contains("boom"), "msg should include stderr: {msg}");
    }

    #[tokio::test]
    async fn run_oneshot_errors_when_binary_missing() {
        let cmd = Command::new("kobito-no-such-binary-xyz");
        let err = run_oneshot(cmd, "ghost").await.unwrap_err();
        assert!(format!("{err:#}").contains("ghost"));
    }

    #[tokio::test]
    async fn run_streamed_collects_messages_from_stdout() {
        let (sink, _dir) = open_sink("ok");
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("printf 'hello\\nworld\\n'");
        let cancelled = Arc::new(AtomicBool::new(false));
        let outcome = run_streamed(cmd, &FakeAgent, &sink, cancelled)
            .await
            .unwrap();
        assert!(outcome.stdout.contains("hello"));
        assert!(outcome.stdout.contains("world"));
        assert!(!outcome.natural_stop);
        assert!(!outcome.task_complete);
    }

    #[tokio::test]
    async fn run_streamed_detects_stop_signal_from_final_json() {
        let (sink, _dir) = open_sink("stop");
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg("printf 'doing some work\\n{\"natural_stop\":true,\"task_complete\":true}\\n'");
        let cancelled = Arc::new(AtomicBool::new(false));
        let outcome = run_streamed(cmd, &FakeAgent, &sink, cancelled)
            .await
            .unwrap();
        assert!(outcome.natural_stop);
        assert!(outcome.task_complete);
    }

    #[tokio::test]
    async fn run_streamed_ignores_stop_field_in_intermediate_messages() {
        // Regression for #28: an intermediate Message that quotes the
        // sentinel must not end the loop. Only the *final* message is
        // parsed as JSON.
        let (sink, _dir) = open_sink("intermediate");
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(
            "printf 'discussed {\"natural_stop\":true} in a test fixture\\nfinal commentary\\n'",
        );
        let cancelled = Arc::new(AtomicBool::new(false));
        let outcome = run_streamed(cmd, &FakeAgent, &sink, cancelled)
            .await
            .unwrap();
        assert!(!outcome.natural_stop);
        assert!(!outcome.task_complete);
    }

    #[tokio::test]
    async fn run_streamed_ignores_legacy_uppercase_sentinels() {
        // The previous implementation used substring matching on
        // NATURAL_STOP / TASK_COMPLETE; tests that contain those
        // tokens as fixture data must no longer trigger termination.
        let (sink, _dir) = open_sink("legacy");
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg("printf 'NATURAL_STOP\\nTASK_COMPLETE\\n'");
        let cancelled = Arc::new(AtomicBool::new(false));
        let outcome = run_streamed(cmd, &FakeAgent, &sink, cancelled)
            .await
            .unwrap();
        assert!(!outcome.natural_stop);
        assert!(!outcome.task_complete);
    }

    #[tokio::test]
    async fn run_streamed_errors_on_nonzero_exit() {
        let (sink, _dir) = open_sink("fail");
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("echo bye; exit 3");
        let cancelled = Arc::new(AtomicBool::new(false));
        let err = run_streamed(cmd, &FakeAgent, &sink, cancelled)
            .await
            .err()
            .unwrap();
        let msg = err.to_string();
        assert!(msg.contains("fake"), "msg should mention agent: {msg}");
        assert!(msg.contains("exited"), "msg should mention exit: {msg}");
    }

    #[tokio::test]
    async fn run_streamed_bails_with_cancelled_when_flag_is_set() {
        let (sink, _dir) = open_sink("cancel");
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("sleep 5");
        let cancelled = Arc::new(AtomicBool::new(true));
        let err = run_streamed(cmd, &FakeAgent, &sink, cancelled)
            .await
            .err()
            .unwrap();
        assert!(err.to_string().contains("cancelled"));
    }

    #[tokio::test]
    async fn run_streamed_errors_when_binary_missing() {
        let (sink, _dir) = open_sink("missing");
        let cmd = Command::new("kobito-no-such-binary-xyz");
        let cancelled = Arc::new(AtomicBool::new(false));
        let err = run_streamed(cmd, &FakeAgent, &sink, cancelled)
            .await
            .err()
            .unwrap();
        assert!(format!("{err:#}").contains("fake"));
    }

    #[test]
    fn parse_stop_signal_returns_false_for_none() {
        assert_eq!(parse_stop_signal(None), (false, false));
    }

    #[test]
    fn parse_stop_signal_returns_false_for_non_json_text() {
        assert_eq!(
            parse_stop_signal(Some("just a free-form summary")),
            (false, false)
        );
    }

    #[test]
    fn parse_stop_signal_extracts_natural_stop() {
        assert_eq!(
            parse_stop_signal(Some(r#"{"natural_stop": true}"#)),
            (true, false)
        );
    }

    #[test]
    fn parse_stop_signal_extracts_task_complete() {
        assert_eq!(
            parse_stop_signal(Some(r#"{"task_complete": true}"#)),
            (false, true)
        );
    }

    #[test]
    fn parse_stop_signal_extracts_both_flags() {
        assert_eq!(
            parse_stop_signal(Some(
                r#"{"natural_stop": true, "task_complete": true, "summary": "done"}"#
            )),
            (true, true)
        );
    }

    #[test]
    fn parse_stop_signal_defaults_missing_fields_to_false() {
        assert_eq!(
            parse_stop_signal(Some(r#"{"summary": "wip"}"#)),
            (false, false)
        );
    }

    #[test]
    fn parse_stop_signal_treats_non_bool_field_as_false() {
        assert_eq!(
            parse_stop_signal(Some(r#"{"natural_stop": "yes"}"#)),
            (false, false)
        );
    }

    #[test]
    fn parse_stop_signal_strips_plain_code_fence() {
        let body = "```\n{\"natural_stop\": true}\n```";
        assert_eq!(parse_stop_signal(Some(body)), (true, false));
    }

    #[test]
    fn parse_stop_signal_strips_json_code_fence() {
        let body = "```json\n{\"task_complete\": true}\n```";
        assert_eq!(parse_stop_signal(Some(body)), (false, true));
    }

    #[test]
    fn parse_stop_signal_tolerates_surrounding_whitespace() {
        let body = "\n   {\"natural_stop\": true}   \n";
        assert_eq!(parse_stop_signal(Some(body)), (true, false));
    }
}
