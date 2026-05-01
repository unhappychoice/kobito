use anyhow::{Result, anyhow, bail};
use chrono::Utc;
use std::fs;
use std::io::IsTerminal;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::agent::Agent;
use crate::cli::{ContinuousArgs, ResumeArgs};
use crate::{agent, branch, commit, git, logger::LogSink, notes, pr, preset, prompt, state, ui};

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

    let base_branch = git::default_remote_branch(&repo);
    let suggested = branch::suggest(&*agent_impl, &repo, &goal)
        .await
        .unwrap_or_else(|_| format!("kobito/{}", slugify(&goal)));
    let branch = format!(
        "{}-{ts}",
        suggested,
        ts = Utc::now().format("%Y%m%d-%H%M%S")
    );
    git::create_and_checkout(&repo, &branch)?;

    let meta = state::RunMeta {
        run_id: run.timestamp.clone(),
        started_at: Utc::now().to_rfc3339(),
        branch: branch.clone(),
        goal: goal.clone(),
        agent: args.agent.clone(),
        pr_url: None,
        base_branch: Some(base_branch.clone()),
    };
    state::write_run_meta(&run, &meta)?;

    let _cursor = ui::CursorGuard::new();
    let bar = ui::make_status_bar();
    let sink = LogSink::open(&run.log_file, Some(bar.clone()))?;
    let cancel = install_cancel_handler();

    sink.note(&format!("kobito start: {}", first_line(&goal)));
    sink.note(&format!(
        "project: {id}  branch: {branch}  run: {}",
        run.timestamp
    ));

    let mut pr_tracker = if remote.is_some() {
        Some(PrTracker::new(base_branch.clone(), &run, meta.clone()))
    } else {
        sink.note("no git remote configured — skipping PR creation and pushes");
        None
    };

    let completed = run_iterations(LoopArgs {
        agent: &*agent_impl,
        repo: &repo,
        run: &run,
        branch: &branch,
        goal: &goal,
        max_iterations: args.max_iterations,
        max_failures: args.max_failures,
        sink: &sink,
        cancelled: cancel.cancelled.clone(),
        finalize_requested: cancel.finalize_requested.clone(),
        pr_tracker: pr_tracker.as_mut(),
    })
    .await?;

    bar.finish_and_clear();
    if cancel.finalize_requested.load(Ordering::SeqCst) && !cancel.cancelled.load(Ordering::SeqCst)
    {
        if let Some(tracker) = pr_tracker.as_mut() {
            finalize_run(
                &*agent_impl,
                &repo,
                &goal,
                &base_branch,
                tracker,
                &sink,
                cancel.cancelled.clone(),
            )
            .await;
        } else {
            sink.note("finalize requested — no PR open, exiting cleanly");
        }
    }

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
    let new_meta = state::RunMeta {
        run_id: new_run.timestamp.clone(),
        started_at: Utc::now().to_rfc3339(),
        branch: prev_meta.branch.clone(),
        goal: prev_meta.goal.clone(),
        agent: prev_meta.agent.clone(),
        pr_url: prev_meta.pr_url.clone(),
        base_branch: prev_meta.base_branch.clone(),
    };
    state::write_run_meta(&new_run, &new_meta)?;

    let _cursor = ui::CursorGuard::new();
    let bar = ui::make_status_bar();
    let sink = LogSink::open(&new_run.log_file, Some(bar.clone()))?;
    let cancel = install_cancel_handler();

    sink.note(&format!(
        "kobito resume from {target_id}: {}",
        first_line(&prev_meta.goal)
    ));
    sink.note(&format!(
        "project: {id}  branch: {}  resumed run: {}",
        prev_meta.branch, new_run.timestamp
    ));

    let resume_base = prev_meta
        .base_branch
        .clone()
        .unwrap_or_else(|| git::default_remote_branch(&repo));
    let mut pr_tracker = if remote.is_some() {
        Some(PrTracker::new(
            resume_base.clone(),
            &new_run,
            new_meta.clone(),
        ))
    } else {
        sink.note("no git remote configured — skipping PR creation and pushes");
        None
    };

    let completed = run_iterations(LoopArgs {
        agent: &*agent_impl,
        repo: &repo,
        run: &new_run,
        branch: &prev_meta.branch,
        goal: &prev_meta.goal,
        max_iterations: args.max_iterations,
        max_failures: args.max_failures,
        sink: &sink,
        cancelled: cancel.cancelled.clone(),
        finalize_requested: cancel.finalize_requested.clone(),
        pr_tracker: pr_tracker.as_mut(),
    })
    .await?;

    bar.finish_and_clear();
    if cancel.finalize_requested.load(Ordering::SeqCst) && !cancel.cancelled.load(Ordering::SeqCst)
    {
        if let Some(tracker) = pr_tracker.as_mut() {
            finalize_run(
                &*agent_impl,
                &repo,
                &prev_meta.goal,
                &resume_base,
                tracker,
                &sink,
                cancel.cancelled.clone(),
            )
            .await;
        } else {
            sink.note("finalize requested — no PR open, exiting cleanly");
        }
    }

    sink.note(&format!(
        "done — {completed} commits on {} (run dir: {})",
        prev_meta.branch,
        new_run.run_dir.display()
    ));
    Ok(())
}

