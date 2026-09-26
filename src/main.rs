mod daemon;
mod lifecycle;
mod output;
mod paths;
mod rpc;

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
    }
}
