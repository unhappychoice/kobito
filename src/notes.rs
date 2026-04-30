use anyhow::Result;
use chrono::Utc;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use crate::agent::{self, Agent};

const NO_NOTES: &str = "NO_NOTES";

pub async fn append_learning(
    agent: &dyn Agent,
    repo: &Path,
    notes_path: &Path,
    iteration: u32,
    iteration_goal: &str,
    diff: &str,
) -> Result<bool> {
    let prompt = format!(
        "You just finished one iteration of an autonomous task. Distill what is \
         useful to remember for the next iteration: surprising findings, \
         constraints discovered, dead-ends to avoid, file paths or commands worth \
         keeping. Skip the obvious — only what would change a future attempt.\n\n\
         Output 1 to 5 short bullets, plain text, no preamble, no explanation, \
         no markdown fences. If nothing non-obvious was learned, output the \
         literal token {NO_NOTES} and nothing else.\n\n\
         ## Iteration goal\n\n{iteration_goal}\n\n\
         ## What happened (diff)\n\n```diff\n{diff_excerpt}\n```\n",
        diff_excerpt = excerpt(diff, 4_000),
    );

    let raw = agent::run_oneshot(agent, repo, &prompt).await?;
    let cleaned = clean(&raw);
    if cleaned.is_empty() || cleaned == NO_NOTES {
        return Ok(false);
    }

    append_section(notes_path, iteration, &cleaned)?;
    Ok(true)
}

fn excerpt(s: &str, max: usize) -> String {
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
        let after = s.split_once('\n').map(|(_, rest)| rest).unwrap_or("");
        s = after.trim_end_matches("```").trim().to_string();
    }
    s
}

fn append_section(path: &Path, iteration: u32, body: &str) -> Result<()> {
    let header = format!(
        "\n## iteration {iteration} — {ts}\n\n",
        ts = Utc::now().to_rfc3339()
    );
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(header.as_bytes())?;
    file.write_all(body.as_bytes())?;
    file.write_all(b"\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn unique_path(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "kobito-notes-{label}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
        ));
        fs::create_dir_all(&dir).unwrap();
        dir.join("notes.md")
    }

    #[test]
    fn clean_strips_fences() {
        assert_eq!(clean("```\n- one\n- two\n```"), "- one\n- two");
        assert_eq!(clean("```text\n- a\n```"), "- a");
    }

    #[test]
    fn clean_preserves_plain() {
        assert_eq!(clean("- one\n- two\n"), "- one\n- two");
    }

    #[test]
    fn clean_returns_empty_for_blank() {
        assert_eq!(clean(""), "");
        assert_eq!(clean("   \n  "), "");
    }

    #[test]
    fn clean_handles_fence_without_newline() {
        assert_eq!(clean("```"), "");
    }

    #[test]
    fn excerpt_passes_through_when_short() {
        assert_eq!(excerpt("short", 100), "short");
    }

    #[test]
    fn excerpt_passes_through_at_exact_limit() {
        let s = "a".repeat(50);
        assert_eq!(excerpt(&s, 50), s);
    }

    #[test]
    fn excerpt_truncates_when_long() {
        let s: String = "x".repeat(10_000);
        let out = excerpt(&s, 4_000);
        assert!(out.contains("[diff truncated]"));
        assert!(out.len() < 5_000);
    }

    #[test]
    fn excerpt_keeps_head_and_tail_markers() {
        let head = "H".repeat(100);
        let tail = "T".repeat(100);
        let input = format!("{head}MIDDLE{tail}");
        let out = excerpt(&input, 20);
        assert!(out.starts_with(&"H".repeat(10)));
        assert!(out.ends_with(&"T".repeat(10)));
        assert!(!out.contains("MIDDLE"));
    }

    #[test]
    fn append_section_creates_file_when_missing() {
        let path = unique_path("create");
        assert!(!path.exists());
        append_section(&path, 1, "- learning one").unwrap();
        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("## iteration 1 —"));
        assert!(contents.contains("- learning one"));
        assert!(contents.ends_with('\n'));
    }

    #[test]
    fn append_section_appends_to_existing_file() {
        let path = unique_path("append");
        fs::write(&path, "preexisting\n").unwrap();
        append_section(&path, 2, "- new note").unwrap();
        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.starts_with("preexisting\n"));
        assert!(contents.contains("## iteration 2 —"));
        assert!(contents.contains("- new note"));
    }

    #[test]
    fn append_section_supports_multiple_iterations() {
        let path = unique_path("multi");
        append_section(&path, 1, "- first").unwrap();
        append_section(&path, 2, "- second").unwrap();
        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("## iteration 1 —"));
        assert!(contents.contains("## iteration 2 —"));
        let pos1 = contents.find("- first").unwrap();
        let pos2 = contents.find("- second").unwrap();
        assert!(pos1 < pos2);
    }

    #[test]
    fn append_section_returns_error_when_path_is_directory() {
        let dir = std::env::temp_dir().join(format!(
            "kobito-notes-dirpath-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
        ));
        fs::create_dir_all(&dir).unwrap();
        assert!(append_section(&dir, 1, "x").is_err());
    }
}
