mod api;
mod daemon;
mod duration;
mod inspect;
mod lifecycle;
mod output;
mod paths;
mod query;
mod rpc;
mod service;
mod store;
mod validate;
mod workflow;

use clap::{Parser, Subcommand};
use output::{emit, CliResult, Mode, Report};

/// Tome: agentic workflow management.
#[derive(Parser)]
#[command(name = "tome", version, about)]
struct Cli {
    /// Emit stable machine-readable JSON (also enabled by TOME_OUTPUT=json).
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Control the tome daemon.
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
    /// Validate workflows (all visible ones, or one by name or path).
    Validate {
        /// Workflow name or path to a workflow file.
        workflow: Option<String>,
        /// Check a parameter value (key=value); repeatable.
        #[arg(long = "param", value_name = "KEY=VALUE")]
        params: Vec<String>,
    },
    /// Inspect current and past runs.
    Runs {
        #[command(subcommand)]
        command: RunsCommand,
    },
    /// Run a read-only SQL query against the run store.
    Query {
        /// A single read-only statement (SELECT, WITH, DESCRIBE, ...).
        sql: String,
    },
}

#[derive(Subcommand)]
enum RunsCommand {
    /// List runs, newest first.
    List {
        /// Only runs with this status (queued, running, succeeded, failed, cancelled).
        #[arg(long)]
        status: Option<String>,
        /// Only runs of this workflow.
        #[arg(long)]
        workflow: Option<String>,
        /// Maximum number of runs to show.
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Show a run: status, steps, history, worktrees and logs.
    Show {
        /// Run id.
        id: String,
        /// Include the workflow snapshot the run was started with.
        #[arg(long)]
        snapshot: bool,
    },
    /// Print a run's step logs.
    Logs {
        /// Run id.
        id: String,
        /// Only this step's log.
        #[arg(long)]
        step: Option<String>,
        /// Only the last N lines of each log.
        #[arg(long, value_name = "N")]
        tail: Option<usize>,
    },
}

#[derive(Subcommand)]
enum DaemonCommand {
    /// Start the daemon in the background.
    Start,
    /// Stop the running daemon.
    Stop,
    /// Show whether the daemon is running (exit code 3 if not).
    Status,
    /// Run the daemon in the foreground (used by service units).
    Run,
    /// Register the daemon as a login service (launchd on macOS, systemd --user on Linux).
    Install {
        /// Print the unit file instead of installing it.
        #[arg(long)]
        print: bool,
        /// Write and register the unit without starting it now.
        #[arg(long)]
        no_start: bool,
    },
    /// Remove the login service registered by `tome daemon install`.
    Uninstall,
}

fn main() {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            // Help/version go to stdout with exit 0; usage errors exit 2.
            let code = if e.use_stderr() { output::exit::INVALID } else { output::exit::OK };
            let _ = e.print();
            std::process::exit(code);
        }
    };
    let mode = Mode::resolve(cli.json);

    if let Command::Daemon { command: DaemonCommand::Run } = cli.command {
        if let Err(e) = daemon::run_foreground() {
            eprintln!("tome daemon: {e:#}");
            std::process::exit(output::exit::FAILURE);
        }
        return;
    }

    let code = emit(mode, dispatch(cli.command));
    std::process::exit(code);
}

fn dispatch(command: Command) -> CliResult<Report> {
    match command {
        Command::Daemon { command } => match command {
            DaemonCommand::Start => lifecycle::start(),
            DaemonCommand::Stop => lifecycle::stop(),
            DaemonCommand::Status => lifecycle::status(),
            DaemonCommand::Install { print, no_start } => service::install(print, no_start),
            DaemonCommand::Uninstall => service::uninstall(),
            DaemonCommand::Run => unreachable!("handled in main"),
        },
        Command::Validate { workflow, params } => validate::run(&current_dir()?, workflow.as_deref(), &params),
        Command::Runs { command } => match command {
            RunsCommand::List { status, workflow, limit } => inspect::list(status, workflow, limit),
            RunsCommand::Show { id, snapshot } => inspect::show(&id, snapshot),
            RunsCommand::Logs { id, step, tail } => inspect::logs(&id, step, tail),
        },
        Command::Query { sql } => inspect::query(&sql),
    }
}

fn current_dir() -> CliResult<std::path::PathBuf> {
    Ok(std::env::current_dir()?)
}
