use anyhow::Result;
use regex::Regex;

use crate::agent;
use std::path::Path;

const MAX_DIFF_CHARS: usize = 12_000;

pub async fn generate_message(
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
        "You are generating a single Conventional Commits message.\n\n\
         Output rules:\n\
         - First line: `<type>(<scope>): <subject>` where type is one of feat, fix, refactor, perf, docs, test, chore, build, ci, style.\n\
         - Subject in imperative mood, no trailing period, no longer than 72 chars.\n\
         - Optional body explaining the why, separated by a blank line.\n\
         - No \"Co-authored-by\" or signature lines.\n\
         - Output ONLY the commit message — no markdown fences, no preamble, no explanation.\n\
         {style_block}\n\
         ## Iteration goal\n\n{iteration_goal}\n\n\
         ## Staged diff\n\n```diff\n{trimmed}\n```\n",
    );

    for attempt in 0..2 {
        let raw = agent::invoke_claude_oneshot(repo, &prompt).await?;
        let cleaned = clean(&raw);
        if is_conventional(&cleaned) {
            return Ok(cleaned);
        }
        if attempt == 0 {
            continue;
        }
    }
    Ok(fallback(iteration_goal))
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

fn is_conventional(msg: &str) -> bool {
    let first = match msg.lines().next() {
        Some(l) => l,
        None => return false,
    };
    let re = Regex::new(
        r"^(feat|fix|refactor|perf|docs|test|chore|build|ci|style|revert)(\([^)]+\))?!?: .+",
    )
    .unwrap();
    re.is_match(first) && first.len() <= 100
}

fn fallback(goal: &str) -> String {
    let subject = goal
        .lines()
        .next()
        .unwrap_or("ongoing work")
        .chars()
        .take(60)
        .collect::<String>();
    format!("chore: {}", subject.trim())
}
