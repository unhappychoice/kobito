use anyhow::{Result, bail};
use std::path::Path;
use std::process::Command as StdCommand;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::cli::IterationArgs;
use crate::{agent, commit, git, logger::LogSink, prompt, state, tasks::Backlog, ui};

pub async fn run(args: IterationArgs) -> Result<()> {
    let repo = git::repo_root()?;
    if !args.allow_dirty {
        git::ensure_clean(&repo)?;
    }

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
        let slug = slug::slugify(body).chars().take(40).collect::<String>();
        let branch = format!("kobito/task-{task_idx}-{slug}");

        if let Err(e) = git::checkout(&repo, &starting_branch) {
            bar.println(format!("✗ failed to return to {starting_branch}: {e}"));
            continue;
        }
        if let Err(e) = git::create_and_checkout(&repo, &branch) {
            bar.println(format!("✗ branch create failed: {e}; skipping task: {body}"));
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

        let started = Instant::now();
        let mut consecutive_failures = 0u32;
        let mut completed = false;

        for iteration in 1..=args.max_iterations {
            if cancelled.load(Ordering::SeqCst) {
                break;
            }
            ui::set_status(
                &bar,
                iteration,
                started.elapsed(),
                consecutive_failures,
                &format!("task {}/{}", task_idx, pending.len()),
            );
            let parts = prompt::PromptParts {
                goal: body.clone(),
                iteration,
                notes: None,
            };
            let prompt_body = prompt::build_task_prompt(&parts, body);
            prompt::save_prompt(&run_dirs.prompts_dir, iteration, &prompt_body)?;

            match agent::invoke_claude(&repo, &prompt_body, &sink).await {
                Ok(out) => {
                    consecutive_failures = 0;
                    git::stage_all(&repo)?;
                    if git::has_staged_changes(&repo)? {
                        let diff = git::diff_staged(&repo)?;
                        let style = git::recent_commit_messages(&repo, 20).unwrap_or_default();
                        let msg = commit::generate_message(&repo, &diff, body, &style).await?;
                        git::commit(&repo, &msg)?;
                        sink.note(&format!(
                            "✓ committed: {}",
                            msg.lines().next().unwrap_or("")
                        ));
                    } else {
                        sink.note("no diff this iteration");
                    }
                    if out.stdout.contains("TASK_COMPLETE") {
                        sink.note("agent reported TASK_COMPLETE");
                        completed = true;
                        break;
                    }
                }
                Err(e) => {
                    consecutive_failures += 1;
                    sink.note(&format!("✗ failed: {e}"));
                    git::reset_hard(&repo).ok();
                    if consecutive_failures >= args.max_failures {
                        sink.note("giving up on this task");
                        break;
                    }
                    let backoff =
                        std::time::Duration::from_secs(2u64.pow(consecutive_failures));
                    tokio::time::sleep(backoff).await;
                }
            }
        }

        if completed {
            match push_and_open_pr(&repo, &branch, body, &starting_branch).await {
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

async fn push_and_open_pr(repo: &Path, branch: &str, title: &str, base: &str) -> Result<String> {
    let push = StdCommand::new("git")
        .current_dir(repo)
        .args(["push", "-u", "origin", branch])
        .output()?;
    if !push.status.success() {
        bail!(
            "git push failed: {}",
            String::from_utf8_lossy(&push.stderr)
        );
    }

    let pr_body = format!("Automated PR generated by kobito.\n\nTask: {title}\n");
    let pr = StdCommand::new("gh")
        .current_dir(repo)
        .args([
            "pr", "create", "--base", base, "--head", branch, "--title", title, "--body",
            &pr_body,
        ])
        .output()?;
    if !pr.status.success() {
        bail!(
            "gh pr create failed — is the `gh` CLI installed and authenticated? {}",
            String::from_utf8_lossy(&pr.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&pr.stdout).trim().to_string())
}
