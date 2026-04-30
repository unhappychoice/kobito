use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::logger::LogSink;

pub struct AgentOutcome {
    #[allow(dead_code)]
    pub stdout: String,
    pub natural_stop: bool,
}

pub async fn invoke_claude(
    repo: &Path,
    prompt: &str,
    sink: &LogSink,
) -> Result<AgentOutcome> {
    let mut child = Command::new("claude")
        .current_dir(repo)
        .args([
            "-p",
            prompt,
            "--permission-mode",
            "acceptEdits",
            "--output-format",
            "text",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawn claude — is the `claude` CLI installed and on PATH?")?;

    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();

    let stdout_sink = sink.clone();
    let stdout_task = tokio::spawn(async move {
        let mut buf = String::new();
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            stdout_sink.write("stdout", &line);
            buf.push_str(&line);
            buf.push('\n');
        }
        buf
    });

    let stderr_sink = sink.clone();
    let stderr_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            stderr_sink.write("stderr", &line);
        }
    });

    let status = child.wait().await?;
    let captured = stdout_task.await.unwrap_or_default();
    let _ = stderr_task.await;

    if !status.success() {
        bail!("claude exited with status {status}");
    }
    let natural_stop = captured.contains("NATURAL_STOP");
    Ok(AgentOutcome { stdout: captured, natural_stop })
}

pub async fn invoke_claude_oneshot(repo: &Path, prompt: &str) -> Result<String> {
    let out = Command::new("claude")
        .current_dir(repo)
        .args(["-p", prompt, "--output-format", "text"])
        .output()
        .await
        .context("spawn claude")?;
    if !out.status.success() {
        bail!(
            "claude exited with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8(out.stdout)?)
}
