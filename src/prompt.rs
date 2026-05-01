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

## How to stop

Your **final message** in this iteration MUST be a single JSON object — nothing else, no prose, no code fence, no commentary:

```
{\"natural_stop\": <bool>, \"summary\": \"<one-line summary of what you did this iteration>\"}
```

- Set `natural_stop` to `true` when the goal is fully reached or no meaningful work remains. The orchestrator will exit the loop cleanly.
- Set `natural_stop` to `false` after making a small focused change so the diff can be committed and the next iteration can run.
- The JSON is the *only* signal the orchestrator looks at. Discussing or quoting `natural_stop` in earlier messages is fine — only this final JSON is parsed.

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

## How to stop

Your **final message** in this iteration MUST be a single JSON object — nothing else, no prose, no code fence, no commentary:

```
{\"task_complete\": <bool>, \"summary\": \"<one-line summary of what you did this iteration>\"}
```

- Set `task_complete` to `true` when the task is fully done. The orchestrator will move on to the next task.
- Set `task_complete` to `false` after making a small focused change so the diff can be committed and the next iteration can run.
- The JSON is the *only* signal the orchestrator looks at. Discussing or quoting `task_complete` in earlier messages is fine — only this final JSON is parsed.

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

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(goal: &str, iteration: u32) -> PromptParts {
        PromptParts {
            goal: goal.to_string(),
            iteration,
            notes: None,
            preset: None,
        }
    }

    #[test]
    fn iteration_prompt_includes_meta_goal_and_iteration() {
        let out = build_iteration_prompt(&parts("ship the thing", 7));
        assert!(out.starts_with("# About this run"));
        assert!(out.contains("\"natural_stop\""));
        assert!(out.contains("## Goal\n\nship the thing"));
        assert!(out.contains("## Iteration 7"));
    }

    #[test]
    fn iteration_prompt_trims_goal() {
        let out = build_iteration_prompt(&parts("  padded goal  \n", 1));
        assert!(out.contains("## Goal\n\npadded goal\n"));
    }

    #[test]
    fn iteration_prompt_omits_notes_section_when_absent() {
        let out = build_iteration_prompt(&parts("g", 1));
        assert!(!out.contains("## Cross-iteration notes"));
    }

    #[test]
    fn iteration_prompt_omits_notes_section_when_blank() {
        let mut p = parts("g", 1);
        p.notes = Some("   \n  ".into());
        let out = build_iteration_prompt(&p);
        assert!(!out.contains("## Cross-iteration notes"));
    }

    #[test]
    fn iteration_prompt_includes_notes_when_present() {
        let mut p = parts("g", 1);
        p.notes = Some("- learned X\n- avoid Y".into());
        let out = build_iteration_prompt(&p);
        assert!(out.contains("## Cross-iteration notes\n\n- learned X\n- avoid Y\n\n"));
    }

    #[test]
    fn iteration_prompt_includes_preset_before_notes() {
        let mut p = parts("g", 1);
        p.preset = Some("PRESET BODY".into());
        p.notes = Some("NOTES".into());
        let out = build_iteration_prompt(&p);
        let preset_idx = out.find("PRESET BODY").unwrap();
        let notes_idx = out.find("## Cross-iteration notes").unwrap();
        let goal_idx = out.find("## Goal").unwrap();
        assert!(preset_idx < notes_idx);
        assert!(notes_idx < goal_idx);
    }

    #[test]
    fn iteration_prompt_ends_with_focus_directive() {
        let out = build_iteration_prompt(&parts("g", 1));
        assert!(out.trim_end().ends_with(
            "Make a small, self-contained improvement toward the goal. Stop when one logical change is complete so the diff can be committed."
        ));
    }

    #[test]
    fn task_prompt_includes_meta_body_and_iteration() {
        let out = build_task_prompt(&parts("ignored goal", 3), "wire up X");
        assert!(out.starts_with("# About this run"));
        assert!(out.contains("\"task_complete\""));
        assert!(out.contains("## Single task\n\nwire up X"));
        assert!(out.contains("## Iteration 3"));
    }

    #[test]
    fn task_prompt_does_not_use_continuous_meta() {
        let out = build_task_prompt(&parts("g", 1), "task");
        assert!(!out.contains("\"natural_stop\""));
    }

    #[test]
    fn task_prompt_omits_notes_when_blank() {
        let mut p = parts("g", 1);
        p.notes = Some("\n".into());
        let out = build_task_prompt(&p, "task");
        assert!(!out.contains("## Cross-iteration notes"));
    }

    #[test]
    fn task_prompt_includes_preset_and_notes() {
        let mut p = parts("g", 2);
        p.preset = Some("PRESET".into());
        p.notes = Some("a note".into());
        let out = build_task_prompt(&p, "do thing");
        assert!(out.contains("PRESET"));
        assert!(out.contains("## Cross-iteration notes\n\na note"));
        assert!(out.contains("## Single task\n\ndo thing"));
    }

    #[test]
    fn task_prompt_ends_with_focus_directive() {
        let out = build_task_prompt(&parts("g", 1), "task");
        assert!(out.trim_end().ends_with(
            "Make focused progress toward completing only this single task. Stop when one logical change is complete so the diff can be committed."
        ));
    }

    #[test]
    fn save_prompt_writes_padded_filename() {
        let dir = std::env::temp_dir().join(format!(
            "kobito-prompt-test-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        fs::create_dir_all(&dir).unwrap();
        save_prompt(&dir, 5, "hello body").unwrap();
        let path = dir.join("iter-0005.md");
        let body = fs::read_to_string(&path).unwrap();
        assert_eq!(body, "hello body");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_prompt_overwrites_existing_file() {
        let dir = std::env::temp_dir().join(format!(
            "kobito-prompt-overwrite-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        fs::create_dir_all(&dir).unwrap();
        save_prompt(&dir, 1, "first").unwrap();
        save_prompt(&dir, 1, "second").unwrap();
        let body = fs::read_to_string(dir.join("iter-0001.md")).unwrap();
        assert_eq!(body, "second");
        fs::remove_dir_all(&dir).ok();
    }
}
