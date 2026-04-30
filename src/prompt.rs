use anyhow::Result;
use std::fs;
use std::path::Path;

pub struct PromptParts {
    pub goal: String,
    pub iteration: u32,
    pub notes: Option<String>,
    pub preset: Option<String>,
}

const META_CONTINUOUS: &str = "\
# About this run

You are being driven by `kobito`, an autonomous orchestrator that spawns you in a loop. \
Each invocation of you is one iteration:

- The repo is checked out on a working branch dedicated to this run.
- After this iteration finishes, the orchestrator stages any diff you produced and commits \
  it with a generated message, then invokes a fresh you with the next iteration's prompt.
- Use `git log` on the current branch to see what previous iterations of *this* run \
  accomplished — your previous self has no in-process memory across iterations.
- Any learnings you wrote in earlier iterations appear below under \"Cross-iteration notes\".
- When the goal is fully reached or no meaningful work remains, output the literal token \
  `NATURAL_STOP` on its own line and stop. The loop will exit cleanly. Otherwise make a \
  small focused change and stop so it can be committed.

";

const META_ITERATION: &str = "\
# About this run

You are being driven by `kobito`, an autonomous orchestrator. This run focuses on **a single \
task** picked from a backlog (`tasks.md`). Other tasks in the backlog are handled on \
separate branches in separate runs — ignore them entirely, even if you notice issues that \
could fix them.

Each invocation of you is one iteration of this task's loop:

- The repo is checked out on a working branch dedicated to this single task.
- After this iteration finishes, the orchestrator stages any diff you produced and commits \
  it with a generated message, then invokes a fresh you with the next iteration's prompt.
- Use `git log` on the current branch to see what previous iterations of *this* task \
  accomplished — your previous self has no in-process memory across iterations.
- Any learnings you wrote in earlier iterations appear below under \"Cross-iteration notes\".
- When the task is fully complete, output the literal token `TASK_COMPLETE` on its own \
  line and stop. The loop will move on to the next task.

";

pub fn build_iteration_prompt(parts: &PromptParts) -> String {
    let mut out = String::from(META_CONTINUOUS);

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
        "Make a small, self-contained improvement toward the goal. Stop when one logical change is complete so the diff can be committed.\n",
    );

    out
}

pub fn build_task_prompt(parts: &PromptParts, task_body: &str) -> String {
    let mut out = String::from(META_ITERATION);

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
        "## Single task\n\n{}\n\n## Iteration {}\n\n",
        task_body.trim(),
        parts.iteration
    ));

    out.push_str(
        "Make focused progress toward completing only this single task. Stop when one logical change is complete so the diff can be committed.\n",
    );

    out
}

pub fn save_prompt(prompts_dir: &Path, iteration: u32, body: &str) -> Result<()> {
    let path = prompts_dir.join(format!("iter-{iteration:04}.md"));
    fs::write(path, body)?;
    Ok(())
}
