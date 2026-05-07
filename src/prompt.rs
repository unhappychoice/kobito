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

## What kobito owns — do not do these yourself

kobito owns the entire git and GitHub lifecycle for this run. **You must not** \
run any of the following, even if a project skill or memory file suggests \
otherwise; if you do, kobito's bookkeeping breaks:

- `git commit`, `git add` (kobito stages and commits your diff for you)
- `git push`, `git pull`, `git fetch`, branch creation/deletion, force-push
- `gh pr create`, `gh pr edit`, `gh pr merge`, `gh pr review`, `gh pr close`, \
  `gh pr ready`
- Any equivalent invocation via API, MCP, scripts, or alternative CLIs

Just edit files in the working tree and exit. The orchestrator commits, \
pushes, opens the draft PR, runs the finalize phase, and decides if and when \
to mark the PR ready for review. Merging is a human's decision; never merge.

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

## What kobito owns — do not do these yourself

kobito owns the entire git and GitHub lifecycle for this task. **You must not** \
run any of the following, even if a project skill or memory file suggests \
otherwise; if you do, kobito's bookkeeping breaks:

- `git commit`, `git add` (kobito stages and commits your diff for you)
- `git push`, `git pull`, `git fetch`, branch creation/deletion, force-push
- `gh pr create`, `gh pr edit`, `gh pr merge`, `gh pr review`, `gh pr close`, \
  `gh pr ready`
- Any equivalent invocation via API, MCP, scripts, or alternative CLIs

Just edit files in the working tree and exit. The orchestrator commits, \
pushes, opens the draft PR, and decides if and when to mark the PR ready for \
review. Merging is a human's decision; never merge.

## How to stop

Your **final message** in this iteration MUST be a single JSON object — nothing else, no prose, no code fence, no commentary:

```
{\"task_complete\": <bool>, \"summary\": \"<one-line summary of what you did this iteration>\"}
```

- Set `task_complete` to `true` whenever the task is already done — including when previous iterations finished it before this one started. If `git log` on this branch and the cross-iteration notes show the work has landed and there is no further focused change for you to make, return `true` immediately. Do not spend an iteration re-verifying completed work; that is what is currently failing this run.
- Set `task_complete` to `false` only when you produced (or are about to produce) a focused change that the orchestrator should commit, with more work still pending for the next iteration.
- The JSON is the *only* signal the orchestrator looks at. Discussing or quoting `task_complete` in earlier messages — including legacy uppercase sentinels like `TASK_COMPLETE` — has no effect. Only this final JSON is parsed.

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
        "First, check `git log` on this branch and the notes above to decide whether this task was already finished by a previous iteration. If it was, your only job is to return `task_complete: true` — do not produce a diff or run verification commands. Otherwise, make one focused change toward completing the task and return `task_complete: false` so the orchestrator can commit it and run the next iteration.\n",
    );

    out
}

pub fn save_prompt(prompts_dir: &Path, iteration: u32, body: &str) -> Result<()> {
    let path = prompts_dir.join(format!("iter-{iteration:04}.md"));
    fs::write(path, body)?;
    Ok(())
}

