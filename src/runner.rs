use anyhow::{Context, Result, anyhow, bail};
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
        let (title, body) = ask_pr_metadata(&*agent_impl, &repo, &goal, &sink).await;
        Some(PrTracker::new(
            base_branch.clone(),
            &run,
            meta.clone(),
            title,
            body,
        ))
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
                &branch,
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
        let (title, body) = if new_meta.pr_url.is_none() {
            ask_pr_metadata(&*agent_impl, &repo, &prev_meta.goal, &sink).await
        } else {
            // PR already exists; title/body are unused for this run.
            (String::new(), String::new())
        };
        Some(PrTracker::new(
            resume_base.clone(),
            &new_run,
            new_meta.clone(),
            title,
            body,
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
                &prev_meta.branch,
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
    /// Title to use when the draft PR is opened. Generated upfront by
    /// the agent so the GitHub PR list shows something meaningful from
    /// the very first commit; the finalize phase can replace it later.
    pending_title: String,
    /// Body for the initial draft PR. Same story as `pending_title` —
    /// agent-suggested, replaced by the finalize phase.
    pending_body: String,
    /// Set after a `gh pr create` failure so we don't pester the user
    /// with the same error on every subsequent commit. From then on we
    /// just push (the local branch is the source of truth; the user
    /// can open the PR by hand).
    create_failed: bool,
}

impl<'a> PrTracker<'a> {
    pub fn new(
        base: String,
        run: &'a state::RunPaths,
        meta: state::RunMeta,
        pending_title: String,
        pending_body: String,
    ) -> Self {
        Self {
            base,
            run,
            meta,
            pending_title,
            pending_body,
            create_failed: false,
        }
    }

    /// Push the working branch (creating the draft PR on first call).
    /// All errors are reported via `sink` and swallowed — push/PR
    /// failures must not abort the surrounding loop.
    pub fn on_commit(&mut self, repo: &Path, branch: &str, sink: &LogSink) {
        if self.meta.pr_url.is_some() || self.create_failed {
            self.push_existing(repo, branch, sink);
        } else {
            self.create_draft(repo, branch, sink);
        }
    }

    fn create_draft(&mut self, repo: &Path, branch: &str, sink: &LogSink) {
        if let Err(e) = pr::push(repo, branch, true) {
            sink.note(&format!("✗ push failed: {e}"));
            self.create_failed = true;
            return;
        }
        let title = self.pending_title.clone();
        let body = self.pending_body.clone();
        match pr::create(repo, &self.base, branch, &title, &body, true) {
            Ok(url) => {
                sink.note(&format!("✓ PR opened (draft): {url}"));
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
                    tracker.on_commit(args.repo, args.branch, args.sink);
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

/// Cap on the review-fix-check loop. High enough that the agent can
/// chase a couple of stubborn lints / failing tests across rounds, but
/// low enough that a confused or stuck agent doesn't burn an unbounded
/// amount of time.
const MAX_FINALIZE_ROUNDS: u32 = 5;

#[allow(clippy::too_many_arguments)]
async fn finalize_run(
    agent: &dyn Agent,
    repo: &Path,
    branch: &str,
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
    sink.note(&format!(
        "finalize: review-fix-check loop, up to {MAX_FINALIZE_ROUNDS} rounds, until ready_for_review"
    ));

    let mut last_title: Option<String> = None;
    let mut last_body: Option<String> = None;

    for round in 1..=MAX_FINALIZE_ROUNDS {
        if cancelled.load(Ordering::SeqCst) {
            sink.note("finalize: cancelled");
            return;
        }
        sink.note(&format!(
            "── finalize round {round} / {MAX_FINALIZE_ROUNDS} ──"
        ));

        let outcome = match run_finalize_round(
            agent,
            repo,
            branch,
            goal,
            base_branch,
            tracker,
            sink,
            cancelled.clone(),
            round,
        )
        .await
        {
            Some(o) => o,
            None => return,
        };

        if let Some(t) = outcome.title {
            last_title = Some(t);
        }
        if let Some(b) = outcome.body {
            last_body = Some(b);
        }

        if outcome.ready {
            apply_pr_metadata(
                repo,
                &url,
                last_title.as_deref(),
                last_body.as_deref(),
                sink,
            );
            match pr::mark_ready(repo, &url) {
                Ok(_) => sink.note(&format!("✓ PR marked ready for review: {url}")),
                Err(e) => sink.note(&format!("✗ gh pr ready failed: {e}")),
            }
            return;
        }
        sink.note(&format!(
            "finalize: not ready yet (round {round}/{MAX_FINALIZE_ROUNDS}); continuing"
        ));
    }

    sink.note(&format!(
        "finalize: gave up after {MAX_FINALIZE_ROUNDS} rounds; leaving PR in draft for manual review"
    ));
    apply_pr_metadata(
        repo,
        &url,
        last_title.as_deref(),
        last_body.as_deref(),
        sink,
    );
}

#[derive(Debug)]
struct FinalizeRoundOutcome {
    ready: bool,
    title: Option<String>,
    body: Option<String>,
}

#[derive(Debug)]
struct FinalizeReply {
    outcome: FinalizeRoundOutcome,
    summary: Option<String>,
}

#[allow(clippy::too_many_arguments)]
async fn run_finalize_round(
    agent: &dyn Agent,
    repo: &Path,
    branch: &str,
    goal: &str,
    base_branch: &str,
    tracker: &mut PrTracker<'_>,
    sink: &LogSink,
    cancelled: Arc<AtomicBool>,
    round: u32,
) -> Option<FinalizeRoundOutcome> {
    let diff = match git::diff_against(repo, base_branch) {
        Ok(d) => d,
        Err(e) => {
            sink.note(&format!("finalize: git diff failed: {e}"));
            return None;
        }
    };
    let prompt_body = prompt::build_finalize_prompt(
        goal,
        &truncate_for_finalize(&diff),
        round,
        MAX_FINALIZE_ROUNDS,
    );
    let outcome = match agent::run(agent, repo, &prompt_body, sink, cancelled.clone()).await {
        Ok(o) => o,
        Err(e) => {
            sink.note(&format!("finalize: agent failed: {e}"));
            return None;
        }
    };

    if let Err(e) = commit_finalize_fixes(agent, repo, branch, goal, tracker, sink).await {
        sink.note(&format!("✗ finalize commit failed: {e}; stopping"));
        return None;
    }
    if cancelled.load(Ordering::SeqCst) {
        return None;
    }

    let text = outcome.final_message.as_deref()?;
    let reply = match parse_finalize_reply(text) {
        Ok(reply) => reply,
        Err(e) => {
            sink.note(&format!(
                "finalize: agent reply was not valid JSON ({e}); stopping"
            ));
            return None;
        }
    };
    if let Some(s) = reply.summary.as_deref() {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            sink.note(&format!("finalize: agent says — {trimmed}"));
        }
    }
    Some(reply.outcome)
}

fn parse_finalize_reply(text: &str) -> Result<FinalizeReply> {
    let body_str = agent::strip_code_fence(text.trim());
    let v: serde_json::Value = serde_json::from_str(&body_str)
        .with_context(|| format!("agent reply was not valid JSON: {body_str}"))?;
    let outcome = FinalizeRoundOutcome {
        ready: v
            .get("ready_for_review")
            .and_then(|x| x.as_bool())
            .unwrap_or(false),
        title: finalize_reply_title(&v),
        body: v
            .get("pr_body")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string()),
    };
    Ok(FinalizeReply {
        outcome,
        summary: finalize_reply_summary(&v),
    })
}

fn finalize_reply_title(v: &serde_json::Value) -> Option<String> {
    v.get("pr_title")
        .and_then(|x| x.as_str())
        .map(|s| s.trim().chars().take(72).collect::<String>())
        .filter(|s| !s.is_empty())
}

fn finalize_reply_summary(v: &serde_json::Value) -> Option<String> {
    v.get("summary")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
}

fn apply_pr_metadata(
    repo: &Path,
    url: &str,
    title: Option<&str>,
    body: Option<&str>,
    sink: &LogSink,
) {
    if title.is_none() && body.is_none() {
        return;
    }
    match pr::edit(repo, url, title, body) {
        Ok(_) => sink.note("✓ PR title/description updated"),
        Err(e) => sink.note(&format!("✗ gh pr edit failed: {e}")),
    }
}

async fn commit_finalize_fixes(
    agent: &dyn Agent,
    repo: &Path,
    branch: &str,
    goal: &str,
    tracker: &mut PrTracker<'_>,
    sink: &LogSink,
) -> Result<()> {
    git::stage_all(repo)?;
    if !git::has_staged_changes(repo)? {
        return Ok(());
    }
    let diff = git::diff_staged(repo)?;
    let style = git::recent_commit_messages(repo, 20).unwrap_or_default();
    let raw = commit::generate_message(agent, repo, &diff, goal, &style).await?;
    let msg = ensure_finalize_prefix(&raw);
    git::commit(repo, &msg)?;
    sink.note(&format!("✓ finalize commit: {}", first_line(&msg)));
    tracker.on_commit(repo, branch, sink);
    Ok(())
}

/// Enforce a `chore(finalize): …` subject on commits made during the
/// review-fix loop, regardless of what the commit-message agent
/// generated. Strips any pre-existing `<type>(<scope>): ` Conventional
/// Commits prefix so the result reads cleanly rather than nesting.
fn ensure_finalize_prefix(msg: &str) -> String {
    let trimmed = msg.trim_start();
    if trimmed.starts_with("chore(finalize)") {
        return trimmed.to_string();
    }
    let head_end = trimmed.find('\n').unwrap_or(trimmed.len());
    let (subject, rest) = trimmed.split_at(head_end);
    let body = subject_without_conventional_prefix(subject);
    format!("chore(finalize): {body}{rest}")
}

fn subject_without_conventional_prefix(subject: &str) -> &str {
    let Some((head, rest)) = subject.split_once(": ") else {
        return subject;
    };
    let typ = match head.split_once('(') {
        Some((typ, scope)) if scope.ends_with(')') => typ,
        Some(_) => return subject,
        None => head,
    };
    if typ.is_empty() || !typ.chars().all(|c| c.is_ascii_lowercase()) {
        return subject;
    }
    rest
}

fn confirm_finalize() -> bool {
    if !std::io::stdin().is_terminal() {
        return false;
    }
    use std::io::{self, Write};
    let prompt = crate::logger::style_kobito_prompt(
        " Finalize PR for review? Agent will review the diff, fix any issues, run quality gates (up to 5 rounds), then rewrite title/body and mark ready. [y/N] ",
    );
    print!("{prompt}");
    let _ = io::stdout().flush();
    let mut buf = String::new();
    if io::stdin().read_line(&mut buf).is_err() {
        return false;
    }
    matches!(buf.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// Wrapper around `suggest_pr_metadata` that logs progress and falls
/// back to a generic title + the goal-as-body when the agent fails.
async fn ask_pr_metadata(
    agent: &dyn Agent,
    repo: &Path,
    goal: &str,
    sink: &LogSink,
) -> (String, String) {
    sink.note("asking agent for draft PR title and description");
    match suggest_pr_metadata(agent, repo, goal).await {
        Ok((title, body)) => {
            sink.note(&format!("✓ PR title drafted: {title}"));
            (title, body)
        }
        Err(e) => {
            sink.note(&format!(
                "✗ PR metadata suggestion failed: {e}; falling back to generic title"
            ));
            (
                "kobito run".to_string(),
                format!("kobito is working on:\n\n{}", goal.trim()),
            )
        }
    }
}

/// Ask the agent for a short PR title + body to use for the draft PR
/// kobito opens at run start. We do this *once* per run (before the
/// first commit) so the GitHub PR list shows something meaningful from
/// the start; the finalize phase can replace both later.
async fn suggest_pr_metadata(
    agent: &dyn Agent,
    repo: &Path,
    goal: &str,
) -> Result<(String, String)> {
    let prompt = format!(
        "Suggest a draft GitHub Pull Request title and body for the goal below.\n\n\
The PR is being opened automatically at the **start** of an autonomous kobito \
run, before any commits land — the body will be rewritten by the agent during \
finalize. Keep this initial pair short and factual; describe what kobito will \
be working on, not what it has done.\n\n\
Reply with EXACTLY one JSON object, nothing else (no prose, no fence):\n\n\
{{\"title\": \"<≤72 chars, conventional-commit style headline>\",\n\
\"body\": \"<2-4 line markdown summary stating the goal in your own words>\"}}\n\n\
## Goal\n\n{goal}\n",
        goal = goal.trim(),
    );
    let raw = agent::run_oneshot(agent, repo, &prompt).await?;
    let body_str = agent::strip_code_fence(raw.trim());
    let v: serde_json::Value = serde_json::from_str(&body_str)
        .with_context(|| format!("agent reply was not valid JSON: {body_str}"))?;
    let title: String = v
        .get("title")
        .and_then(|x| x.as_str())
        .map(|s| s.trim().chars().take(72).collect())
        .filter(|s: &String| !s.is_empty())
        .unwrap_or_else(|| "kobito run".to_string());
    let body: String = v
        .get("body")
        .and_then(|x| x.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("kobito is working on:\n\n{}", goal.trim()));
    Ok((title, body))
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
    use crate::agent::AgentEvent;
    use std::path::PathBuf;
    use std::process;
    use tokio::process::Command;

    struct FakeAgent {
        script: &'static str,
    }

    impl Agent for FakeAgent {
        fn name(&self) -> &str {
            "fake"
        }

        fn build_streaming_command(&self, _: &str) -> Command {
            Command::new("true")
        }

        fn build_oneshot_command(&self, _: &str) -> Command {
            let mut cmd = Command::new("sh");
            cmd.arg("-c").arg(self.script);
            cmd
        }

        fn parse_event(&self, _: &str) -> Vec<agent::AgentEvent> {
            vec![]
        }
    }

    struct StreamingFakeAgent {
        script: &'static str,
        oneshot_script: &'static str,
    }

    impl Agent for StreamingFakeAgent {
        fn name(&self) -> &str {
            "fake"
        }

        fn build_streaming_command(&self, _: &str) -> Command {
            let mut cmd = Command::new("sh");
            cmd.arg("-c").arg(self.script);
            cmd
        }

        fn build_oneshot_command(&self, _: &str) -> Command {
            let mut cmd = Command::new("sh");
            cmd.arg("-c").arg(self.oneshot_script);
            cmd
        }

        fn parse_event(&self, line: &str) -> Vec<AgentEvent> {
            vec![AgentEvent::Message(line.to_string())]
        }
    }

    struct TempDir(PathBuf);

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn temp_run(prefix: &str) -> (TempDir, state::RunPaths, LogSink) {
        let nanos = Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let root =
            std::env::temp_dir().join(format!("kobito-runner-{prefix}-{}-{nanos}", process::id()));
        let project = state::ProjectPaths {
            id: "test-project".to_string(),
            root: root.join("project"),
        };
        let run = state::RunPaths {
            project,
            run_dir: root.join("run"),
            log_file: root.join("run/log.ndjson"),
            prompts_dir: root.join("run/prompts"),
            timestamp: "run-1".to_string(),
        };
        fs::create_dir_all(&run.prompts_dir).unwrap();
        fs::write(&run.log_file, "").unwrap();
        let sink = LogSink::open(&run.log_file, None).unwrap();
        (TempDir(root), run, sink)
    }

    fn flag(value: bool) -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(value))
    }

    fn temp_repo(run: &state::RunPaths) -> PathBuf {
        let repo = run.run_dir.parent().unwrap().join("repo");
        fs::create_dir_all(&repo).unwrap();
        git_cmd(&repo, &["init", "-b", "main"]);
        git_cmd(&repo, &["config", "user.email", "kobito@example.test"]);
        git_cmd(&repo, &["config", "user.name", "kobito"]);
        git_cmd(&repo, &["config", "commit.gpgsign", "false"]);
        fs::write(repo.join("README.md"), "start\n").unwrap();
        git_cmd(&repo, &["add", "-A"]);
        git_cmd(&repo, &["commit", "-m", "chore: initial"]);
        repo
    }

    fn git_cmd(repo: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .current_dir(repo)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {} failed", args.join(" "));
    }

    fn tracker_for(run: &state::RunPaths) -> PrTracker<'_> {
        PrTracker::new(
            "main".to_string(),
            run,
            state::RunMeta {
                run_id: run.timestamp.clone(),
                started_at: "2026-05-01T00:00:00Z".to_string(),
                branch: "feature/test".to_string(),
                goal: "increase coverage".to_string(),
                agent: "fake".to_string(),
                pr_url: Some("https://example.test/pull/1".to_string()),
                base_branch: Some("main".to_string()),
            },
            "draft title".to_string(),
            "draft body".to_string(),
        )
    }

    fn draft_tracker_for(run: &state::RunPaths) -> PrTracker<'_> {
        PrTracker::new(
            "main".to_string(),
            run,
            state::RunMeta {
                run_id: run.timestamp.clone(),
                started_at: "2026-05-01T00:00:00Z".to_string(),
                branch: "feature/test".to_string(),
                goal: "increase coverage".to_string(),
                agent: "fake".to_string(),
                pr_url: None,
                base_branch: Some("main".to_string()),
            },
            "draft title".to_string(),
            "draft body".to_string(),
        )
    }

    fn resume_project(root: &Path) -> state::ProjectPaths {
        let project = state::ProjectPaths {
            id: "test-project".to_string(),
            root: root.join("project"),
        };
        fs::create_dir_all(project.root.join("runs")).unwrap();
        project
    }

    fn write_resume_run(project: &state::ProjectPaths, run_id: &str, goal: &str) {
        let run_dir = project.root.join("runs").join(run_id);
        let run = state::RunPaths {
            project: project.clone(),
            run_dir: run_dir.clone(),
            log_file: run_dir.join("log.ndjson"),
            prompts_dir: run_dir.join("prompts"),
            timestamp: run_id.to_string(),
        };
        fs::create_dir_all(&run.prompts_dir).unwrap();
        state::write_run_meta(
            &run,
            &state::RunMeta {
                run_id: run_id.to_string(),
                started_at: "2026-05-01T00:00:00Z".to_string(),
                branch: "feature/test".to_string(),
                goal: goal.to_string(),
                agent: "fake".to_string(),
                pr_url: None,
                base_branch: Some("main".to_string()),
            },
        )
        .unwrap();
    }

    #[test]
    fn pick_run_to_resume_errors_when_project_has_no_runs() {
        let (_dir, run, _sink) = temp_run("resume-empty");
        let project = resume_project(run.run_dir.parent().unwrap());

        let err = pick_run_to_resume(&project).unwrap_err();

        assert!(err.to_string().contains("no previous runs to resume"));
    }

    #[test]
    fn pick_run_to_resume_returns_latest_run_without_terminal() {
        let (_dir, run, _sink) = temp_run("resume-latest");
        let project = resume_project(run.run_dir.parent().unwrap());
        write_resume_run(&project, "2026-05-01T00-00-00", "older goal");
        write_resume_run(&project, "2026-05-02T00-00-00", "newer goal");

        let selected = pick_run_to_resume(&project).unwrap();

        assert_eq!(selected, "2026-05-02T00-00-00");
    }

    #[test]
    fn pr_tracker_marks_create_failed_when_initial_push_fails() {
        let (_dir, run, sink) = temp_run("pr-tracker-push-failure");
        let repo = temp_repo(&run);
        let mut tracker = draft_tracker_for(&run);

        tracker.on_commit(&repo, "feature/test", &sink);

        assert!(tracker.create_failed);
        assert_eq!(tracker.meta.pr_url, None);
    }

    #[tokio::test]
    async fn run_iterations_stops_when_agent_reports_natural_stop() {
        let (_dir, run, sink) = temp_run("natural-stop");
        let agent = StreamingFakeAgent {
            script: r#"printf '{"natural_stop":true}\n'"#,
            oneshot_script: "true",
        };

        let completed = run_iterations(LoopArgs {
            agent: &agent,
            repo: Path::new("."),
            run: &run,
            branch: "feature/test",
            goal: "increase coverage",
            max_iterations: 3,
            max_failures: 1,
            sink: &sink,
            cancelled: flag(false),
            finalize_requested: flag(false),
            pr_tracker: None,
        })
        .await
        .unwrap();

        assert_eq!(completed, 0);
        assert!(run.prompts_dir.join("iter-0001.md").exists());
        assert!(!run.prompts_dir.join("iter-0002.md").exists());
    }

    #[tokio::test]
    async fn run_iterations_exits_before_prompt_when_finalize_requested() {
        let (_dir, run, sink) = temp_run("finalize-requested");
        let agent = StreamingFakeAgent {
            script: "exit 99",
            oneshot_script: "true",
        };

        let completed = run_iterations(LoopArgs {
            agent: &agent,
            repo: Path::new("."),
            run: &run,
            branch: "feature/test",
            goal: "increase coverage",
            max_iterations: 3,
            max_failures: 1,
            sink: &sink,
            cancelled: flag(false),
            finalize_requested: flag(true),
            pr_tracker: None,
        })
        .await
        .unwrap();

        assert_eq!(completed, 0);
        assert!(!run.prompts_dir.join("iter-0001.md").exists());
    }

    #[tokio::test]
    async fn run_iterations_exits_before_prompt_when_cancelled() {
        let (_dir, run, sink) = temp_run("cancelled-before-prompt");
        let agent = StreamingFakeAgent {
            script: "exit 99",
            oneshot_script: "true",
        };

        let completed = run_iterations(LoopArgs {
            agent: &agent,
            repo: Path::new("."),
            run: &run,
            branch: "feature/test",
            goal: "increase coverage",
            max_iterations: 3,
            max_failures: 1,
            sink: &sink,
            cancelled: flag(true),
            finalize_requested: flag(false),
            pr_tracker: None,
        })
        .await
        .unwrap();

        assert_eq!(completed, 0);
        assert!(!run.prompts_dir.join("iter-0001.md").exists());
    }

    #[tokio::test]
    async fn run_iterations_commits_agent_changes() {
        let (_dir, run, sink) = temp_run("commit-changes");
        let repo = temp_repo(&run);
        let agent = StreamingFakeAgent {
            script: r#"printf 'generated\n' > generated.txt; printf '{"natural_stop":false}\n'"#,
            oneshot_script: "printf 'increase coverage\n'",
        };

        let completed = run_iterations(LoopArgs {
            agent: &agent,
            repo: &repo,
            run: &run,
            branch: "feature/test",
            goal: "increase coverage",
            max_iterations: 1,
            max_failures: 1,
            sink: &sink,
            cancelled: flag(false),
            finalize_requested: flag(false),
            pr_tracker: None,
        })
        .await
        .unwrap();

        let commit_subject = std::process::Command::new("git")
            .current_dir(&repo)
            .args(["log", "-1", "--pretty=%s"])
            .output()
            .unwrap();

        assert_eq!(completed, 1);
        assert!(repo.join("generated.txt").exists());
        assert_eq!(
            String::from_utf8(commit_subject.stdout).unwrap().trim(),
            "increase coverage"
        );
        assert!(state::notes_path(&run).exists());
    }

    #[tokio::test]
    async fn run_iterations_skips_commit_when_agent_produces_no_diff() {
        let (_dir, run, sink) = temp_run("no-diff");
        let repo = temp_repo(&run);
        let agent = StreamingFakeAgent {
            script: r#"printf '{"natural_stop":false}\n'"#,
            oneshot_script: "exit 99",
        };

        let completed = run_iterations(LoopArgs {
            agent: &agent,
            repo: &repo,
            run: &run,
            branch: "feature/test",
            goal: "increase coverage",
            max_iterations: 2,
            max_failures: 1,
            sink: &sink,
            cancelled: flag(false),
            finalize_requested: flag(false),
            pr_tracker: None,
        })
        .await
        .unwrap();

        let commit_count = std::process::Command::new("git")
            .current_dir(&repo)
            .args(["rev-list", "--count", "HEAD"])
            .output()
            .unwrap();

        assert_eq!(completed, 0);
        assert_eq!(String::from_utf8(commit_count.stdout).unwrap().trim(), "1");
        assert!(run.prompts_dir.join("iter-0001.md").exists());
        assert!(run.prompts_dir.join("iter-0002.md").exists());
        assert!(!state::notes_path(&run).exists());
    }

    #[tokio::test]
    async fn run_iterations_resets_worktree_after_agent_failure() {
        let (_dir, run, sink) = temp_run("agent-failure");
        let repo = temp_repo(&run);
        let agent = StreamingFakeAgent {
            script: "printf 'changed\n' > README.md; exit 7",
            oneshot_script: "true",
        };

        let completed = run_iterations(LoopArgs {
            agent: &agent,
            repo: &repo,
            run: &run,
            branch: "feature/test",
            goal: "increase coverage",
            max_iterations: 3,
            max_failures: 1,
            sink: &sink,
            cancelled: flag(false),
            finalize_requested: flag(false),
            pr_tracker: None,
        })
        .await
        .unwrap();

        assert_eq!(completed, 0);
        assert_eq!(
            fs::read_to_string(repo.join("README.md")).unwrap(),
            "start\n"
        );
        assert!(run.prompts_dir.join("iter-0001.md").exists());
        assert!(!run.prompts_dir.join("iter-0002.md").exists());
    }

    #[tokio::test]
    async fn run_iterations_retries_after_transient_agent_failure() {
        let (_dir, run, sink) = temp_run("transient-agent-failure");
        let repo = temp_repo(&run);
        let agent = StreamingFakeAgent {
            script: "if [ ! -f attempt ]; then touch attempt; printf 'changed\n' > README.md; exit 7; fi; printf 'generated\n' > generated.txt; printf '{\"natural_stop\":false}\n'",
            oneshot_script: "printf 'test(runner): commit after retry\n'",
        };

        let completed = run_iterations(LoopArgs {
            agent: &agent,
            repo: &repo,
            run: &run,
            branch: "feature/test",
            goal: "increase coverage",
            max_iterations: 2,
            max_failures: 2,
            sink: &sink,
            cancelled: flag(false),
            finalize_requested: flag(false),
            pr_tracker: None,
        })
        .await
        .unwrap();
        let commit_subject = std::process::Command::new("git")
            .current_dir(&repo)
            .args(["log", "-1", "--pretty=%s"])
            .output()
            .unwrap();

        assert_eq!(completed, 1);
        assert_eq!(
            fs::read_to_string(repo.join("README.md")).unwrap(),
            "start\n"
        );
        assert_eq!(
            String::from_utf8(commit_subject.stdout).unwrap().trim(),
            "test(runner): commit after retry"
        );
        assert!(run.prompts_dir.join("iter-0001.md").exists());
        assert!(run.prompts_dir.join("iter-0002.md").exists());
    }

    #[tokio::test]
    async fn run_iterations_leaves_worktree_when_cancelled_during_agent_failure() {
        let (_dir, run, sink) = temp_run("cancelled-agent-failure");
        let repo = temp_repo(&run);
        let cancelled = flag(false);
        let agent = StreamingFakeAgent {
            script: "printf 'changed\n' > README.md; sleep 0.4; exit 7",
            oneshot_script: "true",
        };
        let cancel_task = {
            let cancelled = cancelled.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                cancelled.store(true, Ordering::SeqCst);
            })
        };

        let completed = run_iterations(LoopArgs {
            agent: &agent,
            repo: &repo,
            run: &run,
            branch: "feature/test",
            goal: "increase coverage",
            max_iterations: 3,
            max_failures: 1,
            sink: &sink,
            cancelled,
            finalize_requested: flag(false),
            pr_tracker: None,
        })
        .await
        .unwrap();
        cancel_task.await.unwrap();

        assert_eq!(completed, 0);
        assert_eq!(
            fs::read_to_string(repo.join("README.md")).unwrap(),
            "changed\n"
        );
        assert!(run.prompts_dir.join("iter-0001.md").exists());
        assert!(!run.prompts_dir.join("iter-0002.md").exists());
    }

    #[tokio::test]
    async fn run_finalize_round_returns_ready_metadata_from_agent_reply() {
        let (_dir, run, sink) = temp_run("finalize-ready");
        let repo = temp_repo(&run);
        let mut tracker = tracker_for(&run);
        let agent = StreamingFakeAgent {
            script: r#"printf '{"ready_for_review":true,"pr_title":" test(runner): finalize ","pr_body":"ready body","summary":"checked"}\n'"#,
            oneshot_script: "exit 99",
        };

        let outcome = run_finalize_round(
            &agent,
            &repo,
            "feature/test",
            "increase coverage",
            "main",
            &mut tracker,
            &sink,
            flag(false),
            1,
        )
        .await
        .unwrap();

        assert!(outcome.ready);
        assert_eq!(outcome.title.as_deref(), Some("test(runner): finalize"));
        assert_eq!(outcome.body.as_deref(), Some("ready body"));
    }

    #[tokio::test]
    async fn run_finalize_round_stops_on_invalid_agent_reply() {
        let (_dir, run, sink) = temp_run("finalize-invalid-json");
        let repo = temp_repo(&run);
        let mut tracker = tracker_for(&run);
        let agent = StreamingFakeAgent {
            script: "printf 'not json\n'",
            oneshot_script: "exit 99",
        };

        let outcome = run_finalize_round(
            &agent,
            &repo,
            "feature/test",
            "increase coverage",
            "main",
            &mut tracker,
            &sink,
            flag(false),
            1,
        )
        .await;

        assert!(outcome.is_none());
    }

    #[tokio::test]
    async fn run_finalize_round_commits_agent_fixes_with_finalize_prefix() {
        let (_dir, run, sink) = temp_run("finalize-commit-fixes");
        let repo = temp_repo(&run);
        let mut tracker = tracker_for(&run);
        let agent = StreamingFakeAgent {
            script: r#"printf 'fixed\n' > README.md; printf '{"ready_for_review":false,"summary":"fixed lint"}\n'"#,
            oneshot_script: "printf 'fix(runner): polish finalize path\n\nKeep the PR clean.\n'",
        };

        let outcome = run_finalize_round(
            &agent,
            &repo,
            "feature/test",
            "increase coverage",
            "main",
            &mut tracker,
            &sink,
            flag(false),
            1,
        )
        .await
        .unwrap();
        let commit_subject = std::process::Command::new("git")
            .current_dir(&repo)
            .args(["log", "-1", "--pretty=%s"])
            .output()
            .unwrap();

        assert!(!outcome.ready);
        assert_eq!(
            fs::read_to_string(repo.join("README.md")).unwrap(),
            "fixed\n"
        );
        assert_eq!(
            String::from_utf8(commit_subject.stdout).unwrap().trim(),
            "chore(finalize): polish finalize path"
        );
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
    fn ensure_finalize_prefix_passes_through_when_already_correct() {
        assert_eq!(
            ensure_finalize_prefix("chore(finalize): polish things"),
            "chore(finalize): polish things"
        );
    }

    #[test]
    fn ensure_finalize_prefix_replaces_existing_conventional_prefix() {
        assert_eq!(
            ensure_finalize_prefix("test(notes): cover edge case"),
            "chore(finalize): cover edge case"
        );
        assert_eq!(ensure_finalize_prefix("fix: oops"), "chore(finalize): oops");
    }

    #[test]
    fn ensure_finalize_prefix_prepends_when_no_conventional_type() {
        assert_eq!(
            ensure_finalize_prefix("Update foo"),
            "chore(finalize): Update foo"
        );
    }

    #[test]
    fn ensure_finalize_prefix_preserves_body() {
        let msg = "test(x): cover thing\n\nbody line\nanother";
        assert_eq!(
            ensure_finalize_prefix(msg),
            "chore(finalize): cover thing\n\nbody line\nanother"
        );
    }

    #[test]
    fn ensure_finalize_prefix_does_not_mistake_url_colon_for_conventional() {
        // Subjects with mixed case or `://` are not conventional commit
        // prefixes — leave them alone (just prepend).
        assert_eq!(
            ensure_finalize_prefix("https://example.com is a url"),
            "chore(finalize): https://example.com is a url"
        );
    }

    #[test]
    fn parse_finalize_reply_accepts_fenced_ready_response() {
        let reply = parse_finalize_reply(
            "```json\n{\"ready_for_review\":true,\"pr_title\":\" test(scope): cover x \",\"pr_body\":\"body\",\"summary\":\"done\"}\n```",
        )
        .unwrap();
        assert!(reply.outcome.ready);
        assert_eq!(reply.outcome.title.as_deref(), Some("test(scope): cover x"));
        assert_eq!(reply.outcome.body.as_deref(), Some("body"));
        assert_eq!(reply.summary.as_deref(), Some("done"));
    }

    #[test]
    fn parse_finalize_reply_defaults_missing_ready_to_false() {
        let reply = parse_finalize_reply("{\"pr_body\":\"body\"}").unwrap();
        assert!(!reply.outcome.ready);
        assert_eq!(reply.outcome.title, None);
        assert_eq!(reply.outcome.body.as_deref(), Some("body"));
    }

    #[test]
    fn parse_finalize_reply_drops_blank_title_and_truncates_long_title() {
        let blank = parse_finalize_reply("{\"pr_title\":\"   \"}").unwrap();
        let long =
            parse_finalize_reply(&format!("{{\"pr_title\":\"{}\"}}", "x".repeat(80))).unwrap();
        assert_eq!(blank.outcome.title, None);
        assert_eq!(long.outcome.title.unwrap().chars().count(), 72);
    }

    #[test]
    fn parse_finalize_reply_rejects_invalid_json() {
        let err = parse_finalize_reply("not json").unwrap_err();
        assert!(err.to_string().contains("agent reply was not valid JSON"));
    }

    #[tokio::test]
    async fn suggest_pr_metadata_accepts_fenced_json() {
        let agent = FakeAgent {
            script: r#"printf '```json\n{"title":" test(runner): cover metadata ","body":" body text "}\n```'"#,
        };
        let (title, body) = suggest_pr_metadata(&agent, Path::new("."), "increase coverage")
            .await
            .unwrap();

        assert_eq!(title, "test(runner): cover metadata");
        assert_eq!(body, "body text");
    }

    #[tokio::test]
    async fn suggest_pr_metadata_defaults_blank_fields() {
        let agent = FakeAgent {
            script: r#"printf '{"title":"   ","body":"   "}'"#,
        };
        let (title, body) = suggest_pr_metadata(&agent, Path::new("."), " increase coverage ")
            .await
            .unwrap();

        assert_eq!(title, "kobito run");
        assert_eq!(body, "kobito is working on:\n\nincrease coverage");
    }

    #[tokio::test]
    async fn suggest_pr_metadata_rejects_invalid_json() {
        let agent = FakeAgent {
            script: "printf 'not json'",
        };
        let err = suggest_pr_metadata(&agent, Path::new("."), "increase coverage")
            .await
            .unwrap_err();

        assert!(err.to_string().contains("agent reply was not valid JSON"));
    }

    #[tokio::test]
    async fn ask_pr_metadata_returns_agent_suggestion() {
        let (_dir, _run, sink) = temp_run("ask-pr-metadata-ok");
        let agent = FakeAgent {
            script: r#"printf '{"title":"test(runner): cover pr metadata","body":"planned work"}'"#,
        };

        let (title, body) =
            ask_pr_metadata(&agent, Path::new("."), "increase coverage", &sink).await;

        assert_eq!(title, "test(runner): cover pr metadata");
        assert_eq!(body, "planned work");
    }

    #[tokio::test]
    async fn ask_pr_metadata_falls_back_when_agent_fails() {
        let (_dir, _run, sink) = temp_run("ask-pr-metadata-fallback");
        let agent = FakeAgent {
            script: "printf 'not json'",
        };

        let (title, body) =
            ask_pr_metadata(&agent, Path::new("."), " increase coverage ", &sink).await;

        assert_eq!(title, "kobito run");
        assert_eq!(body, "kobito is working on:\n\nincrease coverage");
    }
}
