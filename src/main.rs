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
mod harnesscmd;
mod inspect;
mod lifecycle;
mod node;
mod nodecmd;
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

    /// Send the command to a node's daemon (also TOME_NODE; `local` is this
    /// machine).
    #[arg(long, global = true, value_name = "NODE")]
    on: Option<String>,

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
    /// Start the daemon in the background (same as `tome daemon start`).
    Start,
    /// Stop the running daemon (same as `tome daemon stop`).
    Stop,
    /// Create, list and remove workflows.
    Workflow {
        #[command(subcommand)]
        command: WorkflowCommand,
    },
    /// Inspect configured agent harnesses.
    Harness {
        #[command(subcommand)]
        command: HarnessCommand,
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
        /// Use this harness for the main agent and all workers.
        #[arg(long)]
        harness: Option<String>,
        /// Use this model for the main agent and all workers.
        #[arg(long)]
        model: Option<String>,
        /// Return the run id right away instead of streaming the run.
        #[arg(long)]
        detach: bool,
        /// Start detached, wait for the run's agent to start, then view its
        /// session (`tome session view`).
        #[arg(long, conflicts_with = "detach")]
        view: bool,
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
    /// View or move a run's live sessions.
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
    /// Project message queues: work for runs, workers and you to pass around.
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
    /// Other machines to send commands to with `--on <node>`.
    Node {
        #[command(subcommand)]
        command: NodeCommand,
    },
    /// Relay JSON-RPC between stdin/stdout and the daemon (what a node runs
    /// at the other end of `ssh`).
    #[command(hide = true)]
    Rpc {
        #[arg(long)]
        stdio: bool,
    },
}

