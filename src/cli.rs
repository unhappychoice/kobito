use anyhow::{Result, bail};
use clap::{ArgGroup, Parser, Subcommand};
use std::path::PathBuf;

use crate::{iteration, runner};

#[derive(Parser, Debug)]
#[command(
    name = "kobito",
    version,
    about = "Autonomous coding agent orchestrator — works while you sleep"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Run a never-ending goal on a single branch with many commits.
    Continuous(ContinuousArgs),
    /// Consume a tasks.md backlog, one branch + PR per task.
    Iteration(IterationArgs),
    /// List all known projects with their last run.
    Ls,
    /// Replay the last run's log for a project.
    Log { project: Option<String> },
    /// Resume iteration mode from where it stopped.
    Resume { project: Option<String> },
    /// Manage the iteration backlog.
    Tasks {
        #[command(subcommand)]
        action: TasksAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum TasksAction {
    /// Open $EDITOR on the state copy of tasks.md.
    Edit,
}

#[derive(Parser, Debug)]
pub struct IterationArgs {
    /// Path to a tasks.md backlog. If omitted, uses the state copy
    /// (seeded from .kobito/tasks.md on first run).
    #[arg(long)]
    pub backlog: Option<PathBuf>,

    /// Compose a preset above each task prompt. Looks up
    /// .kobito/presets/<name>.md (project) then $XDG_CONFIG_HOME/kobito/presets/<name>.md.
    #[arg(long)]
    pub preset: Option<String>,

    /// Variable for the preset, repeatable: --var path=src --var target=80.
    #[arg(long = "var")]
    pub vars: Vec<String>,

    /// Maximum iterations per task before giving up.
    #[arg(long, default_value_t = 30)]
    pub max_iterations: u32,

    /// Maximum consecutive failures per task before skipping.
    #[arg(long, default_value_t = 3)]
    pub max_failures: u32,

    /// Agent backend: `claude` (alias `claude-code`) or `codex`.
    #[arg(long, default_value = "claude")]
    pub agent: String,

    /// Skip the clean-tree check (dangerous).
    #[arg(long)]
    pub allow_dirty: bool,
}

#[derive(Parser, Debug)]
#[command(group = ArgGroup::new("goal").required(true).multiple(false).args(["prompt", "preset"]))]
pub struct ContinuousArgs {
    /// The goal to pursue, e.g. "Increase test coverage in src/".
    /// Mutually exclusive with --preset.
    #[arg(short, long)]
    pub prompt: Option<String>,

    /// Use a preset (with --var substitution) as the goal.
    /// Looks up .kobito/presets/<name>.md (project) then
    /// $XDG_CONFIG_HOME/kobito/presets/<name>.md. Mutually exclusive with --prompt.
    #[arg(long)]
    pub preset: Option<String>,

    /// Variable for the preset, repeatable: --var path=src --var target=80.
    #[arg(long = "var")]
    pub vars: Vec<String>,

    /// Maximum number of iterations before exiting.
    #[arg(long, default_value_t = 50)]
    pub max_iterations: u32,

    /// Maximum number of consecutive failures before giving up.
    #[arg(long, default_value_t = 3)]
    pub max_failures: u32,

    /// Agent backend: `claude` (alias `claude-code`) or `codex`.
    #[arg(long, default_value = "claude")]
    pub agent: String,

    /// Skip the clean-tree check (dangerous).
    #[arg(long)]
    pub allow_dirty: bool,
}

pub async fn dispatch(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Continuous(args) => runner::run_continuous(args).await,
        Command::Iteration(args) => iteration::run(args).await,
        Command::Ls => crate::state::list_projects(),
        Command::Tasks {
            action: TasksAction::Edit,
        } => edit_tasks(),
        Command::Log { .. } | Command::Resume { .. } => {
            bail!("not yet implemented — tracked in #6")
        }
    }
}

fn edit_tasks() -> Result<()> {
    let repo = crate::git::repo_root()?;
    let remote = crate::git::remote_url(&repo);
    let id = crate::state::project_id(&repo, remote.as_deref());
    let project = crate::state::project_paths(id)?;
    let path = crate::state::seed_tasks_if_needed(&project, &repo)?;
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "vi".into());
    let status = std::process::Command::new(editor).arg(&path).status()?;
    if !status.success() {
        bail!("editor exited with {status}");
    }
    println!("{}", path.display());
    Ok(())
}