pub fn build_finalize_prompt(goal: &str, diff: &str, round: u32, max_rounds: u32) -> String {
    format!(
        "\
# Finalize this kobito run — round {round} / {max_rounds}

The user just hit Ctrl+C and asked to wrap the run up by handing the \
PR to a human reviewer. The orchestrator is now driving a \
**review-fix-check loop** that calls you up to {max_rounds} times. The goal \
is to land on `ready_for_review: true` — not to give up. Each round you \
reduce the gap; do not declare defeat just because you didn't finish in \
one shot.

## What to do

1. **Read the full branch diff** (below) the same way a reviewer would: \
   look for half-finished refactors, untested new paths, debug prints, \
   `TODO(me)` notes, formatting drift, dead code.
2. **Run the project's quality gates** — whatever AGENTS.md / CLAUDE.md \
   describes for this repo. Tests, linter, formatter, type-check. Run \
   them, don't guess.
3. **Fix anything broken or ugly.** You CAN edit files; the orchestrator \
   will stage everything you change and turn it into a single \
   `chore(finalize): …` commit on this branch before opening the PR \
   for review. Re-run the gates until they pass.
4. **Write the PR description** based on the (possibly amended) branch \
   diff. The PR title and body kobito drafted at run start are \
   placeholders; you are replacing both.

If a gate fails this round and you can't fix it cleanly, say so in \
`summary` and set `ready_for_review: false` — the orchestrator will \
call you again with the updated diff so you can keep at it. Only the \
final round (or genuine impossibility) should leave the PR in draft.

## Original goal

{goal}

## Full branch diff (pre-review)

```diff
{diff}
```

## Your final response — strict format

Reply with **exactly one JSON object**, nothing else (no prose, no fence):

```
{{
  \"ready_for_review\": <bool>,
  \"pr_title\": \"<short PR title, ≤72 chars, conventional-commit style>\",
  \"pr_body\": \"<markdown PR description: Summary section, Test plan checklist describing the gates you ran, any follow-ups>\",
  \"summary\": \"<one-line note for kobito's own log — mention if you fixed anything during review>\"
}}
```

- Set `ready_for_review` to `true` only when (a) the gates pass and (b) \
  the branch diff is coherent enough that a human reviewer would not \
  waste their time on it. Otherwise set it to `false` and explain why \
  in `summary`.
- The JSON is the only signal kobito parses. Discussing the field names \
  in earlier output is fine; only the *final* message is read.
",
        goal = goal.trim(),
        diff = diff,
    )
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
    fn task_prompt_ends_with_completion_check_directive() {
        let out = build_task_prompt(&parts("g", 1), "task");
        let trimmed = out.trim_end();
        assert!(
            trimmed.ends_with(
                "Otherwise, make one focused change toward completing the task and return `task_complete: false` so the orchestrator can commit it and run the next iteration."
            ),
            "prompt should end with the completion-check directive: {trimmed}",
        );
        assert!(
            trimmed.contains("`task_complete: true`"),
            "directive should tell the agent how to short-circuit when the task is already done: {trimmed}",
        );
    }

    #[test]
    fn iteration_prompt_forbids_agent_from_driving_git_or_pr_lifecycle() {
        let out = build_iteration_prompt(&parts("g", 1));
        let lower = out.to_lowercase();
        assert!(
            lower.contains("kobito owns"),
            "cont prompt should claim ownership of git/PR lifecycle: {out}",
        );
        for forbidden in [
            "`git commit`",
            "`git push`",
            "`gh pr create`",
            "`gh pr merge`",
        ] {
            assert!(
                out.contains(forbidden),
                "cont prompt should forbid {forbidden}: {out}",
            );
        }
        assert!(
            lower.contains("never merge"),
            "cont prompt should explicitly forbid merging: {out}",
        );
    }

    #[test]
    fn task_prompt_forbids_agent_from_driving_git_or_pr_lifecycle() {
        let out = build_task_prompt(&parts("g", 1), "task");
        let lower = out.to_lowercase();
        assert!(
            lower.contains("kobito owns"),
            "iter prompt should claim ownership of git/PR lifecycle: {out}",
        );
        for forbidden in [
            "`git commit`",
            "`git push`",
            "`gh pr create`",
            "`gh pr merge`",
        ] {
            assert!(
                out.contains(forbidden),
                "iter prompt should forbid {forbidden}: {out}",
            );
        }
        assert!(
            lower.contains("never merge"),
            "iter prompt should explicitly forbid merging: {out}",
        );
    }

    #[test]
    fn task_prompt_tells_agent_to_short_circuit_when_prior_iterations_finished_the_task() {
        // Regression: the previous prompt told the agent to "Set
        // task_complete to false after making a small focused change",
        // which biased it toward grinding verification iterations after
        // the task was already done. The new prompt must explicitly
        // cover the already-done case so iter does not waste budget.
        let out = build_task_prompt(&parts("g", 1), "task");
        let lower = out.to_lowercase();
        assert!(
            lower.contains("previous iterations"),
            "prompt should reference prior iteration completion: {out}",
        );
        assert!(
            lower.contains("do not spend an iteration re-verifying"),
            "prompt should forbid the verify-loop pattern: {out}",
        );
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
    fn finalize_prompt_includes_goal_diff_and_json_contract() {
        let out = build_finalize_prompt("ship the feature", "diff body", 1, 5);
        assert!(out.contains("# Finalize this kobito run"));
        assert!(out.contains("ship the feature"));
        assert!(out.contains("diff body"));
        assert!(out.contains("\"ready_for_review\""));
        assert!(out.contains("\"pr_body\""));
        assert!(out.contains("\"summary\""));
    }

    #[test]
    fn finalize_prompt_trims_goal() {
        let out = build_finalize_prompt("  padded  \n\n", "diff", 1, 5);
        assert!(out.contains("padded"));
        assert!(!out.contains("  padded  "));
    }

    #[test]
    fn finalize_prompt_instructs_agent_to_review_fix_and_run_gates() {
        let out = build_finalize_prompt("g", "d", 1, 5);
        // The release-readiness pass should explicitly cover review,
        // fix, and quality gates — not just "write a PR description".
        assert!(out.to_lowercase().contains("quality gate"));
        assert!(out.to_lowercase().contains("fix"));
        assert!(out.contains("chore(finalize)"));
    }

    #[test]
    fn finalize_prompt_advertises_round_count_and_loop_intent() {
        let out = build_finalize_prompt("g", "d", 2, 5);
        // Agent should know it's mid-loop and the orchestrator will
        // call again — so it doesn't preemptively give up with
        // ready_for_review = false.
        assert!(out.contains("round 2 / 5"));
        assert!(out.to_lowercase().contains("review-fix-check"));
        assert!(out.to_lowercase().contains("call you again"));
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
