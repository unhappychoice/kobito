use anyhow::{Result, anyhow, bail};
use chrono::Utc;
use indicatif::ProgressBar;
use std::fs;
use std::io::IsTerminal;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::agent::Agent;
use crate::cli::{ContinuousArgs, ResumeArgs};
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

    state::write_run_meta(
        &run,
        &state::RunMeta {
            run_id: run.timestamp.clone(),
            started_at: Utc::now().to_rfc3339(),
            branch: branch.clone(),
            goal: goal.clone(),
            agent: args.agent.clone(),
        },
    )?;

    let bar = ui::make_status_bar();
    let sink = LogSink::open(&run.log_file, Some(bar.clone()))?;
    let cancelled = install_cancel_handler();

    sink.note(&format!("kobito start: {}", first_line(&goal)));
    sink.note(&format!(
        "project: {id}  branch: {branch}  run: {}",
        run.timestamp
    ));

    let completed = run_iterations(LoopArgs {
        agent: &*agent_impl,
        repo: &repo,
        run: &run,
        branch: &branch,
        goal: &goal,
        max_iterations: args.max_iterations,
        max_failures: args.max_failures,
        sink: &sink,
        bar: &bar,
        cancelled,
    })
    .await?;

    bar.finish_and_clear();
    sink.note(&format!(
        "done — {completed} commits on {branch} (run dir: {})",
        run.run_dir.display()
    ));
    Ok(())
}

pub async fn resume_continuous(args: ResumeArgs) -> Result<()> {
    let repo = git::repo_root()?;
    if !args.allow_dirty {
        git::ensure_clean(&repo)?;
    }

    let remote = git::remote_url(&repo);
    let id = state::project_id(&repo, remote.as_deref());
    let project = state::project_paths(id.clone())?;

    let target_id = match args.run.as_ref() {
        Some(s) => s.clone(),
        None => pick_run_to_resume(&project)?,
    };
    let (prev_meta, prev_run) = state::read_run_meta(&project, &target_id)?;
    let agent_impl = agent::from_name(&prev_meta.agent)?;

    git::checkout(&repo, &prev_meta.branch)?;

    let new_run = state::new_run(project.clone())?;
    let prev_notes = state::notes_path(&prev_run);
    if prev_notes.exists() {
        fs::copy(&prev_notes, state::notes_path(&new_run))?;
    }
    state::write_run_meta(
        &new_run,
        &state::RunMeta {
            run_id: new_run.timestamp.clone(),
            started_at: Utc::now().to_rfc3339(),
            branch: prev_meta.branch.clone(),
            goal: prev_meta.goal.clone(),
            agent: prev_meta.agent.clone(),
        },
    )?;

    let bar = ui::make_status_bar();
    let sink = LogSink::open(&new_run.log_file, Some(bar.clone()))?;
    let cancelled = install_cancel_handler();

    sink.note(&format!(
        "kobito resume from {target_id}: {}",
        first_line(&prev_meta.goal)
    ));
    sink.note(&format!(
        "project: {id}  branch: {}  resumed run: {}",
        prev_meta.branch, new_run.timestamp
    ));

    let completed = run_iterations(LoopArgs {
        agent: &*agent_impl,
        repo: &repo,
        run: &new_run,
        branch: &prev_meta.branch,
        goal: &prev_meta.goal,
        max_iterations: args.max_iterations,
        max_failures: args.max_failures,
        sink: &sink,
        bar: &bar,
        cancelled,
    })
    .await?;

    bar.finish_and_clear();
    sink.note(&format!(
        "done — {completed} commits on {} (run dir: {})",
        prev_meta.branch,
        new_run.run_dir.display()
    ));
    Ok(())
}

struct LoopArgs<'a> {
    agent: &'a dyn Agent,
    repo: &'a Path,
    run: &'a state::RunPaths,
    branch: &'a str,
    goal: &'a str,
    max_iterations: u32,
    max_failures: u32,
    sink: &'a LogSink,
    bar: &'a ProgressBar,
    cancelled: Arc<AtomicBool>,
}

