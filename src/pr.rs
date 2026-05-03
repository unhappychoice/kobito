use anyhow::{Result, bail};
use std::path::Path;
use std::process::Command;

pub fn push(repo: &Path, branch: &str, set_upstream: bool) -> Result<()> {
    let args = push_args(branch, set_upstream);
    run_git(repo, &args)?;
    Ok(())
}

pub fn create(
    repo: &Path,
    base: &str,
    branch: &str,
    title: &str,
    body: &str,
    draft: bool,
) -> Result<String> {
    let args = create_args(base, branch, title, body, draft);
    let out = run_gh(repo, &args).map_err(|e| {
        anyhow::anyhow!("gh pr create failed — is the `gh` CLI installed and authenticated? {e}")
    })?;
    Ok(out)
}

/// Edit a PR's title and/or body via `gh pr edit`. Each field is optional
/// so callers can update one without clobbering the other.
pub fn edit(repo: &Path, url: &str, title: Option<&str>, body: Option<&str>) -> Result<()> {
    let Some(args) = edit_args(url, title, body) else {
        return Ok(());
    };
    run_gh(repo, &args)?;
    Ok(())
}

pub fn mark_ready(repo: &Path, url: &str) -> Result<()> {
    run_gh(repo, &["pr", "ready", url])?;
    Ok(())
}

fn push_args(branch: &str, set_upstream: bool) -> Vec<String> {
    let mut args = vec!["push".to_string()];
    if set_upstream {
        args.push("-u".to_string());
    }
    args.push("origin".to_string());
    args.push(branch.to_string());
    args
}

fn create_args(base: &str, branch: &str, title: &str, body: &str, draft: bool) -> Vec<String> {
    let mut args = vec![
        "pr".to_string(),
        "create".to_string(),
        "--base".to_string(),
        base.to_string(),
        "--head".to_string(),
        branch.to_string(),
        "--title".to_string(),
        title.to_string(),
        "--body".to_string(),
        body.to_string(),
    ];
    if draft {
        args.push("--draft".to_string());
    }
    args
}

fn edit_args(url: &str, title: Option<&str>, body: Option<&str>) -> Option<Vec<String>> {
    if title.is_none() && body.is_none() {
        return None;
    }
    let mut args = vec!["pr".to_string(), "edit".to_string(), url.to_string()];
    if let Some(t) = title {
        args.push("--title".to_string());
        args.push(t.to_string());
    }
    if let Some(b) = body {
        args.push("--body".to_string());
        args.push(b.to_string());
    }
    Some(args)
}

