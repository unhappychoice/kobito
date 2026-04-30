use anyhow::{Context, Result, bail};
use std::process::Stdio;
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
) -> Result<AgentOutcome> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
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
    let mut usage = Usage::default();
    let mut reader = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = reader.next_line().await {
        let event = agent.parse_event(&line);
        sink.event(&line, &event);
        match &event {
            AgentEvent::Message(text) => {
                accumulated.push_str(text);
                accumulated.push('\n');
            }
            AgentEvent::Usage(u) => {
                usage = *u;
            }
            _ => {}
        }
    }

    let status = child.wait().await?;
    let _ = stderr_task.await;

    if !status.success() {
        bail!("{name} exited with status {status}");
    }
    let natural_stop = accumulated.contains("NATURAL_STOP");
    let task_complete = accumulated.contains("TASK_COMPLETE");
    Ok(AgentOutcome {
        stdout: accumulated,
        natural_stop,
        task_complete,
        usage,
    })
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
