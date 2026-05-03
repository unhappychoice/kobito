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
    #[command(alias = "continuous")]
    Cont(ContinuousArgs),
    /// Consume a tasks.md backlog, one branch + PR per task.
    #[command(alias = "iteration")]
    Iter(IterationArgs),
    /// List all known projects with their last run.
    Ls,
    /// Replay the last run's log for a project.
    Log { project: Option<String> },
    /// Resume a previous cont run (interactive picker, or --run <id>).
    Resume(ResumeArgs),
    /// Manage the iter backlog.
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
pub struct ResumeArgs {
    /// Run id to resume (timestamp directory name). If omitted, an
    /// interactive picker shows the 10 most recent runs (auto-picks the
    /// only one when there's just one, or in non-interactive shells).
    #[arg(long)]
    pub run: Option<String>,

    /// Maximum iterations before exiting.
    #[arg(long, default_value_t = 50)]
    pub max_iterations: u32,

    /// Maximum consecutive failures before giving up.
    #[arg(long, default_value_t = 3)]
    pub max_failures: u32,

    /// Skip the clean-tree check (dangerous).
    #[arg(long)]
    pub allow_dirty: bool,
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
        Command::Cont(args) => runner::run_continuous(args).await,
        Command::Iter(args) => iteration::run(args).await,
        Command::Resume(args) => runner::resume_continuous(args).await,
        Command::Ls => crate::state::list_projects(),
        Command::Tasks {
            action: TasksAction::Edit,
        } => edit_tasks(),
        Command::Log { .. } => bail!("not yet implemented"),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cont_accepts_prompt_and_defaults() {
        let cli = Cli::parse_from(["kobito", "cont", "--prompt", "ship it"]);
        let Command::Cont(args) = cli.command else {
            panic!("expected cont command");
        };

        assert_eq!(args.prompt.as_deref(), Some("ship it"));
        assert_eq!(args.preset, None);
        assert_eq!(args.max_iterations, 50);
        assert_eq!(args.max_failures, 3);
        assert_eq!(args.agent, "claude");
        assert!(!args.allow_dirty);
    }

    #[test]
    fn continuous_alias_accepts_preset_vars_and_options() {
        let cli = Cli::parse_from([
            "kobito",
            "continuous",
            "--preset",
            "coverage",
            "--var",
            "path=src",
            "--max-iterations",
            "7",
            "--max-failures",
            "2",
            "--agent",
            "codex",
            "--allow-dirty",
        ]);
        let Command::Cont(args) = cli.command else {
            panic!("expected cont command");
        };

        assert_eq!(args.preset.as_deref(), Some("coverage"));
        assert_eq!(args.vars, vec!["path=src"]);
        assert_eq!(args.max_iterations, 7);
        assert_eq!(args.max_failures, 2);
        assert_eq!(args.agent, "codex");
        assert!(args.allow_dirty);
    }

    #[test]
    fn cont_requires_exactly_one_goal_source() {
        assert!(Cli::try_parse_from(["kobito", "cont"]).is_err());
        assert!(Cli::try_parse_from(["kobito", "cont", "--prompt", "x", "--preset", "y"]).is_err());
    }

    #[test]
    fn iteration_alias_parses_backlog_preset_and_vars() {
        let cli = Cli::parse_from([
            "kobito",
            "iteration",
            "--backlog",
            "tasks.md",
            "--preset",
            "repo",
            "--var",
            "area=runner",
            "--agent",
            "codex",
            "--allow-dirty",
        ]);
        let Command::Iter(args) = cli.command else {
            panic!("expected iter command");
        };

        assert_eq!(args.backlog, Some(PathBuf::from("tasks.md")));
        assert_eq!(args.preset.as_deref(), Some("repo"));
        assert_eq!(args.vars, vec!["area=runner"]);
        assert_eq!(args.max_iterations, 30);
        assert_eq!(args.max_failures, 3);
        assert_eq!(args.agent, "codex");
        assert!(args.allow_dirty);
    }

    #[test]
    fn resume_parses_optional_run_and_limits() {
        let cli = Cli::parse_from([
            "kobito",
            "resume",
            "--run",
            "20260503-120000",
            "--max-iterations",
            "4",
            "--max-failures",
            "1",
            "--allow-dirty",
        ]);
        let Command::Resume(args) = cli.command else {
            panic!("expected resume command");
        };

        assert_eq!(args.run.as_deref(), Some("20260503-120000"));
        assert_eq!(args.max_iterations, 4);
        assert_eq!(args.max_failures, 1);
        assert!(args.allow_dirty);
    }

    #[test]
    fn parses_leaf_commands() {
        assert!(matches!(
            Cli::parse_from(["kobito", "ls"]).command,
            Command::Ls
        ));
        assert!(matches!(
            Cli::parse_from(["kobito", "log", "project-id"]).command,
            Command::Log { project: Some(_) }
        ));
        assert!(matches!(
            Cli::parse_from(["kobito", "tasks", "edit"]).command,
            Command::Tasks {
                action: TasksAction::Edit
            }
        ));
    }
}
