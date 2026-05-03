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
            .unwrap_or_else(|_| fallback_branch_name(task_idx, body));
        let branch = task_branch_name(&suggested, task_idx);

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
    let body = task_pr_body(title);
    pr::create(repo, base, branch, title, &body, false)
}

fn fallback_branch_name(task_idx: usize, body: &str) -> String {
    let slug = slug::slugify(body).chars().take(40).collect::<String>();
    format!("kobito/task-{task_idx}-{slug}")
}

fn task_branch_name(suggested: &str, task_idx: usize) -> String {
    format!("{suggested}-task-{task_idx}")
}

fn task_pr_body(title: &str) -> String {
    format!("Automated PR generated by kobito.\n\nTask: {title}\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn fallback_branch_name_uses_task_index_and_slugifies_body() {
        let branch = fallback_branch_name(3, "Fix flaky tests in runner.rs!");

        assert_eq!(branch, "kobito/task-3-fix-flaky-tests-in-runner-rs");
    }

    #[test]
    fn fallback_branch_name_truncates_slug_to_40_chars() {
        let branch = fallback_branch_name(12, "one two three four five six seven eight nine ten");

        assert_eq!(
            branch,
            "kobito/task-12-one-two-three-four-five-six-seven-eight-"
        );
    }

    #[test]
    fn task_branch_name_appends_task_suffix_to_agent_suggestion() {
        let branch = task_branch_name("feature/add-coverage", 2);

        assert_eq!(branch, "feature/add-coverage-task-2");
    }

    #[test]
    fn task_pr_body_includes_title_in_standard_template() {
        let body = task_pr_body("Cover iteration branch fallback");

        assert_eq!(
            body,
            "Automated PR generated by kobito.\n\nTask: Cover iteration branch fallback\n",
        );
    }

    #[tokio::test]
    async fn run_processes_pending_task_without_completion() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_dir = std::env::current_dir().unwrap();
        let old_path = std::env::var_os("PATH");
        let old_xdg = std::env::var_os("XDG_STATE_HOME");
        let dir = unique_tmp("pending-task");
        let repo = dir.0.join("repo");
        let state_home = dir.0.join("state");
        let backlog = dir.0.join("tasks.md");
        let bin = dir.0.join("bin");

        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(&backlog, "- [ ] Cover iteration run path\n").unwrap();
        write_fake_codex(&bin);
        init_repo(&repo);

        set_env("XDG_STATE_HOME", Some(state_home.as_os_str()));
        set_env(
            "PATH",
            Some(prefixed_path(&bin, old_path.as_deref()).as_os_str()),
        );
        std::env::set_current_dir(&repo).unwrap();

        let result = run(IterationArgs {
            backlog: Some(backlog),
            preset: None,
            vars: vec![],
            max_iterations: 1,
            max_failures: 1,
            agent: "codex".into(),
            allow_dirty: false,
        })
        .await;
        let project = single_project_under(&state_home);

        std::env::set_current_dir(old_dir).unwrap();
        set_env("PATH", old_path.as_deref());
        set_env("XDG_STATE_HOME", old_xdg.as_deref());

        result.unwrap();
        assert_eq!(git::current_branch(&repo).unwrap(), "main");
        assert_branch_exists(&repo, "feature/from-fake-task-1");

        let tasks = std::fs::read_to_string(state::tasks_path(&project)).unwrap();
        assert_eq!(tasks, "- [ ] Cover iteration run path\n");
        assert_saved_prompt_mentions_task(&project, "Cover iteration run path");
    }

    #[tokio::test]
    async fn run_uses_fallback_branch_when_agent_suggests_blank() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_dir = std::env::current_dir().unwrap();
        let old_path = std::env::var_os("PATH");
        let old_xdg = std::env::var_os("XDG_STATE_HOME");
        let dir = unique_tmp("fallback-branch");
        let repo = dir.0.join("repo");
        let state_home = dir.0.join("state");
        let backlog = dir.0.join("tasks.md");
        let bin = dir.0.join("bin");

        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(&backlog, "- [ ] Use fallback branch name\n").unwrap();
        write_fake_codex_with_branch(&bin, "   ");
        init_repo(&repo);

        set_env("XDG_STATE_HOME", Some(state_home.as_os_str()));
        set_env(
            "PATH",
            Some(prefixed_path(&bin, old_path.as_deref()).as_os_str()),
        );
        std::env::set_current_dir(&repo).unwrap();

        let result = run(IterationArgs {
            backlog: Some(backlog),
            preset: None,
            vars: vec![],
            max_iterations: 1,
            max_failures: 1,
            agent: "codex".into(),
            allow_dirty: false,
        })
        .await;

        std::env::set_current_dir(old_dir).unwrap();
        set_env("PATH", old_path.as_deref());
        set_env("XDG_STATE_HOME", old_xdg.as_deref());

        result.unwrap();
        assert_eq!(git::current_branch(&repo).unwrap(), "main");
        assert_branch_exists(&repo, "kobito/task-1-use-fallback-branch-name-task-1");
    }

    #[tokio::test]
    async fn run_commits_completed_task_and_marks_backlog_done() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_dir = std::env::current_dir().unwrap();
        let old_path = std::env::var_os("PATH");
        let old_xdg = std::env::var_os("XDG_STATE_HOME");
        let dir = unique_tmp("completed-task");
        let repo = dir.0.join("repo");
        let remote = dir.0.join("origin.git");
        let state_home = dir.0.join("state");
        let backlog = dir.0.join("tasks.md");
        let bin = dir.0.join("bin");

        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(&backlog, "- [ ] Complete iteration task\n").unwrap();
        write_completing_fake_codex(&bin);
        write_fake_gh(&bin);
        init_repo(&repo);
        std::fs::write(repo.join(".git").join("kobito-test-gh-success"), "").unwrap();
        init_bare_remote(&remote);
        run_git(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );

        set_env("XDG_STATE_HOME", Some(state_home.as_os_str()));
        set_env(
            "PATH",
            Some(prefixed_path(&bin, old_path.as_deref()).as_os_str()),
        );
        std::env::set_current_dir(&repo).unwrap();

        let result = run(IterationArgs {
            backlog: Some(backlog),
            preset: None,
            vars: vec![],
            max_iterations: 1,
            max_failures: 1,
            agent: "codex".into(),
            allow_dirty: false,
        })
        .await;
        let project = single_project_under(&state_home);

        std::env::set_current_dir(old_dir).unwrap();
        set_env("PATH", old_path.as_deref());
        set_env("XDG_STATE_HOME", old_xdg.as_deref());

        result.unwrap();
        assert_eq!(git::current_branch(&repo).unwrap(), "main");
        assert_branch_exists(&repo, "feature/completed-task-1");
        assert_eq!(
            std::fs::read_to_string(state::tasks_path(&project)).unwrap(),
            "- [x] Complete iteration task\n",
        );
        assert_eq!(
            git_output(
                &repo,
                &["log", "feature/completed-task-1", "-1", "--pretty=%s"]
            ),
            "test(iteration): complete task",
        );
    }

    struct TempDir(PathBuf);

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn unique_tmp(prefix: &str) -> TempDir {
        let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "kobito-iteration-{prefix}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        TempDir(path)
    }

    fn write_fake_codex(bin: &Path) {
        write_fake_codex_with_branch(bin, "feature/from-fake");
    }

    fn write_fake_codex_with_branch(bin: &Path, branch: &str) {
        let script = bin.join("codex");
        std::fs::write(
            &script,
            r#"#!/bin/sh
case " $* " in
  *" --json "*)
    printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"{\"task_complete\":false}"}}'
    printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":2,"cached_input_tokens":3}}'
    ;;
  *)
    printf '%s\n' '__BRANCH__'
    ;;
