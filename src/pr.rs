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

pub fn edit_body(repo: &Path, url: &str, body: &str) -> Result<()> {
    run_gh(repo, &["pr", "edit", url, "--body", body])?;
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

    struct TempRepo(PathBuf);
    impl std::ops::Deref for TempRepo {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempRepo {
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

    fn init_repo(label: &str) -> TempRepo {
        let dir = unique_dir(label);
        let cfg = |args: &[&str]| {
            std::process::Command::new("git")
                .current_dir(&dir)
                .args(args)
                .output()
                .unwrap();
        };
        cfg(&["init", "-q", "-b", "main"]);
        cfg(&["config", "user.email", "test@example.com"]);
        cfg(&["config", "user.name", "Test"]);
        cfg(&["config", "commit.gpgsign", "false"]);
        TempRepo(dir)
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
    fn push_bails_when_remote_is_missing() {
        let repo = init_repo("push-no-remote");
        let err = push(&repo, "main", false).unwrap_err().to_string();
        assert!(
            err.contains("git push failed"),
            "expected git push failure, got: {err}"
        );
    }

    #[test]
    fn push_with_upstream_also_bails_when_remote_is_missing() {
        let repo = init_repo("push-upstream-no-remote");
        let err = push(&repo, "main", true).unwrap_err().to_string();
        assert!(
            err.contains("git push failed"),
            "expected git push failure, got: {err}"
        );
    }

    #[test]
    fn create_wraps_gh_failure_with_install_hint() {
        let repo = init_repo("create-no-gh-context");
        let err = create(&repo, "main", "topic", "title", "body", true)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("gh pr create failed"),
            "expected wrapped gh failure, got: {err}"
        );
        assert!(
            err.contains("gh` CLI"),
            "expected install/auth hint, got: {err}"
        );
    }

    #[test]
    fn edit_body_bails_when_gh_fails() {
        let repo = init_repo("edit-body-fail");
        let err = edit_body(&repo, "https://example.invalid/owner/repo/pull/1", "x")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("gh pr failed"),
            "expected gh pr failure, got: {err}"
        );
    }

    #[test]
    fn mark_ready_bails_when_gh_fails() {
        let repo = init_repo("ready-fail");
        let err = mark_ready(&repo, "https://example.invalid/owner/repo/pull/1")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("gh pr failed"),
            "expected gh pr failure, got: {err}"
        );
    }
}
