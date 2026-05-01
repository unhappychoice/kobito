use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn repo_root() -> Result<PathBuf> {
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("invoke git rev-parse")?;
    if !out.status.success() {
        bail!("not inside a git repository");
    }
    let path = String::from_utf8(out.stdout)?.trim().to_string();
    Ok(PathBuf::from(path))
}

pub fn remote_url(repo: &Path) -> Option<String> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

pub fn ensure_clean(repo: &Path) -> Result<()> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["status", "--porcelain"])
        .output()?;
    if !out.status.success() {
        bail!("git status failed");
    }
    if !out.stdout.is_empty() {
        bail!("working tree is dirty — commit or stash before running kobito");
    }
    Ok(())
}

#[allow(dead_code)]
pub fn current_branch(repo: &Path) -> Result<String> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()?;
    Ok(String::from_utf8(out.stdout)?.trim().to_string())
}

/// Resolve the repository's default branch on `origin` (the target a PR
/// should be opened against). Falls back to `main` when origin/HEAD is
/// not configured locally — `current_branch` is deliberately *not*
/// used as a fallback because kobito itself may be running from a
/// previously-generated kobito branch, which is never the right PR base.
pub fn default_remote_branch(repo: &Path) -> String {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["symbolic-ref", "refs/remotes/origin/HEAD"])
        .output();
    if let Ok(out) = out
        && out.status.success()
        && let Ok(s) = String::from_utf8(out.stdout)
        && let Some(name) = s.trim().strip_prefix("refs/remotes/origin/")
    {
        return name.to_string();
    }
    "main".to_string()
}

pub fn create_and_checkout(repo: &Path, branch: &str) -> Result<()> {
    run(repo, &["checkout", "-b", branch])
}

pub fn checkout(repo: &Path, branch: &str) -> Result<()> {
    run(repo, &["checkout", branch])
}

pub fn stage_all(repo: &Path) -> Result<()> {
    run(repo, &["add", "-A"])
}

pub fn has_staged_changes(repo: &Path) -> Result<bool> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["diff", "--cached", "--quiet"])
        .output()?;
    // exit 1 = changes; 0 = none
    Ok(!out.status.success())
}

pub fn diff_staged(repo: &Path) -> Result<String> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["diff", "--cached"])
        .output()?;
    Ok(String::from_utf8(out.stdout)?)
}

pub fn diff_against(repo: &Path, base: &str) -> Result<String> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["diff", &format!("{base}...HEAD")])
        .output()?;
    if !out.status.success() {
        bail!(
            "git diff {base}...HEAD failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8(out.stdout)?)
}

pub fn commit(repo: &Path, message: &str) -> Result<()> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["commit", "-m", message])
        .output()?;
    if !out.status.success() {
        bail!(
            "git commit failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

pub fn reset_hard(repo: &Path) -> Result<()> {
    run(repo, &["reset", "--hard", "HEAD"])
}

pub fn recent_commit_messages(repo: &Path, n: usize) -> Result<Vec<String>> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["log", "--pretty=%s", &format!("-{n}")])
        .output()?;
    let s = String::from_utf8(out.stdout)?;
    Ok(s.lines().map(|l| l.to_string()).collect())
}