struct LoopArgs<'a, 'b> {
    agent: &'a dyn Agent,
    repo: &'a Path,
    run: &'a state::RunPaths,
    branch: &'a str,
    goal: &'a str,
    max_iterations: u32,
    max_failures: u32,
    sink: &'a LogSink,
    cancelled: Arc<AtomicBool>,
    finalize_requested: Arc<AtomicBool>,
    pr_tracker: Option<&'a mut PrTracker<'b>>,
}

pub struct PrTracker<'a> {
    base: String,
    run: &'a state::RunPaths,
    meta: state::RunMeta,
    /// Set after a `gh pr create` failure so we don't pester the user
    /// with the same error on every subsequent commit. From then on we
    /// just push (the local branch is the source of truth; the user
    /// can open the PR by hand).
    create_failed: bool,
}

impl<'a> PrTracker<'a> {
    pub fn new(base: String, run: &'a state::RunPaths, meta: state::RunMeta) -> Self {
        Self {
            base,
            run,
            meta,
            create_failed: false,
        }
    }

    /// Push the working branch (creating the draft PR on first call).
    /// All errors are reported via `sink` and swallowed — push/PR
    /// failures must not abort the surrounding loop.
    pub fn on_commit(&mut self, repo: &Path, branch: &str, goal: &str, sink: &LogSink) {
        if self.meta.pr_url.is_some() || self.create_failed {
            self.push_existing(repo, branch, sink);
        } else {
            self.create_draft(repo, branch, goal, sink);
        }
    }

    fn create_draft(&mut self, repo: &Path, branch: &str, goal: &str, sink: &LogSink) {
        if let Err(e) = pr::push(repo, branch, true) {
            sink.note(&format!("✗ push failed: {e}"));
            self.create_failed = true;
            return;
        }
        let title: String = goal
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("kobito run")
            .chars()
            .take(72)
            .collect();
        let body = format!(
            "Automated draft PR opened by kobito.\n\n## Goal\n\n{}\n",
            goal.trim()
        );
        match pr::create(repo, &self.base, branch, &title, &body, true) {
            Ok(url) => {
                sink.note(&format!("✓ draft PR: {url}"));
                self.meta.pr_url = Some(url);
                if let Err(e) = state::write_run_meta(self.run, &self.meta) {
                    sink.note(&format!("✗ persist meta failed: {e}"));
                }
            }
            Err(e) => {
                sink.note(&format!(
                    "✗ draft PR create failed: {e} (will keep pushing without retrying)"
                ));
                self.create_failed = true;
            }
        }
    }

    fn push_existing(&self, repo: &Path, branch: &str, sink: &LogSink) {
        let set_upstream = self.meta.pr_url.is_none();
        if let Err(e) = pr::push(repo, branch, set_upstream) {
            sink.note(&format!("✗ push failed: {e}"));
        }
    }
}