async fn run_iterations(args: LoopArgs<'_>) -> Result<u32> {
    let started = Instant::now();
    let mut consecutive_failures = 0u32;
    let mut total_retries = 0u32;
    let mut completed = 0u32;

    for iteration in 1..=args.max_iterations {
        if args.cancelled.load(Ordering::SeqCst) {
            args.sink.note("interrupted by user");
            break;
        }
        ui::set_status(args.bar, iteration, started.elapsed(), total_retries, "thinking");

        let notes = fs::read_to_string(state::notes_path(args.run)).ok();
        let parts = prompt::PromptParts {
            goal: args.goal.to_string(),
            iteration,
            notes,
            preset: None,
        };
        let body = prompt::build_iteration_prompt(&parts);
        prompt::save_prompt(&args.run.prompts_dir, iteration, &body)?;

        match agent::run(args.agent, args.repo, &body, args.sink).await {
            Ok(out) => {
                consecutive_failures = 0;
                if out.natural_stop {
                    args.sink
                        .note("agent reported NATURAL_STOP — exiting cleanly");
                    break;
                }
                git::stage_all(args.repo)?;
                if !git::has_staged_changes(args.repo)? {
                    args.sink.note("iteration produced no diff — skipping commit");
                    continue;
                }
                ui::set_status(
                    args.bar,
                    iteration,
                    started.elapsed(),
                    total_retries,
                    "committing",
                );
                let diff = git::diff_staged(args.repo)?;
                let style = git::recent_commit_messages(args.repo, 20).unwrap_or_default();
                let msg =
                    commit::generate_message(args.agent, args.repo, &diff, args.goal, &style)
                        .await?;
                git::commit(args.repo, &msg)?;
                args.sink
                    .note(&format!("✓ committed: {}", first_line(&msg)));
                completed += 1;

                let notes_path = state::notes_path(args.run);
                if let Err(e) = notes::append_learning(
                    args.agent,
                    args.repo,
                    &notes_path,
                    iteration,
                    args.goal,
                    &diff,
                )
                .await
                {
                    args.sink.note(&format!("notes update failed: {e}"));
                }
            }
            Err(e) => {
                consecutive_failures += 1;
                total_retries += 1;
                args.sink.note(&format!("✗ iteration failed: {e}"));
                git::reset_hard(args.repo).ok();
                if consecutive_failures >= args.max_failures {
                    args.sink.note(&format!(
                        "giving up after {consecutive_failures} consecutive failures"
                    ));
                    break;
                }
                let backoff =
                    std::time::Duration::from_secs(2u64.pow(consecutive_failures));
                tokio::time::sleep(backoff).await;
            }
        }
    }

    let _ = args.branch;
    Ok(completed)
}

fn install_cancel_handler() -> Arc<AtomicBool> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = cancelled.clone();
    let _ = ctrlc::set_handler(move || flag.store(true, Ordering::SeqCst));
    cancelled
}

fn pick_run_to_resume(project: &state::ProjectPaths) -> Result<String> {
    let recent = state::recent_runs(project, 10)?;
    if recent.is_empty() {
        return Err(anyhow!(
            "no previous runs to resume in {}",
            project.root.display()
        ));
    }
    if recent.len() == 1 || !std::io::stdin().is_terminal() {
        return Ok(recent.into_iter().next().unwrap().id);
    }
    let labels: Vec<String> = recent
        .iter()
        .map(|r| {
            let goal = first_line(&r.meta.goal).chars().take(60).collect::<String>();
            format!("{}  [{}]  {goal}", r.id, r.meta.branch)
        })
        .collect();
    let selection = dialoguer::Select::new()
        .with_prompt("Resume which run?")
        .items(&labels)
        .default(0)
        .interact()?;
    Ok(recent[selection].id.clone())
}

fn slugify(s: &str) -> String {
    let raw = slug::slugify(s);
    raw.chars().take(40).collect::<String>()
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("")
}
