mod api;
mod daemon;
mod duration;
mod engine;
mod gc;
mod harness;
mod inspect;
mod lifecycle;
mod orchestrator;
mod output;
mod paths;
mod query;
mod recovery;
mod rpc;
mod runcmd;
mod scaffold;
mod service;
mod session;
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
    /// Create workflows.
    Workflow {
        #[command(subcommand)]
        command: WorkflowCommand,
    },
    /// Validate workflows (all visible ones, or one by name or path).
    Validate {
        /// Workflow name or path to a workflow file.
        workflow: Option<String>,
        /// Check a parameter value (key=value); repeatable.
        #[arg(long = "param", value_name = "KEY=VALUE")]
        params: Vec<String>,
    },
    /// Start a workflow run (`tome run <workflow>`), or finish/cancel one.
    #[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
    Run {
        #[command(subcommand)]
        command: Option<RunCommand>,
        /// Workflow name or path to a workflow file.
        #[arg(required = true)]
        workflow: Option<String>,
        /// Set a parameter (key=value); repeatable.
        #[arg(long = "param", value_name = "KEY=VALUE")]
        params: Vec<String>,
        /// Return the run id right away instead of streaming the run.
        #[arg(long)]
        detach: bool,
    },
    /// Report step progress for a run (used by the orchestrator).
    Step {
        #[command(subcommand)]
        command: StepCommand,
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
    /// Delete finished runs older than an age, with their logs and worktrees.
    Gc {
        /// Minimum age since the run finished, e.g. 7d, 12h, 2w.
        #[arg(long, value_name = "AGE")]
        older_than: String,
        /// Show what would be deleted without deleting anything.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum RunCommand {
    /// End a run with its final status (used by the orchestrator).
    Finish {
        /// Final status: succeeded or failed.
        #[arg(long)]
        status: String,
        /// A short summary of the outcome.
        #[arg(long)]
        summary: Option<String>,
        /// Run id (defaults to TOME_RUN_ID).
        #[arg(long = "run", env = "TOME_RUN_ID", value_name = "ID")]
        run: Option<String>,
    },
    /// Cancel a run: kill its sessions, keep its worktrees.
    Cancel {
        /// Run id (defaults to TOME_RUN_ID).
        #[arg(env = "TOME_RUN_ID")]
        id: Option<String>,
    },
}

#[derive(Subcommand)]
enum WorkflowCommand {
    /// Write a starter workflow file where `tome run <name>` will find it.
    New {
        /// Workflow name (letters, digits, `_` and `-`).
        name: String,
        /// One-line description to put in the frontmatter.
        #[arg(long, short)]
        description: Option<String>,
        /// Create it in ~/.tome/workflows instead of the project.
        #[arg(long)]
        global: bool,
        /// Overwrite the file if it already exists.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum StepCommand {
    /// Report that a step has started (again, if it's being retried).
    Start {
        /// Step name, usually the workflow heading it carries out.
        name: String,
        #[command(flatten)]
        report: StepReport,
    },
    /// Report that a step is done (defaults to the running step).
    Done {
        /// Step name.
        name: Option<String>,
        #[command(flatten)]
        report: StepReport,
    },
    /// Report that a step failed (defaults to the running step).
    Fail {
        /// Step name.
        name: Option<String>,
        #[command(flatten)]
        report: StepReport,
    },
}

#[derive(clap::Args)]
struct StepReport {
    /// A short note recorded with the transition.
    #[arg(long, short)]
    message: Option<String>,
    /// Run id (defaults to TOME_RUN_ID).
    #[arg(long = "run", env = "TOME_RUN_ID", value_name = "ID")]
    run: Option<String>,
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

    let code = emit(mode, dispatch(cli.command, mode));
    std::process::exit(code);
}

fn dispatch(command: Command, mode: Mode) -> CliResult<Report> {
    match command {
        Command::Daemon { command } => match command {
            DaemonCommand::Start => lifecycle::start(),
            DaemonCommand::Stop => lifecycle::stop(),
            DaemonCommand::Status => lifecycle::status(),
            DaemonCommand::Install { print, no_start } => service::install(print, no_start),
            DaemonCommand::Uninstall => service::uninstall(),
            DaemonCommand::Run => unreachable!("handled in main"),
        },
        Command::Workflow { command } => match command {
            WorkflowCommand::New { name, description, global, force } => {
                scaffold::new(&current_dir()?, &name, description.as_deref(), global, force)
            }
        },
        Command::Validate { workflow, params } => validate::run(&current_dir()?, workflow.as_deref(), &params),
        Command::Run { command: Some(command), .. } => match command {
            RunCommand::Finish { status, summary, run } => runcmd::finish(run, &status, summary),
            RunCommand::Cancel { id } => runcmd::cancel(id),
        },
        Command::Run { command: None, workflow, params, detach } => {
            let workflow = workflow.expect("clap requires a workflow");
            if detach {
                runcmd::start_detached(&current_dir()?, &workflow, &params)
            } else {
                runcmd::start_attached(&current_dir()?, &workflow, &params, mode)
            }
        }
        Command::Step { command } => match command {
            StepCommand::Start { name, report } => runcmd::step("start", Some(name), report.message, report.run),
            StepCommand::Done { name, report } => runcmd::step("done", name, report.message, report.run),
            StepCommand::Fail { name, report } => runcmd::step("fail", name, report.message, report.run),
        },
        Command::Runs { command } => match command {
            RunsCommand::List { status, workflow, limit } => inspect::list(status, workflow, limit),
            RunsCommand::Show { id, snapshot } => inspect::show(&id, snapshot),
            RunsCommand::Logs { id, step, tail } => inspect::logs(&id, step, tail),
        },
        Command::Query { sql } => inspect::query(&sql),
        Command::Gc { older_than, dry_run } => gc::run(&older_than, dry_run),
    }
}

fn current_dir() -> CliResult<std::path::PathBuf> {
    Ok(std::env::current_dir()?)
}
