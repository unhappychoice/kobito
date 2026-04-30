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
    let total = s.chars().count();
    if total <= max {
        return s.to_string();
    }
    let keep = max / 2;
    let head: String = s.chars().take(keep).collect();
    let tail: String = s.chars().skip(total - keep).collect();
    format!("{head}\n…\n[diff truncated]\n…\n{tail}")
}

fn clean(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    if s.starts_with("```") {
        let after = s.split_once('\n').map(|(_, rest)| rest).unwrap_or("");
        s = after.trim_end_matches("```").trim().to_string();
    }
    s
}

fn fallback(goal: &str) -> String {
    goal.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("ongoing work")
        .chars()
        .take(72)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_returns_input_when_within_limit() {
        let s = "short diff";
        assert_eq!(truncate(s, 100), s);
    }

    #[test]
    fn truncate_returns_input_at_exact_limit() {
        let s = "a".repeat(50);
        assert_eq!(truncate(&s, 50), s);
    }

    #[test]
    fn truncate_keeps_head_and_tail_when_over_limit() {
        let head = "H".repeat(100);
        let tail = "T".repeat(100);
        let input = format!("{head}MIDDLE{tail}");
        let out = truncate(&input, 20);
        assert!(out.starts_with(&"H".repeat(10)));
        assert!(out.ends_with(&"T".repeat(10)));
        assert!(out.contains("[diff truncated]"));
        assert!(!out.contains("MIDDLE"));
    }

    #[test]
    fn truncate_handles_multibyte_chars_without_panic() {
        let head = "あ".repeat(100);
        let tail = "い".repeat(100);
        let input = format!("{head}MIDDLE{tail}");
        let out = truncate(&input, 20);
        assert!(out.starts_with(&"あ".repeat(10)));
        assert!(out.ends_with(&"い".repeat(10)));
        assert!(out.contains("[diff truncated]"));
    }

    #[test]
    fn clean_trims_surrounding_whitespace() {
        assert_eq!(clean("  hello world  \n"), "hello world");
    }

    #[test]
    fn clean_strips_leading_fence_and_trailing_fence() {
        let raw = "```\nfeat: add thing\n\nbody line\n```";
        assert_eq!(clean(raw), "feat: add thing\n\nbody line");
    }

    #[test]
    fn clean_strips_language_tagged_fence() {
        let raw = "```text\nfix: bug\n```";
        assert_eq!(clean(raw), "fix: bug");
    }

    #[test]
    fn clean_returns_empty_when_only_fence_with_no_newline() {
        assert_eq!(clean("```"), "");
    }

    #[test]
    fn clean_returns_empty_for_blank_input() {
        assert_eq!(clean(""), "");
        assert_eq!(clean("   \n  "), "");
    }

    #[test]
    fn fallback_uses_first_line_of_goal() {
        let goal = "implement feature\nmore details\n";
        assert_eq!(fallback(goal), "implement feature");
    }

    #[test]
    fn fallback_truncates_to_72_chars() {
        let goal = "a".repeat(100);
        let out = fallback(&goal);
        assert_eq!(out.chars().count(), 72);
    }

    #[test]
    fn fallback_trims_whitespace() {
        assert_eq!(fallback("   hello   "), "hello");
    }

    #[test]
    fn fallback_uses_default_for_empty_goal() {
        assert_eq!(fallback(""), "ongoing work");
    }

    #[test]
    fn fallback_skips_blank_first_line() {
        assert_eq!(fallback("   \nreal goal"), "real goal");
    }

    #[test]
    fn fallback_uses_default_when_all_lines_blank() {
        assert_eq!(fallback("   \n\t\n  "), "ongoing work");
    }
}