async fn run_iterations(mut args: LoopArgs<'_, '_>) -> Result<u32> {
    let mut consecutive_failures = 0u32;
    let mut total_retries = 0u32;
    let mut completed = 0u32;

    for iteration in 1..=args.max_iterations {
        if args.cancelled.load(Ordering::SeqCst) {
            args.sink.note("interrupted by user");
            break;
        }
        if args.finalize_requested.load(Ordering::SeqCst) {
            args.sink
                .note("finalize requested (Ctrl+C) — exiting iteration loop");
            break;
        }
        args.sink.note(&format!(
            "═══ iteration {iteration} / {} ═══",
            args.max_iterations
        ));
        args.sink
            .set_iteration_status(iteration, total_retries, "thinking");

        let notes = fs::read_to_string(state::notes_path(args.run)).ok();
        let parts = prompt::PromptParts {
            goal: args.goal.to_string(),
            iteration,
            notes,
            preset: None,
        };
        let body = prompt::build_iteration_prompt(&parts);
        prompt::save_prompt(&args.run.prompts_dir, iteration, &body)?;

        match agent::run(
            args.agent,
            args.repo,
            &body,
            args.sink,
            args.cancelled.clone(),
        )
        .await
        {
            Ok(out) => {
                consecutive_failures = 0;
                if out.natural_stop {
                    args.sink
                        .note("agent reported natural_stop — exiting cleanly");
                    break;
                }
                git::stage_all(args.repo)?;
                if !git::has_staged_changes(args.repo)? {
                    args.sink
                        .note("iteration produced no diff — skipping commit");
                    continue;
                }
                args.sink
                    .set_iteration_status(iteration, total_retries, "committing");
                let diff = git::diff_staged(args.repo)?;
                let style = git::recent_commit_messages(args.repo, 20).unwrap_or_default();
                let msg = commit::generate_message(args.agent, args.repo, &diff, args.goal, &style)
                    .await?;
                git::commit(args.repo, &msg)?;
                args.sink
                    .note(&format!("✓ committed: {}", first_line(&msg)));
                args.sink
                    .note(&format!("  tokens — {}", format_usage(&out.usage)));
                completed += 1;

                if let Some(tracker) = args.pr_tracker.as_deref_mut() {
                    tracker.on_commit(args.repo, args.branch, args.goal, args.sink);
                }

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
                if args.cancelled.load(Ordering::SeqCst) {
                    // User cancellation — leave the working tree
                    // alone and exit the loop, don't count it as a
                    // failure or burn a retry / backoff.
                    break;
                }
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
                let backoff = std::time::Duration::from_secs(2u64.pow(consecutive_failures));
                tokio::time::sleep(backoff).await;
            }
        }
    }

    let _ = args.branch;
    Ok(completed)
}

async fn finalize_run(
    agent: &dyn Agent,
    repo: &Path,
    goal: &str,
    base_branch: &str,
    tracker: &mut PrTracker<'_>,
    sink: &LogSink,
    cancelled: Arc<AtomicBool>,
) {
    let url = match tracker.meta.pr_url.clone() {
        Some(url) => url,
        None => {
            sink.note("finalize: no PR URL recorded; skipping");
            return;
        }
    };
    if !confirm_finalize() {
        sink.note("finalize: declined; PR left in draft");
        return;
    }
    sink.note("finalize: asking agent for PR description and ready-flag");
    let diff = match git::diff_against(repo, base_branch) {
        Ok(d) => d,
        Err(e) => {
            sink.note(&format!("finalize: git diff failed: {e}"));
            return;
        }
    };
    let prompt_body = prompt::build_finalize_prompt(goal, &truncate_for_finalize(&diff));
    let outcome = match agent::run(agent, repo, &prompt_body, sink, cancelled).await {
        Ok(o) => o,
        Err(e) => {
            sink.note(&format!("finalize: agent failed: {e}"));
            return;
        }
    };
    let Some(text) = outcome.final_message.as_deref() else {
        sink.note("finalize: agent returned no message; PR left in draft");
        return;
    };
    let body_str = agent::strip_code_fence(text.trim());
    let v: serde_json::Value = match serde_json::from_str(&body_str) {
        Ok(v) => v,
        Err(e) => {
            sink.note(&format!(
                "finalize: agent reply was not valid JSON ({e}); PR left in draft"
            ));
            return;
        }
    };
    let ready = v
        .get("ready_for_review")
        .and_then(|x| x.as_bool())
        .unwrap_or(false);
    let pr_body = v
        .get("pr_body")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    let summary = v.get("summary").and_then(|x| x.as_str()).unwrap_or("");
    if !summary.is_empty() {
        sink.note(&format!("finalize: agent says — {summary}"));
    }
    if let Some(body) = pr_body {
        if let Err(e) = pr::edit_body(repo, &url, &body) {
            sink.note(&format!("✗ gh pr edit failed: {e}"));
            return;
        }
        sink.note("✓ PR description updated");
    }
    if ready {
        match pr::mark_ready(repo, &url) {
            Ok(_) => sink.note(&format!("✓ PR marked ready for review: {url}")),
            Err(e) => sink.note(&format!("✗ gh pr ready failed: {e}")),
        }
    } else {
        sink.note("agent says not ready — PR left in draft");
    }
}

fn confirm_finalize() -> bool {
    if !std::io::stdin().is_terminal() {
        return false;
    }
    dialoguer::Confirm::new()
        .with_prompt("Finalize PR for review? (no = exit, leave PR draft)")
        .default(false)
        .interact()
        .unwrap_or(false)
}