fn run(repo: &Path, args: &[&str]) -> Result<()> {
    let out = Command::new("git").current_dir(repo).args(args).output()?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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
            "kobito-git-{label}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn init_repo(label: &str) -> TempRepo {
        let dir = unique_dir(label);
        run(&dir, &["init", "-q", "-b", "main"]).unwrap();
        run(&dir, &["config", "user.email", "test@example.com"]).unwrap();
        run(&dir, &["config", "user.name", "Test"]).unwrap();
        run(&dir, &["config", "commit.gpgsign", "false"]).unwrap();
        TempRepo(dir)
    }

    fn write_file(repo: &Path, name: &str, content: &str) {
        fs::write(repo.join(name), content).unwrap();
    }

    fn empty_commit(repo: &Path, message: &str) {
        run(repo, &["commit", "--allow-empty", "-m", message]).unwrap();
    }

    #[test]
    fn ensure_clean_passes_for_a_clean_tree() {
        let repo = init_repo("clean");
        empty_commit(&repo, "init");
        ensure_clean(&repo).unwrap();
    }

    #[test]
    fn ensure_clean_fails_when_tree_is_dirty() {
        let repo = init_repo("dirty");
        empty_commit(&repo, "init");
        write_file(&repo, "untracked.txt", "x");
        let err = ensure_clean(&repo).unwrap_err().to_string();
        assert!(err.contains("dirty"), "expected dirty error, got: {err}");
    }

    #[test]
    fn current_branch_reports_active_branch() {
        let repo = init_repo("curbranch");
        empty_commit(&repo, "init");
        assert_eq!(current_branch(&repo).unwrap(), "main");
    }

    #[test]
    fn create_and_checkout_then_checkout_switches_branches() {
        let repo = init_repo("switch");
        empty_commit(&repo, "init");
        create_and_checkout(&repo, "feature/x").unwrap();
        assert_eq!(current_branch(&repo).unwrap(), "feature/x");
        checkout(&repo, "main").unwrap();
        assert_eq!(current_branch(&repo).unwrap(), "main");
    }

    #[test]
    fn stage_all_and_has_staged_changes_track_index_state() {
        let repo = init_repo("stage");
        empty_commit(&repo, "init");
        assert!(!has_staged_changes(&repo).unwrap());
        write_file(&repo, "a.txt", "hello\n");
        assert!(!has_staged_changes(&repo).unwrap());
        stage_all(&repo).unwrap();
        assert!(has_staged_changes(&repo).unwrap());
    }

    #[test]
    fn diff_staged_returns_diff_text_for_staged_files() {
        let repo = init_repo("diff");
        empty_commit(&repo, "init");
        write_file(&repo, "a.txt", "hello\n");
        stage_all(&repo).unwrap();
        let diff = diff_staged(&repo).unwrap();
        assert!(
            diff.contains("+hello"),
            "expected diff to include +hello, got: {diff}"
        );
    }

    #[test]
    fn commit_creates_revision_with_given_message() {
        let repo = init_repo("commit");
        empty_commit(&repo, "init");
        write_file(&repo, "a.txt", "hi\n");
        stage_all(&repo).unwrap();
        commit(&repo, "feat: add a").unwrap();
        let msgs = recent_commit_messages(&repo, 5).unwrap();
        assert_eq!(msgs[0], "feat: add a");
    }

    #[test]
    fn commit_fails_when_nothing_is_staged() {
        let repo = init_repo("commit-empty");
        empty_commit(&repo, "init");
        let err = commit(&repo, "noop").unwrap_err().to_string();
        assert!(err.contains("git commit failed"), "got: {err}");
    }

    #[test]
    fn recent_commit_messages_returns_subjects_in_order() {
        let repo = init_repo("log");
        empty_commit(&repo, "first");
        empty_commit(&repo, "second");
        empty_commit(&repo, "third");
        let msgs = recent_commit_messages(&repo, 2).unwrap();
        assert_eq!(msgs, vec!["third".to_string(), "second".to_string()]);
    }

    #[test]
    fn reset_hard_drops_unstaged_modifications() {
        let repo = init_repo("reset");
        write_file(&repo, "a.txt", "v1\n");
        stage_all(&repo).unwrap();
        commit(&repo, "init a").unwrap();
        write_file(&repo, "a.txt", "v2\n");
        reset_hard(&repo).unwrap();
        assert_eq!(fs::read_to_string(repo.join("a.txt")).unwrap(), "v1\n");
    }

    #[test]
    fn remote_url_is_none_when_no_remote_configured() {
        let repo = init_repo("noremote");
        assert!(remote_url(&repo).is_none());
    }

    #[test]
    fn remote_url_returns_configured_origin() {
        let repo = init_repo("remote");
        run(
            &repo,
            &["remote", "add", "origin", "https://example.com/x.git"],
        )
        .unwrap();
        assert_eq!(
            remote_url(&repo),
            Some("https://example.com/x.git".to_string())
        );
    }

    #[test]
    fn run_returns_error_with_stderr_when_git_fails() {
        let repo = init_repo("run-fail");
        let err = run(&repo, &["checkout", "does-not-exist"])
            .unwrap_err()
            .to_string();
        assert!(err.contains("git checkout"), "got: {err}");
    }
}
