use anyhow::{Result, bail};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::cli::IterationArgs;
use crate::{
    agent, branch, commit, git, logger::LogSink, notes, pr, preset, prompt, state, tasks::Backlog,
    ui,
};

pub async fn run(args: IterationArgs) -> Result<()> {
    let agent_impl = agent::from_name(&args.agent)?;
    let repo = git::repo_root()?;
    if !args.allow_dirty {
        git::ensure_clean(&repo)?;
    }

    let preset_body = match &args.preset {
        Some(name) => {
            let vars = preset::parse_vars(&args.vars)?;
            Some(preset::load(name, &repo, &vars)?)
        }
        None => {
            if !args.vars.is_empty() {
                bail!("--var requires --preset");
            }
            None
        }
    };

    let remote = git::remote_url(&repo);
    let id = state::project_id(&repo, remote.as_deref());
    let project = state::project_paths(id)?;

    let tasks_state_path = state::tasks_path(&project);
    if let Some(backlog_arg) = &args.backlog {
        let body = std::fs::read_to_string(backlog_arg)?;
        std::fs::write(&tasks_state_path, body)?;
    } else {
        state::seed_tasks_if_needed(&project, &repo)?;
    }

    let mut backlog = Backlog::from_file(&tasks_state_path)?;
    let pending: Vec<(usize, String)> = backlog
        .pending()
        .iter()
        .map(|t| (t.line_no, t.body.clone()))
        .collect();

    if pending.is_empty() {
        println!("no pending tasks in {}", tasks_state_path.display());
        return Ok(());
    }

    let starting_branch = git::current_branch(&repo)?;
    let _cursor = ui::CursorGuard::new();
    let bar = ui::make_status_bar();

    let cancelled = Arc::new(AtomicBool::new(false));
    {
        let f = cancelled.clone();
        let _ = ctrlc::set_handler(move || f.store(true, Ordering::SeqCst));
    }

    for (n, (line_no, body)) in pending.iter().enumerate() {
        if cancelled.load(Ordering::SeqCst) {
            break;
        }
        let task_idx = n + 1;
        let suggested = branch::suggest(&*agent_impl, &repo, body)
            .await
            .unwrap_or_else(|_| {
                let slug = slug::slugify(body).chars().take(40).collect::<String>();
                format!("kobito/task-{task_idx}-{slug}")
            });
        let branch = format!("{}-task-{task_idx}", suggested);

        if let Err(e) = git::checkout(&repo, &starting_branch) {
            bar.println(format!("✗ failed to return to {starting_branch}: {e}"));
            continue;
        }
        if let Err(e) = git::create_and_checkout(&repo, &branch) {
            bar.println(format!(
                "✗ branch create failed: {e}; skipping task: {body}"
            ));
            continue;
        }

        let run_dirs = state::new_run(project.clone())?;
        let sink = LogSink::open(&run_dirs.log_file, Some(bar.clone()))?;
        sink.note(&format!(
            "=== task {}/{}: {} ===",
            task_idx,
            pending.len(),
            body
        ));

        let mut consecutive_failures = 0u32;
        let mut completed = false;

        for iteration in 1..=args.max_iterations {
            if cancelled.load(Ordering::SeqCst) {
                break;
            }
            sink.note(&format!(
                "── iteration {iteration} / {} ──",
                args.max_iterations
            ));
            sink.set_iteration_status(
                iteration,
                consecutive_failures,
                &format!("task {}/{}", task_idx, pending.len()),
            );
            let notes = std::fs::read_to_string(state::notes_path(&run_dirs)).ok();
            let parts = prompt::PromptParts {
                goal: body.clone(),
                iteration,
                notes,
                preset: preset_body.clone(),
            };
            let prompt_body = prompt::build_task_prompt(&parts, body);
            prompt::save_prompt(&run_dirs.prompts_dir, iteration, &prompt_body)?;

            match agent::run(&*agent_impl, &repo, &prompt_body, &sink, cancelled.clone()).await {
                Ok(out) => {
                    consecutive_failures = 0;
                    git::stage_all(&repo)?;
                    if git::has_staged_changes(&repo)? {
                        let diff = git::diff_staged(&repo)?;
                        let style = git::recent_commit_messages(&repo, 20).unwrap_or_default();
                        let msg =
                            commit::generate_message(&*agent_impl, &repo, &diff, body, &style)
                                .await?;
                        git::commit(&repo, &msg)?;
                        sink.note(&format!(
                            "✓ committed: {}",
                            msg.lines().next().unwrap_or("")
                        ));

                        let notes_path = state::notes_path(&run_dirs);
                        if let Err(e) = notes::append_learning(
                            &*agent_impl,
                            &repo,
                            &notes_path,
                            iteration,
                            body,
                            &diff,
                        )
                        .await
                        {
                            sink.note(&format!("notes update failed: {e}"));
                        }
                    } else {
                        sink.note("no diff this iteration");
                    }
                    sink.note(&format!(
                        "  tokens — in {} · out {} · cached {}",
                        out.usage.input_tokens,
                        out.usage.output_tokens,
                        out.usage.cached_input_tokens,
                    ));
                    if out.task_complete {
                        sink.note("agent reported task_complete");
                        completed = true;
                        break;
                    }
                }
                Err(e) => {
                    if cancelled.load(Ordering::SeqCst) {
                        // User cancellation — bail out of the
                        // per-task loop without resetting the worktree
                        // or counting it as a failure.
                        break;
                    }
                    consecutive_failures += 1;
                    sink.note(&format!("✗ failed: {e}"));
                    git::reset_hard(&repo).ok();
                    if consecutive_failures >= args.max_failures {
                        sink.note("giving up on this task");
                        break;
                    }
                    let backoff = std::time::Duration::from_secs(2u64.pow(consecutive_failures));
                    tokio::time::sleep(backoff).await;
                }
            }
        }

        if completed {
            match open_task_pr(&repo, &branch, body, &starting_branch) {
                Ok(url) => {
                    sink.note(&format!("✓ PR: {url}"));
                    backlog.mark_completed(*line_no);
                    backlog.write(&tasks_state_path)?;
                }
                Err(e) => {
                    sink.note(&format!("✗ PR creation failed: {e}"));
                }
            }
        } else {
            sink.note(&format!(
                "task did not complete; leaving branch {branch} for inspection"
            ));
        }

        git::checkout(&repo, &starting_branch).ok();
    }

    bar.finish_and_clear();
    println!("done — backlog state: {}", tasks_state_path.display());
    Ok(())
}

fn open_task_pr(repo: &std::path::Path, branch: &str, title: &str, base: &str) -> Result<String> {
    pr::push(repo, branch, true)?;
    let body = format!("Automated PR generated by kobito.\n\nTask: {title}\n");
    pr::create(repo, base, branch, title, &body, false)
}
