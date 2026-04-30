use anyhow::{Result, bail};
use clap::{Parser, Subcommand};

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
pub struct ContinuousArgs {
    /// The goal to pursue, e.g. "Increase test coverage in src/".
    #[arg(short, long)]
    pub prompt: String,

    /// Maximum number of iterations before exiting.
    #[arg(long, default_value_t = 50)]
    pub max_iterations: u32,

    /// Maximum number of consecutive failures before giving up.
    #[arg(long, default_value_t = 3)]
    pub max_failures: u32,

    /// Output language for code, comments, commit messages.
    #[arg(long)]
    pub language: Option<String>,

    /// Agent backend (currently only `claude`).
    #[arg(long, default_value = "claude")]
    pub agent: String,

    /// Skip the clean-tree check (dangerous).
    #[arg(long)]
    pub allow_dirty: bool,
}

pub async fn dispatch(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Continuous(_)
        | Command::Ls
        | Command::Log { .. }
        | Command::Resume { .. }
        | Command::Tasks { .. } => bail!("not yet implemented"),
    }
}
