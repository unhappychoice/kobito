use anyhow::Result;

use crate::agent::{self, Agent};
use std::path::Path;

const MAX_DIFF_CHARS: usize = 12_000;

pub async fn generate_message(
    agent: &dyn Agent,
    repo: &Path,
    diff: &str,
    iteration_goal: &str,
    style_examples: &[String],
) -> Result<String> {
    let trimmed = truncate(diff, MAX_DIFF_CHARS);
    let style_block = if style_examples.is_empty() {
        String::new()
    } else {
        format!(
            "\n## Existing project commit style (recent subjects)\n\n{}\n",
            style_examples
                .iter()
                .take(10)
                .map(|s| format!("- {s}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };

    let prompt = format!(
        "Generate a single commit message for the staged diff.\n\n\
         Follow this project's commit conventions. The agent's own memory files \
         (AGENTS.md / CLAUDE.md, both global and project scope) are authoritative; \
         additionally match the style of the recent commit subjects below. If both \
         are silent, use a short imperative subject and an optional blank-line-separated body.\n\n\
         Output ONLY the commit message — no markdown fences, no preamble, no explanation. \
         No \"Co-authored-by\" or signature lines.\n\
         {style_block}\n\
         ## Iteration goal\n\n{iteration_goal}\n\n\
         ## Staged diff\n\n```diff\n{trimmed}\n```\n",
    );

    let raw = agent::run_oneshot(agent, repo, &prompt).await?;
    let cleaned = clean(&raw);
    if cleaned.is_empty() {
        return Ok(fallback(iteration_goal));
    }
    Ok(cleaned)
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let head = &s[..max / 2];
    let tail = &s[s.len() - max / 2..];
    format!("{head}\n…\n[diff truncated]\n…\n{tail}")
}

fn clean(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    if s.starts_with("```") {
        let after = s.splitn(2, '\n').nth(1).unwrap_or("").to_string();
        s = after.trim_end_matches("```").trim().to_string();
    }
    s
}

fn fallback(goal: &str) -> String {
    goal.lines()
        .next()
        .unwrap_or("ongoing work")
        .chars()
        .take(72)
        .collect::<String>()
        .trim()
        .to_string()
}
