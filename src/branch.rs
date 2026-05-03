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
    use tokio::process::Command;

    struct FakeAgent {
        script: &'static str,
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
            cmd.arg("-c").arg(self.script);
            cmd
        }

        fn parse_event(&self, _: &str) -> Vec<crate::agent::AgentEvent> {
            vec![]
        }
    }

    #[tokio::test]
    async fn suggest_returns_cleaned_first_line_from_agent() {
        let agent = FakeAgent {
            script: "printf '\"feature/coverage/\"\\nexplanation\\n'",
        };
        let branch = suggest(&agent, Path::new("."), "increase coverage")
            .await
            .unwrap();
        assert_eq!(branch, "feature/coverage");
    }

    #[tokio::test]
    async fn suggest_errors_when_agent_returns_empty_branch_name() {
        let agent = FakeAgent {
            script: "printf '   \\nexplanation\\n'",
        };
        let err = suggest(&agent, Path::new("."), "increase coverage")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("empty branch name"));
    }

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
}
