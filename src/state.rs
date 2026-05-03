use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ProjectPaths {
    pub id: String,
    pub root: PathBuf,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct RunPaths {
    pub project: ProjectPaths,
    pub run_dir: PathBuf,
    pub log_file: PathBuf,
    pub prompts_dir: PathBuf,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunMeta {
    pub run_id: String,
    pub started_at: String,
    pub branch: String,
    pub goal: String,
    pub agent: String,
    /// URL of the draft PR opened for this run, if any. Persisted so
    /// `kobito resume` can push to the same PR instead of opening a
    /// duplicate. Older meta files without this field deserialize to
    /// `None`.
    #[serde(default)]
    pub pr_url: Option<String>,
    /// Branch this run was started from (the branch we'll target as
    /// the PR base). Defaults to "main" for older meta files.
    #[serde(default)]
    pub base_branch: Option<String>,
}

pub fn state_root() -> PathBuf {
    if let Ok(custom) = std::env::var("XDG_STATE_HOME")
        && !custom.is_empty()
    {
        return PathBuf::from(custom).join("kobito");
    }
    dirs::home_dir()
        .map(|h| h.join(".local/state/kobito"))
        .unwrap_or_else(|| PathBuf::from(".kobito-state"))
}

pub fn project_id(repo_root: &Path, remote_url: Option<&str>) -> String {
    let basename = repo_root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("project");
    let identity = remote_url
        .map(|s| s.to_string())
        .unwrap_or_else(|| repo_root.to_string_lossy().to_string());
    let mut hasher = Sha1::new();
    hasher.update(identity.as_bytes());
    let digest = hasher.finalize();
    let short = digest
        .iter()
        .take(4)
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    format!("{basename}-{short}")
}

pub fn project_paths(id: String) -> Result<ProjectPaths> {
    let root = state_root().join("projects").join(&id);
    fs::create_dir_all(root.join("runs")).with_context(|| format!("create {root:?}"))?;
    Ok(ProjectPaths { id, root })
}

pub fn new_run(project: ProjectPaths) -> Result<RunPaths> {
    let timestamp = Utc::now().format("%Y-%m-%dT%H-%M-%S").to_string();
    let run_dir = project.root.join("runs").join(&timestamp);
    let prompts_dir = run_dir.join("prompts");
    fs::create_dir_all(&prompts_dir)?;
    let log_file = run_dir.join("log.ndjson");
    fs::write(&log_file, "")?;
    Ok(RunPaths {
        project,
        run_dir,
        log_file,
        prompts_dir,
        timestamp,
    })
}

pub fn write_run_meta(run: &RunPaths, meta: &RunMeta) -> Result<()> {
    let path = run.run_dir.join("meta.json");
    fs::write(path, serde_json::to_string_pretty(meta)?)?;
    Ok(())
}

pub fn read_run_meta(project: &ProjectPaths, run_id: &str) -> Result<(RunMeta, RunPaths)> {
    let run_dir = project.root.join("runs").join(run_id);
    if !run_dir.exists() {
        bail!("run `{run_id}` not found in {}", project.root.display());
    }
    let meta_path = run_dir.join("meta.json");
    let body =
        fs::read_to_string(&meta_path).with_context(|| format!("read {}", meta_path.display()))?;
    let meta: RunMeta = serde_json::from_str(&body)?;
    let run = RunPaths {
        project: project.clone(),
        run_dir: run_dir.clone(),
        log_file: run_dir.join("log.ndjson"),
        prompts_dir: run_dir.join("prompts"),
        timestamp: run_id.to_string(),
    };
    Ok((meta, run))
}

#[derive(Debug, Clone)]
pub struct RunSummary {
    pub id: String,
    pub meta: RunMeta,
}

pub fn recent_runs(project: &ProjectPaths, limit: usize) -> Result<Vec<RunSummary>> {
    let runs = project.root.join("runs");
    if !runs.exists() {
        return Ok(vec![]);
    }
    let mut summaries: Vec<RunSummary> = fs::read_dir(&runs)?
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let id = e.file_name().to_string_lossy().to_string();
            let body = fs::read_to_string(e.path().join("meta.json")).ok()?;
            let meta: RunMeta = serde_json::from_str(&body).ok()?;
            Some(RunSummary { id, meta })
        })
        .collect();
    summaries.sort_by(|a, b| b.id.cmp(&a.id));
    summaries.truncate(limit);
    Ok(summaries)
}

pub fn notes_path(run: &RunPaths) -> PathBuf {
    run.run_dir.join("notes.md")
}

pub fn tasks_path(project: &ProjectPaths) -> PathBuf {
    project.root.join("tasks.md")
}

