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
