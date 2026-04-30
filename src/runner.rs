use anyhow::{Result, bail};
use chrono::Utc;
use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::cli::ContinuousArgs;
use crate::{agent, branch, commit, git, logger::LogSink, notes, preset, prompt, state, ui};

pub async fn run_continuous(args: ContinuousArgs) -> Result<()> {
    let agent_impl = agent::from_name(&args.agent)?;
    let repo = git::repo_root()?;
    if !args.allow_dirty {
        git::ensure_clean(&repo)?;
    }

    let goal = match (args.prompt.as_ref(), args.preset.as_ref()) {
        (Some(p), None) => p.clone(),
        (None, Some(name)) => {
            let vars = preset::parse_vars(&args.vars)?;
            preset::load(name, &repo, &vars)?
        }
        _ => bail!("either --prompt or --preset is required (not both)"),
    };
    if args.preset.is_none() && !args.vars.is_empty() {
        bail!("--var requires --preset");
    }

    let remote = git::remote_url(&repo);
    let id = state::project_id(&repo, remote.as_deref());
    let project = state::project_paths(id.clone())?;
    let run = state::new_run(project.clone())?;

    let suggested = branch::suggest(&*agent_impl, &repo, &goal)
        .await
        .unwrap_or_else(|_| format!("kobito/{}", slugify(&goal)));
    let branch = format!(
        "{}-{ts}",
        suggested,
        ts = Utc::now().format("%Y%m%d-%H%M%S")
    );
    git::create_and_checkout(&repo, &branch)?;

    state::write_current_run(
        &project,
        &state::CurrentRun {
            run_id: run.timestamp.clone(),
            started_at: Utc::now().to_rfc3339(),
            branch: branch.clone(),
            goal: goal.clone(),
            agent: args.agent.clone(),
        },
    )?;

    let bar = ui::make_status_bar();
    let sink = LogSink::open(&run.log_file, Some(bar.clone()))?;

    let cancelled = Arc::new(AtomicBool::new(false));
    {
        let flag = cancelled.clone();
        let _ = ctrlc::set_handler(move || flag.store(true, Ordering::SeqCst));
    }

    sink.note(&format!("kobito start: {}", first_line(&goal)));
    sink.note(&format!("project: {id}  branch: {branch}"));

    let started = Instant::now();
    let mut consecutive_failures = 0u32;
    let mut total_retries = 0u32;
    let mut completed = 0u32;

    for iteration in 1..=args.max_iterations {
        if cancelled.load(Ordering::SeqCst) {
            sink.note("interrupted by user");
            break;
        }
        ui::set_status(&bar, iteration, started.elapsed(), total_retries, "thinking");

        let notes = fs::read_to_string(state::notes_path(&project)).ok();
        let parts = prompt::PromptParts {
            goal: goal.clone(),
            iteration,
            notes,
            preset: None,
        };
        let body = prompt::build_iteration_prompt(&parts);
        prompt::save_prompt(&run.prompts_dir, iteration, &body)?;

        let outcome = agent::run(&*agent_impl, &repo, &body, &sink).await;

        match outcome {
            Ok(out) => {
                consecutive_failures = 0;
                if out.natural_stop {
                    sink.note("agent reported NATURAL_STOP — exiting cleanly");
                    break;
                }
                git::stage_all(&repo)?;
                if !git::has_staged_changes(&repo)? {
                    sink.note("iteration produced no diff — skipping commit");
                    continue;
                }
                ui::set_status(&bar, iteration, started.elapsed(), total_retries, "committing");
                let diff = git::diff_staged(&repo)?;
                let style = git::recent_commit_messages(&repo, 20).unwrap_or_default();
                let msg =
                    commit::generate_message(&*agent_impl, &repo, &diff, &goal, &style).await?;
                git::commit(&repo, &msg)?;
                sink.note(&format!("✓ committed: {}", first_line(&msg)));
                completed += 1;

                let notes_path = state::notes_path(&project);
                if let Err(e) = notes::append_learning(
                    &*agent_impl,
                    &repo,
                    &notes_path,
                    iteration,
                    &goal,
                    &diff,
                )
                .await
                {
                    sink.note(&format!("notes update failed: {e}"));
                }
            }
            Err(e) => {
                consecutive_failures += 1;
                total_retries += 1;
                sink.note(&format!("✗ iteration failed: {e}"));
                git::reset_hard(&repo).ok();
                if consecutive_failures >= args.max_failures {
                    sink.note(&format!(
                        "giving up after {consecutive_failures} consecutive failures"
                    ));
                    break;
                }
                let backoff = std::time::Duration::from_secs(2u64.pow(consecutive_failures));
                tokio::time::sleep(backoff).await;
            }
        }
    }

    bar.finish_and_clear();
    state::clear_current_run(&project)?;
    sink.note(&format!(
        "done — {completed} commits on {branch} (run dir: {})",
        run.run_dir.display()
    ));
    Ok(())
}

fn slugify(s: &str) -> String {
    let raw = slug::slugify(s);
    raw.chars().take(40).collect::<String>()
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("")
}
