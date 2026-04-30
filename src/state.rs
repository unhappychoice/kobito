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
}

pub fn state_root() -> PathBuf {
    if let Ok(custom) = std::env::var("XDG_STATE_HOME") {
        if !custom.is_empty() {
            return PathBuf::from(custom).join("kobito");
        }
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
    let short = digest.iter().take(4).map(|b| format!("{b:02x}")).collect::<String>();
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
    let body = fs::read_to_string(&meta_path)
        .with_context(|| format!("read {}", meta_path.display()))?;
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

pub fn latest_run_id(project: &ProjectPaths) -> Result<Option<String>> {
    let runs = project.root.join("runs");
    if !runs.exists() {
        return Ok(None);
    }
    let mut ids: Vec<String> = fs::read_dir(&runs)?
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    ids.sort();
    Ok(ids.into_iter().last())
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
