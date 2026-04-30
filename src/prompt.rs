use anyhow::Result;
use std::fs;
use std::path::Path;

pub struct PromptParts {
    pub goal: String,
    pub iteration: u32,
    pub notes: Option<String>,
    pub preset: Option<String>,
}

pub fn build_iteration_prompt(parts: &PromptParts) -> String {
    let mut out = String::new();

    if let Some(preset) = &parts.preset {
        out.push_str(preset.trim());
        out.push_str("\n\n");
    }

    if let Some(notes) = &parts.notes
        && !notes.trim().is_empty()
    {
        out.push_str("## Cross-iteration notes\n\n");
        out.push_str(notes.trim());
        out.push_str("\n\n");
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

pub fn build_task_prompt(parts: &PromptParts, task_body: &str) -> String {
    let mut out = String::new();

    if let Some(preset) = &parts.preset {
        out.push_str(preset.trim());
        out.push_str("\n\n");
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