pub fn seed_tasks_if_needed(project: &ProjectPaths, repo: &Path) -> Result<PathBuf> {
    let dest = tasks_path(project);
    if dest.exists() {
        return Ok(dest);
    }
    let src = repo.join(".kobito/tasks.md");
    let body = if src.exists() {
        fs::read_to_string(&src)?
    } else {
        String::new()
    };
    fs::write(&dest, body)?;
    Ok(dest)
}

pub fn list_projects() -> Result<()> {
    let root = state_root().join("projects");
    if !root.exists() {
        println!("(no projects yet)");
        return Ok(());
    }
    for entry in fs::read_dir(&root)? {
        let entry = entry?;
        let id = entry.file_name().to_string_lossy().to_string();
        let runs_dir = entry.path().join("runs");
        let last_run = fs::read_dir(&runs_dir)
            .ok()
            .and_then(|it| {
                it.flatten()
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .max()
            })
            .unwrap_or_else(|| "—".to_string());
        println!("{id}\tlast: {last_run}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ENV_LOCK;

    fn unique_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "kobito-state-{label}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn with_xdg_state_home<T>(root: &Path, f: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap();
        let previous = std::env::var_os("XDG_STATE_HOME");
        unsafe { std::env::set_var("XDG_STATE_HOME", root) };
        let result = f();
        match previous {
            Some(value) => unsafe { std::env::set_var("XDG_STATE_HOME", value) },
            None => unsafe { std::env::remove_var("XDG_STATE_HOME") },
        }
        result
    }

    fn project_with_root(root: PathBuf, id: &str) -> ProjectPaths {
        fs::create_dir_all(root.join("runs")).unwrap();
        ProjectPaths {
            id: id.to_string(),
            root,
        }
    }

    fn sample_meta(run_id: &str) -> RunMeta {
        RunMeta {
            run_id: run_id.to_string(),
            started_at: "2026-05-01T00:00:00Z".to_string(),
            branch: "feature/x".to_string(),
            goal: "do thing".to_string(),
            agent: "claude_code".to_string(),
            pr_url: None,
            base_branch: None,
        }
    }

    #[test]
    fn project_id_starts_with_basename_and_8_hex_suffix() {
        let id = project_id(
            Path::new("/tmp/myproj"),
            Some("https://example.com/myproj.git"),
        );
        let (basename, suffix) = id.rsplit_once('-').unwrap();
        assert_eq!(basename, "myproj");
        assert_eq!(suffix.len(), 8);
        assert!(suffix.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn project_id_is_stable_for_same_remote_url() {
        let a = project_id(Path::new("/x/proj"), Some("git@github.com:o/r.git"));
        let b = project_id(Path::new("/elsewhere/proj"), Some("git@github.com:o/r.git"));
        assert_eq!(a, b);
    }

    #[test]
    fn project_id_falls_back_to_path_when_no_remote() {
        let a = project_id(Path::new("/some/where/proj"), None);
        let b = project_id(Path::new("/other/proj"), None);
        assert!(a.starts_with("proj-"));
        assert!(b.starts_with("proj-"));
        assert_ne!(a, b);
    }

    #[test]
    fn project_id_uses_default_basename_when_path_has_none() {
        let id = project_id(Path::new("/"), Some("remote"));
        assert!(id.starts_with("project-"));
    }

    #[test]
    fn state_root_uses_non_empty_xdg_state_home() {
        let dir = unique_dir("xdg-root");
        let root = with_xdg_state_home(&dir, state_root);
        assert_eq!(root, dir.join("kobito"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn project_paths_creates_project_runs_dir_under_state_root() {
        let dir = unique_dir("project-paths");
        let project = with_xdg_state_home(&dir, || project_paths("p-9".to_string()).unwrap());
        assert_eq!(project.id, "p-9");
        assert_eq!(project.root, dir.join("kobito/projects/p-9"));
        assert!(project.root.join("runs").is_dir());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn new_run_creates_run_dir_prompts_and_empty_log() {
        let dir = unique_dir("new-run");
        let project = project_with_root(dir.clone(), "p-1");
        let run = new_run(project).unwrap();
        assert!(run.run_dir.exists());
        assert!(run.prompts_dir.exists());
        assert!(run.log_file.exists());
        assert_eq!(fs::read_to_string(&run.log_file).unwrap(), "");
        assert!(!run.timestamp.is_empty());
        assert_eq!(run.run_dir.file_name().unwrap(), run.timestamp.as_str());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_run_meta_round_trips_written_meta() {
        let dir = unique_dir("meta-rt");
        let project = project_with_root(dir.clone(), "p-2");
        let run = new_run(project.clone()).unwrap();
        write_run_meta(&run, &sample_meta(&run.timestamp)).unwrap();
        let (loaded, paths) = read_run_meta(&project, &run.timestamp).unwrap();
        assert_eq!(loaded.branch, "feature/x");
        assert_eq!(loaded.goal, "do thing");
        assert_eq!(loaded.agent, "claude_code");
        assert_eq!(paths.run_dir, run.run_dir);
        assert_eq!(paths.timestamp, run.timestamp);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_run_meta_errors_when_run_missing() {
        let dir = unique_dir("meta-missing");
        let project = project_with_root(dir.clone(), "p-3");
        let err = read_run_meta(&project, "no-such-run").unwrap_err();
        assert!(format!("{err:#}").contains("not found"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn recent_runs_returns_empty_when_runs_dir_missing() {
        let dir = unique_dir("recent-empty");
        let project = ProjectPaths {
            id: "p".to_string(),
            root: dir.clone(),
        };
        assert!(recent_runs(&project, 5).unwrap().is_empty());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn recent_runs_sorts_desc_and_truncates_to_limit() {
        let dir = unique_dir("recent-sort");
        let project = project_with_root(dir.clone(), "p-4");
        for id in [
            "2026-01-01T00-00-00",
            "2026-03-01T00-00-00",
            "2026-02-01T00-00-00",
        ] {
            let rd = project.root.join("runs").join(id);
            fs::create_dir_all(&rd).unwrap();
            fs::write(
                rd.join("meta.json"),
                serde_json::to_string(&sample_meta(id)).unwrap(),
            )
            .unwrap();
        }
        let summaries = recent_runs(&project, 2).unwrap();
        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].id, "2026-03-01T00-00-00");
        assert_eq!(summaries[1].id, "2026-02-01T00-00-00");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn recent_runs_skips_dirs_without_meta_json() {
        let dir = unique_dir("recent-skip");
        let project = project_with_root(dir.clone(), "p-5");
        fs::create_dir_all(project.root.join("runs").join("orphan")).unwrap();
        assert!(recent_runs(&project, 5).unwrap().is_empty());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn notes_path_and_tasks_path_match_layout() {
        let dir = unique_dir("paths");
        let project = ProjectPaths {
            id: "p".to_string(),
            root: dir.clone(),
        };
        let run = RunPaths {
            project: project.clone(),
            run_dir: dir.join("runs/r1"),
            log_file: dir.join("runs/r1/log.ndjson"),
            prompts_dir: dir.join("runs/r1/prompts"),
            timestamp: "r1".to_string(),
        };
        assert_eq!(notes_path(&run), dir.join("runs/r1/notes.md"));
        assert_eq!(tasks_path(&project), dir.join("tasks.md"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn seed_tasks_copies_from_repo_kobito_dir() {
        let dir = unique_dir("seed-copy");
        let project = project_with_root(dir.clone(), "p-6");
        let repo = unique_dir("seed-copy-repo");
        fs::create_dir_all(repo.join(".kobito")).unwrap();
        fs::write(repo.join(".kobito/tasks.md"), "- [ ] do x\n").unwrap();
        let dest = seed_tasks_if_needed(&project, &repo).unwrap();
        assert_eq!(dest, project.root.join("tasks.md"));
        assert_eq!(fs::read_to_string(&dest).unwrap(), "- [ ] do x\n");
        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&repo).ok();
    }

    #[test]
    fn seed_tasks_creates_empty_file_when_repo_has_none() {
        let dir = unique_dir("seed-empty");
        let project = project_with_root(dir.clone(), "p-7");
        let repo = unique_dir("seed-empty-repo");
        let dest = seed_tasks_if_needed(&project, &repo).unwrap();
        assert!(dest.exists());
        assert_eq!(fs::read_to_string(&dest).unwrap(), "");
        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&repo).ok();
    }

    #[test]
    fn seed_tasks_is_idempotent_when_dest_already_present() {
        let dir = unique_dir("seed-idem");
        let project = project_with_root(dir.clone(), "p-8");
        fs::write(project.root.join("tasks.md"), "existing\n").unwrap();
        let repo = unique_dir("seed-idem-repo");
        fs::create_dir_all(repo.join(".kobito")).unwrap();
        fs::write(repo.join(".kobito/tasks.md"), "should NOT overwrite\n").unwrap();
        let dest = seed_tasks_if_needed(&project, &repo).unwrap();
        assert_eq!(fs::read_to_string(&dest).unwrap(), "existing\n");
        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&repo).ok();
    }

    #[test]
    fn list_projects_accepts_empty_state_root() {
        let dir = unique_dir("list-empty");
        let result = with_xdg_state_home(&dir, list_projects);
        assert!(result.is_ok());
        fs::remove_dir_all(&dir).ok();
    }
}