fn run_git<S: AsRef<str>>(repo: &Path, args: &[S]) -> Result<String> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(args.iter().map(|s| s.as_ref()))
        .output()?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args[0].as_ref(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn run_gh<S: AsRef<str>>(repo: &Path, args: &[S]) -> Result<String> {
    let out = Command::new("gh")
        .current_dir(repo)
        .args(args.iter().map(|s| s.as_ref()))
        .output()?;
    if !out.status.success() {
        bail!(
            "gh {} failed: {}",
            args[0].as_ref(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

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

    fn unique_dir(label: &str) -> TempDir {
        let dir = std::env::temp_dir().join(format!(
            "kobito-pr-{label}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
        ));
        fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    fn git(repo: &Path, args: &[&str]) {
        let out = Command::new("git")
            .current_dir(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr),
        );
    }

    fn repo_with_remote(label: &str) -> (TempDir, TempDir) {
        let repo = unique_dir(&format!("{label}-repo"));
        let remote = unique_dir(&format!("{label}-remote"));
        git(&remote, &["init", "--bare", "-q"]);
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "Test"]);
        git(&repo, &["config", "commit.gpgsign", "false"]);
        git(&repo, &["commit", "--allow-empty", "-m", "init"]);
        git(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        (repo, remote)
    }

    #[test]
    fn push_args_with_upstream_includes_dash_u() {
        let args = push_args("feature/x", true);
        assert_eq!(args, vec!["push", "-u", "origin", "feature/x"]);
    }

    #[test]
    fn push_args_without_upstream_omits_dash_u() {
        let args = push_args("feature/x", false);
        assert_eq!(args, vec!["push", "origin", "feature/x"]);
    }

    #[test]
    fn create_args_draft_appends_flag() {
        let args = create_args("main", "topic", "title", "body", true);
        assert!(args.contains(&"--draft".to_string()));
        assert!(args.contains(&"--base".to_string()));
        assert!(args.contains(&"main".to_string()));
        assert!(args.contains(&"--head".to_string()));
        assert!(args.contains(&"topic".to_string()));
    }

    #[test]
    fn create_args_non_draft_omits_flag() {
        let args = create_args("main", "topic", "title", "body", false);
        assert!(!args.contains(&"--draft".to_string()));
    }

    #[test]
    fn create_args_preserves_title_and_body_verbatim() {
        let args = create_args("main", "topic", "the title", "multi\nline\nbody", false);
        let title_idx = args.iter().position(|a| a == "--title").unwrap();
        assert_eq!(args[title_idx + 1], "the title");
        let body_idx = args.iter().position(|a| a == "--body").unwrap();
        assert_eq!(args[body_idx + 1], "multi\nline\nbody");
    }

    #[test]
    fn edit_args_returns_none_when_no_fields_are_set() {
        assert_eq!(edit_args("https://github.com/o/r/pull/1", None, None), None);
    }

    #[test]
    fn edit_args_with_title_only_omits_body() {
        let args = edit_args("https://github.com/o/r/pull/1", Some("new title"), None).unwrap();
        assert_eq!(
            args,
            vec![
                "pr",
                "edit",
                "https://github.com/o/r/pull/1",
                "--title",
                "new title",
            ],
        );
    }

    #[test]
    fn edit_args_with_body_only_omits_title() {
        let args = edit_args("https://github.com/o/r/pull/1", None, Some("body")).unwrap();
        assert_eq!(
            args,
            vec![
                "pr",
                "edit",
                "https://github.com/o/r/pull/1",
                "--body",
                "body"
            ],
        );
    }

    #[test]
    fn edit_args_with_title_and_body_preserves_order() {
        let args = edit_args("https://github.com/o/r/pull/1", Some("title"), Some("body")).unwrap();
        assert_eq!(
            args,
            vec![
                "pr",
                "edit",
                "https://github.com/o/r/pull/1",
                "--title",
                "title",
                "--body",
                "body",
            ],
        );
    }

    #[test]
    fn push_pushes_branch_to_origin_and_sets_upstream() {
        let (repo, _remote) = repo_with_remote("push-ok");
        push(&repo, "main", true).unwrap();

        let out = Command::new("git")
            .current_dir(&*repo)
            .args(["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), "origin/main");
    }

    #[test]
    fn push_reports_git_stderr_on_failure() {
        let repo = unique_dir("push-fail");
        git(&repo, &["init", "-q", "-b", "main"]);

        let err = push(&repo, "main", false).unwrap_err().to_string();
        assert!(
            err.contains("git push failed") && err.contains("origin"),
            "expected git push failure with stderr, got: {err}",
        );
    }

    #[test]
    fn edit_returns_ok_without_running_gh_when_nothing_changes() {
        let missing = unique_dir("edit-noop");
        fs::remove_dir_all(&*missing).unwrap();
        edit(&missing, "https://github.com/o/r/pull/1", None, None).unwrap();
    }

    #[test]
    fn create_wraps_gh_invocation_errors_with_actionable_context() {
        let missing = unique_dir("create-missing");
        fs::remove_dir_all(&*missing).unwrap();

        let err = create(&missing, "main", "topic", "title", "body", true)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("gh pr create failed") && err.contains("installed and authenticated"),
            "expected actionable gh create error, got: {err}",
        );
    }

    #[test]
    fn mark_ready_surfaces_gh_invocation_errors() {
        let missing = unique_dir("ready-missing");
        fs::remove_dir_all(&*missing).unwrap();

        let err = mark_ready(&missing, "https://github.com/o/r/pull/1")
            .unwrap_err()
            .to_string();
        assert!(!err.is_empty(), "expected non-empty gh invocation error",);
    }
}
