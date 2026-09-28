mod api;
mod arming;
mod bus;
mod config;
mod cron;
mod daemon;
mod duration;
mod engine;
mod eventscmd;
mod gc;
mod glob;
mod handshake;
mod harness;
mod inspect;
mod lifecycle;
mod orchestrator;
mod output;
mod paths;
mod placement;
mod primitives;
mod query;
mod recovery;
mod rpc;
mod runcmd;
mod scaffold;
mod service;
mod session;
mod store;
mod topic;
mod triggers;
mod triggerscmd;
mod validate;
mod watch;
mod workers;
mod workflow;
mod worktree;

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
        #[command(flatten)]
        placement: PlacementArgs,
    },
    /// Tell tome this agent has started: the first thing an agent runs.
    Ready,
    /// Report step progress for a run (used by the orchestrator).
    Step {
        #[command(subcommand)]
        command: StepCommand,
    },
    /// Spawn and track workers (used by the orchestrator; `done`/`fail` by workers).
    Worker {
        #[command(subcommand)]
        command: WorkerCommand,
    },
    /// Move a run's live sessions.
    Session {
        #[command(subcommand)]
        command: SessionCommand,
    },
    /// Group workers to wait on them together.
    Group {
        #[command(subcommand)]
        command: GroupCommand,
    },
    /// Create git worktrees for a run.
    Worktree {
        #[command(subcommand)]
        command: WorktreeCommand,
    },
    /// Pass messages between a run's orchestrator and workers.
    Queue {
        #[command(subcommand)]
        command: QueueCommand,
    },
    /// Inspect current and past runs.
    Runs {
        #[command(subcommand)]
        command: RunsCommand,
    },
    /// Fire and inspect workflow triggers.
    Triggers {
        #[command(subcommand)]
        command: TriggersCommand,
    },
    /// Publish an event to a topic on the project's message bus.
    Publish {
        /// Topic, e.g. review.requested.
        topic: String,
        /// The payload (`-` reads it from stdin).
        text: String,
        /// Show which workflows would get it, without publishing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Inspect and repair the project's message bus.
    Events {
        #[command(subcommand)]
        command: EventsCommand,
    },
    /// Session placement: list the layout presets.
    Layout {
        #[command(subcommand)]
        command: LayoutCommand,
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

#[derive(clap::Args)]
struct RunArg {
    /// Run id (defaults to TOME_RUN_ID).
    #[arg(long = "run", env = "TOME_RUN_ID", value_name = "ID")]
    run: Option<String>,
}

#[derive(Subcommand)]
enum WorkerCommand {
    /// Start a worker: an agent with a task (--prompt), or a command (after `--`).
    Spawn {
        /// Worker name, unique in the run (default: w1, w2, ...).
        #[arg(long)]
        name: Option<String>,
        /// Add the worker to this group (created on first use).
        #[arg(long)]
        group: Option<String>,
        /// Give the worker its own git worktree on branch tome/<run>/<name>.
        #[arg(long)]
        worktree: bool,
        /// What the worktree branches from (default: the current HEAD commit).
        #[arg(long, value_name = "REF")]
        base: Option<String>,
        /// Harness for an agent worker (default: the workflow's, else claude).
        #[arg(long)]
        harness: Option<String>,
        /// Keep the worker's session open after it finishes.
        #[arg(long)]
        keep_open: bool,
        #[command(flatten)]
        placement: PlacementArgs,
        /// The agent worker's task.
        #[arg(long, conflicts_with = "prompt_file")]
        prompt: Option<String>,
        /// Read the agent worker's task from a file.
        #[arg(long, value_name = "PATH")]
        prompt_file: Option<std::path::PathBuf>,
        #[command(flatten)]
        run: RunArg,
        /// The command worker's command and arguments.
        #[arg(last = true, value_name = "COMMAND")]
        command: Vec<String>,
    },
    /// Report that this worker's task is done (used by workers).
    Done {
        /// A line or two on what was done.
        #[arg(long, short)]
        summary: Option<String>,
        /// Worker name (defaults to TOME_WORKER_ID).
        #[arg(long, env = "TOME_WORKER_ID", hide_env_values = true)]
        name: Option<String>,
        #[command(flatten)]
        run: RunArg,
    },
    /// Report that this worker's task failed (used by workers).
    Fail {
        /// What went wrong.
        #[arg(long, short)]
        summary: Option<String>,
        /// Worker name (defaults to TOME_WORKER_ID).
        #[arg(long, env = "TOME_WORKER_ID", hide_env_values = true)]
        name: Option<String>,
        #[command(flatten)]
        run: RunArg,
    },
    /// Wait for a worker to finish, then print its status.
    Wait {
        name: String,
        #[command(flatten)]
        run: RunArg,
    },
    /// A worker's status, summary, branch and worktree (all workers if no name).
    Status {
        name: Option<String>,
        #[command(flatten)]
        run: RunArg,
    },
    /// Stop a worker (it's marked cancelled).
    Kill {
        name: String,
        #[command(flatten)]
        run: RunArg,
    },
}

#[derive(Subcommand)]
enum GroupCommand {
    /// Create a group up front.
    Create {
        name: String,
        /// The first failure cancels the other members.
        #[arg(long)]
        fail_fast: bool,
        #[command(flatten)]
        run: RunArg,
    },
    /// Stop a group taking new members.
    Close {
        name: String,
        #[command(flatten)]
        run: RunArg,
    },
    /// Close a group and wait until all its members have finished.
    Wait {
        name: String,
        #[command(flatten)]
        run: RunArg,
    },
    /// A group's state and its members' results.
    Status {
        name: String,
        #[command(flatten)]
        run: RunArg,
    },
}

#[derive(Subcommand)]
enum WorktreeCommand {
    /// Create a worktree that isn't tied to a worker.
    Create {
        name: String,
        /// What the branch starts from (default: the current HEAD commit).
        #[arg(long, value_name = "REF")]
        base: Option<String>,
        #[command(flatten)]
        run: RunArg,
    },
}

#[derive(Subcommand)]
enum QueueCommand {
    /// Add a message to a queue (`-` reads it from stdin).
    Push {
        queue: String,
        text: String,
        #[command(flatten)]
        run: RunArg,
    },
    /// Claim the next message (exit 3 if there's none).
    Pull {
        queue: String,
        /// Wait for a message, for at most DURATION (e.g. 30s, 5m) or forever.
        #[arg(long, value_name = "DURATION", num_args = 0..=1, default_missing_value = "forever")]
        wait: Option<String>,
        #[command(flatten)]
        run: RunArg,
    },
    /// Remove a message you've claimed.
    Ack {
        id: String,
        #[command(flatten)]
        run: RunArg,
    },
    /// Stop a queue taking messages; pulls say `closed` once it's drained.
    Close {
        queue: String,
        #[command(flatten)]
        run: RunArg,
    },
    /// List the run's queues.
    Ls {
        #[command(flatten)]
        run: RunArg,
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

/// Where a session goes; each flag overrides the workflow and config.
#[derive(clap::Args, Default)]
struct PlacementArgs {
    /// A layout preset: built-in (tab, split, workspace) or from `layout_presets`.
    #[arg(long, value_name = "NAME", help_heading = "Placement")]
    preset: Option<String>,
    /// tab, split, or workspace (a workspace of its own).
    #[arg(long, value_name = "LAYOUT", help_heading = "Placement")]
    layout: Option<String>,
    /// project, focused, own, or a name for a `<project>-<name>` workspace.
    #[arg(long, value_name = "WORKSPACE", help_heading = "Placement")]
    workspace: Option<String>,
    /// Which way a split opens: right, left, down, or up.
    #[arg(long, value_name = "DIR", help_heading = "Placement")]
    direction: Option<String>,
    /// A split's size: a percentage (30%) or cells (80).
    #[arg(long, value_name = "SIZE", help_heading = "Placement")]
    size: Option<String>,
    /// The pane a split or tab opens from: orchestrator, last, or first.
    #[arg(long, value_name = "PANE", help_heading = "Placement")]
    from: Option<String>,
}

impl PlacementArgs {
    fn settings(&self) -> CliResult<placement::Settings> {
        placement::from_flags(
            self.preset.as_deref(),
            self.layout.as_deref(),
            self.workspace.as_deref(),
            self.direction.as_deref(),
            self.size.as_deref(),
            self.from.as_deref(),
        )
    }
}

#[derive(Subcommand)]
enum SessionCommand {
    /// Move an orchestrator or worker session without restarting it; settings
    /// the flags don't give stay as they were.
    Move {
        /// `<run>/<name>`: `orchestrator` or a worker's name (the run defaults
        /// to TOME_RUN_ID).
        #[arg(value_name = "RUN/NAME")]
        session: String,
        #[command(flatten)]
        placement: PlacementArgs,
    },
}

#[derive(Subcommand)]
enum LayoutCommand {
    /// List the built-in, global and project layout presets with their settings.
    Presets,
}

#[derive(Subcommand)]
enum TriggersCommand {
    /// List each project's armed triggers, with when each last fired.
    Ls,
    /// Resume a project's triggers.
    Enable {
        /// The project (default: the current one).
        #[arg(long, value_name = "PATH")]
        project: Option<std::path::PathBuf>,
    },
    /// Pause a project's triggers; the setting persists.
    Disable {
        /// The project (default: the current one).
        #[arg(long, value_name = "PATH")]
        project: Option<std::path::PathBuf>,
    },
    /// Fire a workflow's trigger with a synthetic event.
    Fire {
        /// Workflow name or path to a workflow file.
        workflow: String,
        /// Which trigger (0-based); defaults to the first non-manual one.
        #[arg(long)]
        index: Option<usize>,
        /// A changed path to report (file triggers); repeatable.
        #[arg(long = "path", value_name = "PATH")]
        paths: Vec<String>,
        /// Fire a topic trigger with a test event carrying this payload,
        /// delivered to this workflow only.
        #[arg(long)]
        payload: Option<String>,
        /// Show what would happen, with the resolved params, without doing it.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum EventsCommand {
    /// List the project's topics, with each subscriber's backlog.
    Ls,
    /// List a topic's events that aren't settled yet.
    Show {
        topic: String,
        /// Include events every workflow is done with.
        #[arg(long)]
        all: bool,
    },
    /// Hand a failed delivery of an event out again.
    Retry {
        /// The event id.
        event: i64,
        /// Which workflow's delivery, if it went to several.
        #[arg(long)]
        workflow: Option<String>,
    },
    /// Drop a pending or failed delivery of an event.
    Remove {
        /// The event id.
        event: i64,
        /// Which workflow's delivery, if it went to several.
        #[arg(long)]
        workflow: Option<String>,
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

    if !matches!(cli.command, Command::Daemon { .. }) {
        if let Ok(cwd) = std::env::current_dir() {
            triggerscmd::register(&cwd);
        }
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
        Command::Run { command: None, workflow, params, detach, placement } => {
            let workflow = workflow.expect("clap requires a workflow");
            let placement = placement.settings()?;
            if detach {
                runcmd::start_detached(&current_dir()?, &workflow, &params, &placement)
            } else {
                runcmd::start_attached(&current_dir()?, &workflow, &params, &placement, mode)
            }
        }
        Command::Ready => runcmd::ready(),
        Command::Step { command } => match command {
            StepCommand::Start { name, report } => runcmd::step("start", Some(name), report.message, report.run),
            StepCommand::Done { name, report } => runcmd::step("done", name, report.message, report.run),
            StepCommand::Fail { name, report } => runcmd::step("fail", name, report.message, report.run),
        },
        Command::Worker { command } => match command {
            WorkerCommand::Spawn {
                name,
                group,
                worktree,
                base,
                harness,
                keep_open,
                placement,
                prompt,
                prompt_file,
                run,
                command,
            } => primitives::spawn(primitives::Spawn {
                placement: placement.settings()?,
                run: run.run,
                name,
                group,
                worktree,
                base,
                harness,
                keep_open,
                prompt,
                prompt_file,
                command,
            }),
            WorkerCommand::Done { summary, name, run } => primitives::report("done", summary, name, run.run),
            WorkerCommand::Fail { summary, name, run } => primitives::report("fail", summary, name, run.run),
            WorkerCommand::Wait { name, run } => primitives::worker_status(Some(name), true, run.run),
            WorkerCommand::Status { name, run } => primitives::worker_status(name, false, run.run),
            WorkerCommand::Kill { name, run } => primitives::kill(name, run.run),
        },
        Command::Session { command: SessionCommand::Move { session, placement } } => {
            primitives::session_move(&session, &placement.settings()?)
        }
        Command::Group { command } => match command {
            GroupCommand::Create { name, fail_fast, run } => primitives::group_create(name, fail_fast, run.run),
            GroupCommand::Close { name, run } => primitives::group("group.close", name, false, run.run),
            GroupCommand::Wait { name, run } => primitives::group("group.status", name, true, run.run),
            GroupCommand::Status { name, run } => primitives::group("group.status", name, false, run.run),
        },
        Command::Worktree { command: WorktreeCommand::Create { name, base, run } } => {
            primitives::worktree_create(name, base, run.run)
        }
        Command::Queue { command } => match command {
            QueueCommand::Push { queue, text, run } => primitives::push(queue, text, run.run),
            QueueCommand::Pull { queue, wait, run } => primitives::pull(queue, wait, run.run),
            QueueCommand::Ack { id, run } => primitives::ack(id, run.run),
            QueueCommand::Close { queue, run } => primitives::close(queue, run.run),
            QueueCommand::Ls { run } => primitives::ls(run.run),
        },
        Command::Runs { command } => match command {
            RunsCommand::List { status, workflow, limit } => inspect::list(status, workflow, limit),
            RunsCommand::Show { id, snapshot } => inspect::show(&id, snapshot),
            RunsCommand::Logs { id, step, tail } => inspect::logs(&id, step, tail),
        },
        Command::Triggers { command } => match command {
            TriggersCommand::Ls => triggerscmd::ls(),
            TriggersCommand::Enable { project } => triggerscmd::enable(&current_dir()?, project, true),
            TriggersCommand::Disable { project } => triggerscmd::enable(&current_dir()?, project, false),
            TriggersCommand::Fire { workflow, index, paths, payload, dry_run } => {
                triggerscmd::fire(&current_dir()?, &workflow, index, &paths, payload.as_deref(), dry_run)
            }
        },
        Command::Publish { topic, text, dry_run } => eventscmd::publish(&current_dir()?, &topic, text, dry_run),
        Command::Events { command } => match command {
            EventsCommand::Ls => eventscmd::ls(&current_dir()?),
            EventsCommand::Show { topic, all } => eventscmd::show(&current_dir()?, &topic, all),
            EventsCommand::Retry { event, workflow } => eventscmd::retry(&current_dir()?, event, workflow.as_deref()),
            EventsCommand::Remove { event, workflow } => eventscmd::remove(&current_dir()?, event, workflow.as_deref()),
        },
        Command::Layout { command: LayoutCommand::Presets } => placement::presets_report(&current_dir()?),
        Command::Query { sql } => inspect::query(&sql),
        Command::Gc { older_than, dry_run } => gc::run(&older_than, dry_run),
    }
}

fn current_dir() -> CliResult<std::path::PathBuf> {
    Ok(std::env::current_dir()?)
}
