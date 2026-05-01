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

    fn git(repo: &Path, args: &[&str]) {
        let out = Command::new("git")
            .current_dir(repo)
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

    fn init_repo_with_origin(label: &str) -> (TempRepo, TempRepo) {
        let bare = unique_dir(&format!("{label}-bare"));
        git(&bare, &["init", "-q", "--bare", "-b", "main"]);
        let work = unique_dir(&format!("{label}-work"));
        git(&work, &["init", "-q", "-b", "main"]);
        git(&work, &["config", "user.email", "test@example.com"]);
        git(&work, &["config", "user.name", "Test"]);
        git(&work, &["config", "commit.gpgsign", "false"]);
        git(&work, &["remote", "add", "origin", bare.to_str().unwrap()]);
        git(&work, &["commit", "--allow-empty", "-q", "-m", "init"]);
        (TempRepo(bare), TempRepo(work))
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
    fn edit_is_a_noop_when_title_and_body_are_both_none() {
        let dir = unique_dir("edit-noop");
        edit(&dir, "https://example.com/pr/1", None, None).unwrap();
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn push_succeeds_against_local_bare_remote() {
        let (_bare, work) = init_repo_with_origin("push-ok");
        push(&work, "main", true).unwrap();
    }

    #[test]
    fn push_fails_when_repo_has_no_origin() {
        let dir = unique_dir("push-no-origin");
        git(&dir, &["init", "-q", "-b", "main"]);
        git(&dir, &["config", "user.email", "test@example.com"]);
        git(&dir, &["config", "user.name", "Test"]);
        git(&dir, &["config", "commit.gpgsign", "false"]);
        git(&dir, &["commit", "--allow-empty", "-q", "-m", "init"]);
        let err = push(&dir, "main", false).unwrap_err().to_string();
        assert!(err.contains("git push failed"), "got: {err}");
        fs::remove_dir_all(&dir).ok();
    }
}
