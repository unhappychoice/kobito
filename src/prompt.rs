use anyhow::Result;
use std::fs;
use std::path::Path;

pub struct PromptParts {
    pub agents_md: Option<String>,
    pub claude_md: Option<String>,
    pub language: String,
    pub goal: String,
    pub iteration: u32,
    pub notes: Option<String>,
}

pub fn read_repo_docs(repo: &Path) -> (Option<String>, Option<String>) {
    let agents = fs::read_to_string(repo.join("AGENTS.md")).ok();
    let claude = fs::read_to_string(repo.join("CLAUDE.md")).ok();
    (agents, claude)
}

pub fn build_iteration_prompt(parts: &PromptParts, agent: &str) -> String {
    let mut out = String::new();

    out.push_str(&format!(
        "# Project conventions\n\nOutput all code, comments, and commit messages in {}.\n\n",
        parts.language
    ));

    if let Some(md) = &parts.agents_md {
        out.push_str("## AGENTS.md\n\n");
        out.push_str(md.trim());
        out.push_str("\n\n");
    }

    // Claude Code already auto-reads CLAUDE.md, so don't double-inject for `claude`.
    if agent != "claude" {
        if let Some(md) = &parts.claude_md {
            out.push_str("## CLAUDE.md\n\n");
            out.push_str(md.trim());
            out.push_str("\n\n");
        }
    }

    if let Some(notes) = &parts.notes {
        if !notes.trim().is_empty() {
            out.push_str("## Cross-iteration notes\n\n");
            out.push_str(notes.trim());
            out.push_str("\n\n");
        }
    }

    out.push_str(&format!(
        "## Goal\n\n{}\n\n## Iteration {}\n\n",
        parts.goal.trim(),
        parts.iteration
    ));

    out.push_str(
        "Make a small, self-contained improvement toward the goal. Stop when one logical change is complete so the diff can be committed. \
        If the goal is already fully achieved or no meaningful work remains, reply with the literal token NATURAL_STOP and make no changes.\n",
    );

    out
}

pub fn build_task_prompt(parts: &PromptParts, agent: &str, task_body: &str) -> String {
    let mut out = String::new();

    out.push_str(&format!(
        "# Project conventions\n\nOutput all code, comments, and commit messages in {}.\n\n",
        parts.language
    ));

    if let Some(md) = &parts.agents_md {
        out.push_str("## AGENTS.md\n\n");
        out.push_str(md.trim());
        out.push_str("\n\n");
    }

    if agent != "claude" {
        if let Some(md) = &parts.claude_md {
            out.push_str("## CLAUDE.md\n\n");
            out.push_str(md.trim());
            out.push_str("\n\n");
        }
    }

    out.push_str(&format!(
        "## Single task\n\n{}\n\n## Iteration {}\n\n",
        task_body.trim(),
        parts.iteration
    ));

    out.push_str(
        "Make focused progress toward completing only this single task. \
         When the task is fully complete and no more work remains for it, \
         output the literal token TASK_COMPLETE on its own line and stop. \
         Do not start unrelated work even if you notice other issues — they belong to other tasks.\n",
    );

    out
}

pub fn save_prompt(prompts_dir: &Path, iteration: u32, body: &str) -> Result<()> {
    let path = prompts_dir.join(format!("iter-{iteration:04}.md"));
    fs::write(path, body)?;
    Ok(())
}