#[derive(Subcommand)]
enum NodeCommand {
    /// Add (or replace) a node in ~/.tome/config.yaml, then check it.
    Add {
        /// Node name: lowercase letters, digits, `_` and `-`.
        name: String,
        /// Anything `ssh` accepts: user@host, or a Host alias.
        ssh: String,
        /// The tome binary on the node (default: `tome` on its PATH).
        #[arg(long, value_name = "PATH")]
        tome: Option<String>,
    },
    /// Remove a node from ~/.tome/config.yaml.
    Rm { name: String },
    /// List the nodes: reachable, tome version, daemon, round trip.
    Ls,
    /// Check a node step by step, with the fix for the first failure.
    Check { name: String },
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
    /// List the workflows `tome run` can see, project ones first.
    Ls {
        /// Only the ones in ~/.tome/workflows.
        #[arg(long)]
        global: bool,
    },
    /// Delete a workflow file (no prompt; runs and logs are kept).
    Rm {
        /// Workflow name, or path to a `.md` file in a workflows directory.
        workflow: String,
        /// Look the name up in ~/.tome/workflows instead of the project.
        #[arg(long)]
        global: bool,
        /// Delete it even if it has running or queued runs (or the daemon is down).
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum HarnessCommand {
    /// Check harness config, command availability and the expanded command.
    Validate {
        /// Harness name (defaults to every known harness).
        name: Option<String>,
        /// Check whether this model can be passed to the harness.
        #[arg(long)]
        model: Option<String>,
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
        /// Give the worker its own git worktree on a new branch (default: tome/<run>/<name>).
        #[arg(long)]
        worktree: bool,
        /// What the worktree branches from (default: the current HEAD commit).
        #[arg(long, value_name = "REF")]
        base: Option<String>,
        /// Name the worktree's new branch (default: tome/<run>/<name>).
        #[arg(long, value_name = "NAME")]
        branch: Option<String>,
        /// Harness for an agent worker (default: the workflow's, else the orchestrator's).
        #[arg(long)]
        harness: Option<String>,
        /// Model for an agent worker (default: the workflow's `defaults.model`).
        #[arg(long)]
        model: Option<String>,
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
        /// Name the new branch (default: tome/<run>/<name>).
        #[arg(long, value_name = "NAME")]
        branch: Option<String>,
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
    /// Show a queue's oldest messages without claiming any.
    Peek {
        queue: String,
        /// How many messages to show.
        #[arg(long, default_value_t = 20)]
        limit: usize,
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
    /// List the project's queues.
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
        /// Maximum number of runs to show (per node with --nodes).
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Include every configured node, with a NODE column.
        #[arg(long)]
        nodes: bool,
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
    /// The pane a split or tab opens from: orchestrator, last, first, or
    /// caller (the cmux pane running this command; the orchestrator only).
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
    /// Show a run's agent, orchestrator or worker session from here: focus
    /// it, open a tab attached to it, or attach in this terminal.
    View {
        /// `<run>` or `<run>/<worker>` (`<node>:<run>` for a node's run).
        #[arg(value_name = "RUN[/WORKER]")]
        session: String,
        #[command(flatten)]
        placement: PlacementArgs,
    },
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
            let code = if e.use_stderr() {
                output::exit::INVALID
            } else {
                output::exit::OK
            };
            let _ = e.print();
            std::process::exit(code);
        }
    };
    let mode = Mode::resolve(cli.json);

    if let Command::Rpc { stdio } = cli.command {
        if !stdio {
            let e = output::CliError::invalid("`tome rpc` needs --stdio");
            std::process::exit(emit(mode, Err(e)));
        }
        if let Err(e) = node::relay() {
            eprintln!("tome rpc: {e:#}");
            std::process::exit(output::exit::FAILURE);
        }
        return;
    }

    if let Command::Daemon {
        command: DaemonCommand::Run,
    } = cli.command
    {
        if let Err(e) = daemon::run_foreground() {
            eprintln!("tome daemon: {e:#}");
            std::process::exit(output::exit::FAILURE);
        }
        return;
    }

    let mut command = match cli.command {
        Command::Start => Command::Daemon {
            command: DaemonCommand::Start,
        },
        Command::Stop => Command::Daemon {
            command: DaemonCommand::Stop,
        },
        other => other,
    };
    match route(&mut command, cli.on) {
        Ok(Some(node)) => node::set_target(node),
        Ok(None) => {}
        Err(e) => std::process::exit(emit(mode, Err(e))),
    }

    if node::target().is_none() && !matches!(command, Command::Daemon { .. } | Command::Node { .. })
    {
        if let Ok(cwd) = std::env::current_dir() {
            triggerscmd::register(&cwd);
        }
    }
    let code = emit(mode, dispatch(command, mode));
    std::process::exit(code);
}

/// Where a command can go.
enum Reach {
    /// A node's daemon, with `--on`.
    Node,
    /// Only this machine: it reads local files, or manages this daemon.
    Local,
    /// Only the run it's in: what agents call.
    Agent,
}

fn reach(command: &Command) -> Reach {
    match command {
        Command::Run {
            command: Some(RunCommand::Finish { .. }),
            ..
        }
        | Command::Ready
        | Command::Step { .. }
        | Command::Worker { .. }
        | Command::Group { .. }
        | Command::Worktree { .. }
        | Command::Queue { .. }
        | Command::Session {
            command: SessionCommand::Move { .. },
        } => Reach::Agent,
        Command::Validate { .. }
        | Command::Harness { .. }
        | Command::Workflow { .. }
        | Command::Layout { .. }
        | Command::Node { .. }
        | Command::Rpc { .. }
        | Command::Runs {
            command: RunsCommand::List { nodes: true, .. },
        }
        | Command::Daemon {
            command:
                DaemonCommand::Start
                | DaemonCommand::Run
                | DaemonCommand::Install { .. }
                | DaemonCommand::Uninstall,
        } => Reach::Local,
        _ => Reach::Node,
    }
}

/// Take the node off a `<node>:<id>` run reference, leaving the id.
fn take_ref(command: &mut Command) -> Option<String> {
    let id = match command {
        Command::Runs {
            command: RunsCommand::Show { id, .. } | RunsCommand::Logs { id, .. },
        } => id,
        Command::Run {
            command: Some(RunCommand::Cancel { id: Some(id) }),
            ..
        } => id,
        Command::Session {
            command: SessionCommand::View { session, .. },
        } => session,
        _ => return None,
    };
    let (node, rest) = node::split_ref(id);
    let node = node?.to_string();
    *id = rest.to_string();
    Some(node)
}

/// The node this command goes to: `--on`, `TOME_NODE` or a `<node>:<id>`
/// reference. A command that can't go to a node refuses an explicit `--on`;
/// `TOME_NODE` alone leaves it here, and inside a run `TOME_NODE` is
/// ignored, so an agent on a machine with `TOME_NODE` set still reaches its
/// own run.
fn route(command: &mut Command, on: Option<String>) -> CliResult<Option<node::Node>> {
    let from_ref = take_ref(command);
    let on = on.filter(|o| !o.trim().is_empty());
    let explicit = on.as_deref().filter(|o| *o != node::LOCAL);
    match reach(command) {
        Reach::Node => {}
        Reach::Local => {
            let Some(name) = explicit else {
                return Ok(None);
            };
            let dest = node::get(name)
                .map(|n| n.ssh)
                .unwrap_or_else(|_| name.to_string());
            let what = match command {
                Command::Runs { .. } => {
                    "--nodes lists every node already; leave out --on".to_string()
                }
                Command::Node { .. } => {
                    "`tome node` manages this machine's node list; leave out --on".to_string()
                }
                _ => format!("run it on the node instead: `ssh {dest} tome …`"),
            };
            return Err(output::CliError::invalid(
                "this command works on this machine only, so it doesn't take --on",
            )
            .with_hint(what));
        }
        Reach::Agent => {
            if explicit.is_some() {
                return Err(output::CliError::invalid(
                    "agent commands act on the run they're in, on this machine, so they don't take --on",
                )
                .with_hint("to reach another machine's run, use `tome runs …` or `tome run cancel` with --on"));
            }
            return Ok(None);
        }
    }
    // An agent's commands stay on its own machine unless it says --on.
    let in_run = std::env::var("TOME_RUN_ID").is_ok_and(|v| !v.trim().is_empty());
    let env = std::env::var("TOME_NODE")
        .ok()
        .filter(|o| !o.trim().is_empty() && !in_run);
    node::resolve(on.or(env).as_deref(), from_ref.as_deref())
}

fn dispatch(command: Command, mode: Mode) -> CliResult<Report> {
    match command {
        Command::Start | Command::Stop => unreachable!("rewritten to `daemon` in main"),
        Command::Daemon { command } => match command {
            DaemonCommand::Start => lifecycle::start(),
            DaemonCommand::Stop => lifecycle::stop(),
            DaemonCommand::Status => lifecycle::status(),
            DaemonCommand::Install { print, no_start } => service::install(print, no_start),
            DaemonCommand::Uninstall => service::uninstall(),
            DaemonCommand::Run => unreachable!("handled in main"),
        },
        Command::Workflow { command } => match command {
            WorkflowCommand::New {
                name,
                description,
                global,
                force,
            } => scaffold::new(
                &current_dir()?,
                &name,
                description.as_deref(),
                global,
                force,
            ),
            WorkflowCommand::Ls { global } => scaffold::ls(&current_dir()?, global),
            WorkflowCommand::Rm {
                workflow,
                global,
                force,
            } => scaffold::rm(&current_dir()?, &workflow, global, force),
        },
        Command::Harness { command } => match command {
            HarnessCommand::Validate { name, model } => {
                harnesscmd::validate(&current_dir()?, name.as_deref(), model.as_deref())
            }
        },
        Command::Validate { workflow, params } => {
            validate::run(&current_dir()?, workflow.as_deref(), &params)
        }
        Command::Run {
            command: Some(command),
            ..
        } => match command {
            RunCommand::Finish {
                status,
                summary,
                run,
            } => runcmd::finish(run, &status, summary),
            RunCommand::Cancel { id } => runcmd::cancel(id),
        },
        Command::Run {
            command: None,
            workflow,
            params,
            harness,
            model,
            detach,
            view,
            placement,
        } => {
            let workflow = workflow.expect("clap requires a workflow");
            let placement = placement.settings()?;
            if view {
                nodecmd::run_and_view(
                    &current_dir()?,
                    &workflow,
                    &params,
                    &placement,
                    harness.as_deref(),
                    model.as_deref(),
                )
            } else if detach {
                runcmd::start_detached(
                    &current_dir()?,
                    &workflow,
                    &params,
                    &placement,
                    harness.as_deref(),
                    model.as_deref(),
                )
            } else {
                runcmd::start_attached(
                    &current_dir()?,
                    &workflow,
                    &params,
                    &placement,
                    mode,
                    harness.as_deref(),
                    model.as_deref(),
                )
            }
        }
        Command::Ready => runcmd::ready(),
        Command::Step { command } => match command {
            StepCommand::Start { name, report } => {
                runcmd::step("start", Some(name), report.message, report.run)
            }
            StepCommand::Done { name, report } => {
                runcmd::step("done", name, report.message, report.run)
            }
            StepCommand::Fail { name, report } => {
                runcmd::step("fail", name, report.message, report.run)
            }
        },
        Command::Worker { command } => match command {
            WorkerCommand::Spawn {
                name,
                group,
                worktree,
                base,
                branch,
                harness,
                model,
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
                branch,
                harness,
                model,
                keep_open,
                prompt,
                prompt_file,
                command,
            }),
            WorkerCommand::Done { summary, name, run } => {
                primitives::report("done", summary, name, run.run)
            }
            WorkerCommand::Fail { summary, name, run } => {
                primitives::report("fail", summary, name, run.run)
            }
            WorkerCommand::Wait { name, run } => {
                primitives::worker_status(Some(name), true, run.run)
            }
            WorkerCommand::Status { name, run } => primitives::worker_status(name, false, run.run),
            WorkerCommand::Kill { name, run } => primitives::kill(name, run.run),
        },
        Command::Session {
            command: SessionCommand::Move { session, placement },
        } => primitives::session_move(&session, &placement.settings()?),
        Command::Session {
            command: SessionCommand::View { session, placement },
        } => nodecmd::view(&session, &placement.settings()?),
        Command::Group { command } => match command {
            GroupCommand::Create {
                name,
                fail_fast,
                run,
            } => primitives::group_create(name, fail_fast, run.run),
            GroupCommand::Close { name, run } => {
                primitives::group("group.close", name, false, run.run)
            }
            GroupCommand::Wait { name, run } => {
                primitives::group("group.status", name, true, run.run)
            }
            GroupCommand::Status { name, run } => {
                primitives::group("group.status", name, false, run.run)
            }
        },
        Command::Worktree {
            command:
                WorktreeCommand::Create {
                    name,
                    base,
                    branch,
                    run,
                },
        } => primitives::worktree_create(name, base, branch, run.run),
        Command::Queue { command } => match command {
            QueueCommand::Push { queue, text, run } => {
                primitives::push(&current_dir()?, queue, text, run.run)
            }
            QueueCommand::Pull { queue, wait, run } => {
                primitives::pull(&current_dir()?, queue, wait, run.run)
            }
            QueueCommand::Peek { queue, limit, run } => {
                primitives::peek(&current_dir()?, queue, limit, run.run)
            }
            QueueCommand::Ack { id, run } => primitives::ack(&current_dir()?, id, run.run),
            QueueCommand::Close { queue, run } => {
                primitives::close(&current_dir()?, queue, run.run)
            }
            QueueCommand::Ls { run } => primitives::ls(&current_dir()?, run.run),
        },
        Command::Runs { command } => match command {
            RunsCommand::List {
                status,
                workflow,
                limit,
                nodes,
            } => {
                if nodes {
                    nodecmd::runs_everywhere(status, workflow, limit)
                } else {
                    inspect::list(status, workflow, limit)
                }
            }
            RunsCommand::Show { id, snapshot } => inspect::show(&id, snapshot),
            RunsCommand::Logs { id, step, tail } => inspect::logs(&id, step, tail),
        },
        Command::Triggers { command } => match command {
            TriggersCommand::Ls => triggerscmd::ls(),
            TriggersCommand::Enable { project } => {
                triggerscmd::enable(&current_dir()?, project, true)
            }
            TriggersCommand::Disable { project } => {
                triggerscmd::enable(&current_dir()?, project, false)
            }
            TriggersCommand::Fire {
                workflow,
                index,
                paths,
                payload,
                dry_run,
            } => triggerscmd::fire(
                &current_dir()?,
                &workflow,
                index,
                &paths,
                payload.as_deref(),
                dry_run,
            ),
        },
        Command::Publish {
            topic,
            text,
            dry_run,
        } => eventscmd::publish(&current_dir()?, &topic, text, dry_run),
        Command::Events { command } => match command {
            EventsCommand::Ls => eventscmd::ls(&current_dir()?),
            EventsCommand::Show { topic, all } => eventscmd::show(&current_dir()?, &topic, all),
            EventsCommand::Retry { event, workflow } => {
                eventscmd::retry(&current_dir()?, event, workflow.as_deref())
            }
            EventsCommand::Remove { event, workflow } => {
                eventscmd::remove(&current_dir()?, event, workflow.as_deref())
            }
        },
        Command::Layout {
            command: LayoutCommand::Presets,
        } => placement::presets_report(&current_dir()?),
        Command::Query { sql } => inspect::query(&sql),
        Command::Gc {
            older_than,
            dry_run,
        } => gc::run(&older_than, dry_run),
        Command::Node { command } => match command {
            NodeCommand::Add { name, ssh, tome } => nodecmd::add(&name, &ssh, tome.as_deref()),
            NodeCommand::Rm { name } => nodecmd::rm(&name),
            NodeCommand::Ls => nodecmd::ls(),
            NodeCommand::Check { name } => nodecmd::check(&name),
        },
        Command::Rpc { .. } => unreachable!("handled in main"),
    }
}

fn current_dir() -> CliResult<std::path::PathBuf> {
    Ok(std::env::current_dir()?)
}
