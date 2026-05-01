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
    if title.is_none() && body.is_none() {
        return Ok(());
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

    fn unique_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "kobito-pr-{label}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn run_git_in(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn init_repo(label: &str) -> TempDir {
        let dir = unique_dir(label);
        run_git_in(&dir, &["init", "-q", "-b", "main"]);
        run_git_in(&dir, &["config", "user.email", "test@example.com"]);
        run_git_in(&dir, &["config", "user.name", "Test"]);
        run_git_in(&dir, &["config", "commit.gpgsign", "false"]);
        TempDir(dir)
    }

    fn init_bare(label: &str) -> TempDir {
        let dir = unique_dir(label);
        run_git_in(&dir, &["init", "--bare", "-q", "-b", "main"]);
        TempDir(dir)
    }

    fn empty_commit(repo: &Path, msg: &str) {
        run_git_in(repo, &["commit", "--allow-empty", "-m", msg]);
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
    fn edit_is_a_noop_when_both_title_and_body_are_none() {
        let dir = unique_dir("edit-noop");
        let guard = TempDir(dir);
        edit(&guard, "https://example.com/owner/repo/pull/1", None, None).unwrap();
    }

    #[test]
    fn push_to_local_remote_succeeds_and_creates_ref() {
        let bare = init_bare("push-bare");
        let work = init_repo("push-work");
        empty_commit(&work, "init");
        let remote_url = bare.to_string_lossy().to_string();
        run_git_in(&work, &["remote", "add", "origin", &remote_url]);
        push(&work, "main", true).unwrap();
        let out = Command::new("git")
            .current_dir(&*bare)
            .args(["rev-parse", "main"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "expected `main` ref in bare repo: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn push_without_upstream_succeeds_after_initial_push() {
        let bare = init_bare("push-bare2");
        let work = init_repo("push-work2");
        empty_commit(&work, "init");
        let remote_url = bare.to_string_lossy().to_string();
        run_git_in(&work, &["remote", "add", "origin", &remote_url]);
        push(&work, "main", true).unwrap();
        empty_commit(&work, "next");
        push(&work, "main", false).unwrap();
    }

    #[test]
    fn push_returns_error_when_origin_is_not_configured() {
        let work = init_repo("push-noremote");
        empty_commit(&work, "init");
        let err = push(&work, "main", false).unwrap_err().to_string();
        assert!(
            err.contains("git push"),
            "expected git push error, got: {err}"
        );
    }
}