fn truncate_for_finalize(s: &str) -> String {
    const MAX: usize = 20_000;
    let total = s.chars().count();
    if total <= MAX {
        return s.to_string();
    }
    let keep = MAX / 2;
    let head: String = s.chars().take(keep).collect();
    let tail: String = s.chars().skip(total - keep).collect();
    format!("{head}\n…\n[diff truncated]\n…\n{tail}")
}

/// Two-stage cancellation: the first Ctrl+C raises the
/// `finalize_requested` flag (a soft signal for the run loop to wrap
/// up after the current iteration commits), and any subsequent Ctrl+C
/// raises `cancelled` (a hard signal that propagates into
/// `agent::run` and aborts immediately).
pub struct CancelState {
    pub finalize_requested: Arc<AtomicBool>,
    pub cancelled: Arc<AtomicBool>,
}

fn install_cancel_handler() -> CancelState {
    let finalize_requested = Arc::new(AtomicBool::new(false));
    let cancelled = Arc::new(AtomicBool::new(false));
    let f = finalize_requested.clone();
    let c = cancelled.clone();
    let _ = ctrlc::set_handler(move || {
        if !f.swap(true, Ordering::SeqCst) {
            // first press — soft signal, let in-flight iteration finish
        } else {
            c.store(true, Ordering::SeqCst);
        }
    });
    CancelState {
        finalize_requested,
        cancelled,
    }
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
            let goal = first_line(&r.meta.goal)
                .chars()
                .take(60)
                .collect::<String>();
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

fn format_usage(u: &crate::agent::Usage) -> String {
    format!(
        "in {} · out {} · cached {}",
        format_count(u.input_tokens),
        format_count(u.output_tokens),
        format_count(u.cached_input_tokens),
    )
}

fn format_count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Usage;
    use std::fs;
    use std::path::PathBuf;

    struct TempDir(PathBuf);
    impl std::ops::Deref for TempDir {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn unique_tmp(label: &str) -> TempDir {
        let dir = std::env::temp_dir().join(format!(
            "kobito-runner-{label}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
        ));
        fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    fn init_repo(label: &str) -> TempDir {
        let dir = unique_tmp(label);
        let cfg = |args: &[&str]| {
            std::process::Command::new("git")
                .current_dir(&*dir)
                .args(args)
                .output()
                .unwrap();
        };
        cfg(&["init", "-q", "-b", "main"]);
        cfg(&["config", "user.email", "test@example.com"]);
        cfg(&["config", "user.name", "Test"]);
        cfg(&["config", "commit.gpgsign", "false"]);
        dir
    }

    fn run_paths_in(state_dir: &Path) -> state::RunPaths {
        let run_dir = state_dir.join("runs/r1");
        let prompts_dir = run_dir.join("prompts");
        fs::create_dir_all(&prompts_dir).unwrap();
        let log_file = run_dir.join("log.ndjson");
        fs::write(&log_file, "").unwrap();
        state::RunPaths {
            project: state::ProjectPaths {
                id: "p".to_string(),
                root: state_dir.to_path_buf(),
            },
            run_dir,
            log_file,
            prompts_dir,
            timestamp: "r1".to_string(),
        }
    }

    fn sample_meta(pr_url: Option<&str>) -> state::RunMeta {
        state::RunMeta {
            run_id: "r1".to_string(),
            started_at: "2026-05-01T00:00:00Z".to_string(),
            branch: "feature/x".to_string(),
            goal: "do thing".to_string(),
            agent: "claude".to_string(),
            pr_url: pr_url.map(|s| s.to_string()),
            base_branch: Some("main".to_string()),
        }
    }

    #[test]
    fn truncate_for_finalize_passes_short_input_through() {
        assert_eq!(truncate_for_finalize("short"), "short");
    }

    #[test]
    fn truncate_for_finalize_keeps_head_tail_and_marker_for_long_input() {
        let head = "H".repeat(15_000);
        let tail = "T".repeat(15_000);
        let input = format!("{head}MIDDLE{tail}");
        let out = truncate_for_finalize(&input);
        assert!(out.starts_with(&"H".repeat(10_000)));
        assert!(out.ends_with(&"T".repeat(10_000)));
        assert!(out.contains("[diff truncated]"));
        assert!(!out.contains("MIDDLE"));
    }

    #[test]
    fn truncate_for_finalize_handles_multibyte_input() {
        let head = "あ".repeat(15_000);
        let tail = "い".repeat(15_000);
        let input = format!("{head}MIDDLE{tail}");
        let out = truncate_for_finalize(&input);
        assert!(out.starts_with(&"あ".repeat(10_000)));
        assert!(out.ends_with(&"い".repeat(10_000)));
        assert!(out.contains("[diff truncated]"));
    }

    #[test]
    fn slugify_lowercases_and_kebab_cases() {
        assert_eq!(slugify("Increase Test Coverage"), "increase-test-coverage");
    }

    #[test]
    fn slugify_truncates_to_40_chars() {
        let long = "a".repeat(100);
        let out = slugify(&long);
        assert_eq!(out.chars().count(), 40);
    }

    #[test]
    fn slugify_collapses_punctuation_and_whitespace() {
        let out = slugify("hello,   world!!!");
        assert!(out.contains("hello"));
        assert!(out.contains("world"));
        assert!(!out.contains(' '));
        assert!(!out.contains(','));
    }

    #[test]
    fn first_line_returns_empty_for_empty_input() {
        assert_eq!(first_line(""), "");
    }

    #[test]
    fn first_line_returns_only_the_first_line() {
        assert_eq!(first_line("alpha\nbeta\ngamma"), "alpha");
    }

    #[test]
    fn first_line_returns_whole_input_when_single_line() {
        assert_eq!(first_line("just one line"), "just one line");
    }

    #[test]
    fn format_count_renders_compact_units() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1_000), "1.0k");
        assert_eq!(format_count(1_500), "1.5k");
        assert_eq!(format_count(1_000_000), "1.0M");
        assert_eq!(format_count(2_500_000), "2.5M");
    }

    #[test]
    fn format_usage_includes_all_three_token_counts() {
        let u = Usage {
            input_tokens: 1_500,
            output_tokens: 2_500,
            cached_input_tokens: 800,
        };
        let s = format_usage(&u);
        assert!(s.contains("in 1.5k"));
        assert!(s.contains("out 2.5k"));
        assert!(s.contains("cached 800"));
    }

    #[test]
    fn pr_tracker_new_stores_base_and_meta() {
        let state_dir = unique_tmp("tracker-new");
        let run = run_paths_in(&state_dir);
        let meta = sample_meta(None);
        let tracker = PrTracker::new("main".to_string(), &run, meta.clone());
        assert_eq!(tracker.base, "main");
        assert_eq!(tracker.meta.branch, meta.branch);
        assert!(!tracker.create_failed);
    }

    #[test]
    fn pr_tracker_on_commit_logs_push_failure_when_no_remote_and_no_pr_yet() {
        let repo = init_repo("tracker-create-draft");
        let state_dir = unique_tmp("tracker-state-1");
        let run = run_paths_in(&state_dir);
        let log_path = run.log_file.clone();
        let sink = LogSink::open(&log_path, None).unwrap();
        let mut tracker = PrTracker::new("main".to_string(), &run, sample_meta(None));

        tracker.on_commit(&repo, "feature/x", "make a change", &sink);

        let body = fs::read_to_string(&log_path).unwrap();
        assert!(
            body.contains("push failed"),
            "expected push failure note, got: {body}"
        );
        assert!(tracker.create_failed);
        assert!(tracker.meta.pr_url.is_none());
    }

    #[test]
    fn pr_tracker_on_commit_takes_push_existing_path_when_pr_url_set() {
        let repo = init_repo("tracker-push-existing");
        let state_dir = unique_tmp("tracker-state-2");
        let run = run_paths_in(&state_dir);
        let log_path = run.log_file.clone();
        let sink = LogSink::open(&log_path, None).unwrap();
        let mut tracker = PrTracker::new(
            "main".to_string(),
            &run,
            sample_meta(Some("https://example.invalid/owner/repo/pull/1")),
        );

        tracker.on_commit(&repo, "feature/x", "goal", &sink);

        let body = fs::read_to_string(&log_path).unwrap();
        assert!(body.contains("push failed"));
        assert!(!tracker.create_failed);
    }

    #[test]
    fn pr_tracker_on_commit_after_create_failed_uses_push_existing() {
        let repo = init_repo("tracker-after-fail");
        let state_dir = unique_tmp("tracker-state-3");
        let run = run_paths_in(&state_dir);
        let log_path = run.log_file.clone();
        let sink = LogSink::open(&log_path, None).unwrap();
        let mut tracker = PrTracker::new("main".to_string(), &run, sample_meta(None));

        tracker.on_commit(&repo, "feature/x", "goal", &sink);
        assert!(tracker.create_failed);

        tracker.on_commit(&repo, "feature/x", "goal", &sink);

        let body = fs::read_to_string(&log_path).unwrap();
        let push_failed_count = body.matches("push failed").count();
        assert!(push_failed_count >= 2);
    }
}
