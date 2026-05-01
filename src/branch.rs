use anyhow::{Result, bail};
use std::path::Path;

use crate::agent::{self, Agent};

pub async fn suggest(agent: &dyn Agent, repo: &Path, task: &str) -> Result<String> {
    let prompt = format!(
        "Suggest a single git branch name for the task below.\n\n\
         Follow this project's branch naming conventions. The agent's own memory \
         files (AGENTS.md / CLAUDE.md, both global and project scope) are \
         authoritative. If they are silent, use a short kebab-case slug.\n\n\
         Output ONLY the branch name on one line, no quotes, no markdown, no \
         explanation. Do not include a timestamp — kobito appends one.\n\n\
         ## Task\n\n{task}\n",
    );
    let raw = agent::run_oneshot(agent, repo, &prompt).await?;
    let cleaned = clean(&raw);
    if cleaned.is_empty() {
        bail!("agent produced an empty branch name");
    }
    Ok(cleaned)
}

fn clean(raw: &str) -> String {
    raw.lines()
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches(|c: char| c == '"' || c == '\'' || c == '`')
        .trim()
        .trim_end_matches('-')
        .trim_end_matches('/')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentEvent;
    use tokio::process::Command;

    #[test]
    fn clean_strips_quotes_and_trailing_dash() {
        assert_eq!(clean("\"feature/coverage-\""), "feature/coverage");
        assert_eq!(clean("`feat/x`"), "feat/x");
    }

    #[test]
    fn clean_takes_first_line_only() {
        assert_eq!(clean("feature/foo\nexplanation"), "feature/foo");
    }

    #[test]
    fn clean_returns_empty_for_blank() {
        assert_eq!(clean(""), "");
        assert_eq!(clean("   "), "");
    }

    #[test]
    fn clean_strips_trailing_slash() {
        assert_eq!(clean("feature/x/"), "feature/x");
    }

    struct FakeAgent {
        output: String,
        fail: bool,
    }

    impl Agent for FakeAgent {
        fn name(&self) -> &str {
            "fake"
        }
        fn build_streaming_command(&self, _: &str) -> Command {
            Command::new("true")
        }
        fn build_oneshot_command(&self, _: &str) -> Command {
            let mut cmd = Command::new("sh");
            if self.fail {
                cmd.arg("-c").arg("exit 1");
            } else {
                cmd.arg("-c")
                    .arg(format!("printf %s {}", shell_escape(&self.output)));
            }
            cmd
        }
        fn parse_event(&self, _: &str) -> Vec<AgentEvent> {
            vec![]
        }
    }

    fn shell_escape(s: &str) -> String {
        let escaped = s.replace('\'', "'\\''");
        format!("'{escaped}'")
    }

    #[tokio::test]
    async fn suggest_returns_cleaned_branch_name_from_agent() {
        let agent = FakeAgent {
            output: "\"feature/x-\"\n".to_string(),
            fail: false,
        };
        let repo = std::env::temp_dir();
        let name = suggest(&agent, &repo, "implement x").await.unwrap();
        assert_eq!(name, "feature/x");
    }

    #[tokio::test]
    async fn suggest_uses_first_line_of_agent_output() {
        let agent = FakeAgent {
            output: "feature/foo\nexplanation".to_string(),
            fail: false,
        };
        let repo = std::env::temp_dir();
        let name = suggest(&agent, &repo, "task").await.unwrap();
        assert_eq!(name, "feature/foo");
    }

    #[tokio::test]
    async fn suggest_errors_when_agent_returns_blank() {
        let agent = FakeAgent {
            output: "   \n".to_string(),
            fail: false,
        };
        let repo = std::env::temp_dir();
        let err = suggest(&agent, &repo, "task").await.err().unwrap();
        assert!(
            err.to_string().contains("empty"),
            "expected empty-name error, got: {err}",
        );
    }

    #[tokio::test]
    async fn suggest_propagates_agent_failure() {
        let agent = FakeAgent {
            output: String::new(),
            fail: true,
        };
        let repo = std::env::temp_dir();
        let err = suggest(&agent, &repo, "task").await.err().unwrap();
        assert!(
            err.to_string().contains("fake"),
            "expected agent-name in error, got: {err}",
        );
    }
}