esac
"#
            .replace("__BRANCH__", branch),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    fn write_completing_fake_codex(bin: &Path) {
        let script = bin.join("codex");
        std::fs::write(
            &script,
            r#"#!/bin/sh
case " $* " in
  *" --json "*)
    printf 'completed\n' >> README.md
    printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"{\"task_complete\":true}"}}'
    printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":4,"output_tokens":5,"cached_input_tokens":6}}'
    ;;
  *"Suggest a single git branch name"*)
    printf '%s\n' 'feature/completed'
    ;;
  *"Generate a single commit message"*)
    printf '%s\n' 'test(iteration): complete task'
    ;;
  *)
    printf '%s\n' 'NO_NOTES'
    ;;
esac
"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    fn write_fake_gh(bin: &Path) {
        let script = bin.join("gh");
        std::fs::write(
            &script,
            r#"#!/bin/sh
case "$1 $2" in
  "pr create")
    test -f .git/kobito-test-gh-success || exit 1
    printf '%s\n' 'https://github.example/unhappychoice/kobito/pull/1'
    ;;
  *)
    exit 1
    ;;
esac
"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    fn init_repo(repo: &Path) {
        std::fs::create_dir_all(repo).unwrap();
        run_git(repo, &["init", "-b", "main"]);
        run_git(repo, &["config", "user.email", "test@example.com"]);
        run_git(repo, &["config", "user.name", "Test User"]);
        run_git(repo, &["config", "commit.gpgsign", "false"]);
        std::fs::write(repo.join("README.md"), "hello\n").unwrap();
        run_git(repo, &["add", "."]);
        run_git(repo, &["commit", "-m", "chore: initial"]);
    }

    fn init_bare_remote(remote: &Path) {
        std::fs::create_dir_all(remote).unwrap();
        let status = Command::new("git")
            .args(["init", "--bare", "-b", "main"])
            .current_dir(remote)
            .status()
            .unwrap();
        assert!(status.success(), "git init --bare failed with {status}");
    }

    fn run_git(repo: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(repo)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed with {status}");
    }

    fn git_output(repo: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn prefixed_path(bin: &Path, old_path: Option<&std::ffi::OsStr>) -> std::ffi::OsString {
        let mut paths = vec![bin.to_path_buf()];
        paths.extend(std::env::split_paths(old_path.unwrap_or_default()));
        std::env::join_paths(paths).unwrap()
    }

    fn set_env(key: &str, value: Option<&std::ffi::OsStr>) {
        unsafe {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }

    fn assert_branch_exists(repo: &Path, branch: &str) {
        let status = Command::new("git")
            .args(["rev-parse", "--verify", branch])
            .current_dir(repo)
            .status()
            .unwrap();
        assert!(status.success(), "expected branch {branch} to exist");
    }

    fn single_project_under(state_home: &Path) -> state::ProjectPaths {
        let projects = state_home.join("kobito").join("projects");
        let project_root = std::fs::read_dir(&projects)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| path.is_dir())
            .unwrap();
        let id = project_root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap()
            .to_string();
        state::ProjectPaths {
            id,
            root: project_root,
        }
    }

    fn assert_saved_prompt_mentions_task(project: &state::ProjectPaths, task: &str) {
        let runs_dir = project.root.join("runs");
        let runs = std::fs::read_dir(&runs_dir).unwrap().count();
        assert_eq!(runs, 1);
        let prompt = std::fs::read_dir(&runs_dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path().join("prompts").join("iter-0001.md"))
            .find(|path| path.exists())
            .and_then(|path| std::fs::read_to_string(path).ok())
            .unwrap();
        assert!(
            prompt.contains(task),
            "prompt should mention task: {prompt}"
        );
    }
}
