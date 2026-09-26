mod api;
mod daemon;
mod duration;
mod lifecycle;
mod output;
mod paths;
mod rpc;
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
            DaemonCommand::Run => unreachable!("handled in main"),
        },
        Command::Validate { workflow, params } => validate::run(&current_dir()?, workflow.as_deref(), &params),
    }
}

fn current_dir() -> CliResult<std::path::PathBuf> {
    Ok(std::env::current_dir()?)
}
