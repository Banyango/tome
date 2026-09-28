//! Launching a run's orchestrator: an agent CLI, started through the harness
//! adapter in its own tmux session, that reads the workflow and drives the
//! run by calling tome commands.
//!
//! Everything is taken from the run's workflow snapshot, so a queued run
//! launches later exactly as it was when it was requested.

use crate::harness::{self, Harness, Vars};
use crate::output::{CliError, CliResult};
use crate::paths;
use crate::recovery::RecoveryHooks;
use crate::placement::{self, Inputs, Placement, Role};
use crate::session::{self, Backend, Cmux, Kind, Launch, Tmux};
use crate::store::{Run, Session, Worker};
use crate::workflow::{self, Frontmatter, Workflow};
use std::fs;
use std::path::{Path, PathBuf};

/// The built-in orchestrator prompt: its duties and the tome command reference.
pub const PROMPT: &str = include_str!("orchestrator_prompt.md");

pub const ROLE: &str = "orchestrator";

/// Why a run failed when its orchestrator went away before `tome run finish`.
pub const EXITED: &str = "orchestrator_exited";
/// Why a run failed when its orchestrator couldn't be started.
pub const LAUNCH_FAILED: &str = "launch_failed";

/// The harness a workflow's orchestrator runs in:
/// `defaults.orchestrator_harness`, else `defaults.harness`, else `claude`.
pub fn harness_for(fm: &Frontmatter, project: Option<&Path>) -> CliResult<Harness> {
    let d = &fm.defaults;
    harness::resolve(d.orchestrator_harness.as_deref().or(d.harness.as_deref()).unwrap_or(harness::DEFAULT), project)
}

/// Where a run's orchestrator goes.
pub fn placement(fm: &Frontmatter, project: Option<&Path>) -> CliResult<Placement> {
    let inputs = Inputs { role: Role::Orchestrator, flags: None, run_flags: None, spec: fm.defaults.layout.as_ref() };
    placement::resolve(&inputs, project)
}

/// A run's orchestrator, ready to launch.
pub struct Plan {
    pub harness: Harness,
    pub backend: Kind,
    pub placement: Placement,
    pub session: String,
    pub title: String,
    pub cwd: PathBuf,
    pub prompt: String,
    /// How long it has to make its first tome call; `None` when unchecked.
    pub start_timeout: Option<std::time::Duration>,
}

/// The workflow a run was started with (its snapshot; `run` must have been
/// read with it).
pub fn snapshot(run: &Run) -> CliResult<Workflow> {
    let snapshot = run
        .workflow_snapshot
        .as_deref()
        .ok_or_else(|| CliError::internal(format!("run {} has no workflow snapshot", run.id)))?;
    let path = PathBuf::from(run.workflow_path.clone().unwrap_or_default());
    workflow::parse_snapshot(&path, snapshot).map_err(|inv| inv.into_cli_error())
}

/// Where a run's sessions start: its project, else the user's home.
pub fn run_cwd(run: &Run) -> PathBuf {
    run_project(run).unwrap_or_else(paths::user_home)
}

/// A run's project directory, if it has one (and it's still there).
pub fn run_project(run: &Run) -> Option<PathBuf> {
    run.project_path.as_ref().map(PathBuf::from).filter(|p| p.is_dir())
}

pub fn plan(run: &Run) -> CliResult<Plan> {
    let wf = snapshot(run)?;
    let cwd = run_cwd(run);
    let project = run_project(run);
    let project = project.as_deref();
    Ok(Plan {
        harness: harness_for(&wf.frontmatter, project)?,
        backend: Kind::choose(wf.frontmatter.defaults.backend.as_deref(), project)?,
        placement: placement(&wf.frontmatter, project)?,
        session: session::run_session_name(run.id, &run.workflow_name, ROLE),
        title: format!("tome: {} #{}", run.workflow_name, run.id),
        cwd,
        prompt: bootstrap(run, &wf.frontmatter, &wf.body),
        start_timeout: crate::handshake::timeout(&wf.frontmatter),
    })
}

/// The orchestrator's first message: the built-in prompt, the resolved
/// workflow and any extra instructions the workflow gives its orchestrator.
pub fn bootstrap(run: &Run, fm: &Frontmatter, body: &str) -> String {
    let mut out = String::from(PROMPT.trim_end());
    out.push_str(&format!("\n\n## This run\n\nRun #{} of workflow `{}`", run.id, run.workflow_name));
    if let Some(desc) = &fm.description {
        out.push_str(&format!(" ({desc})"));
    }
    out.push_str(".\n");
    if let Some(params) = run.params.as_object().filter(|p| !p.is_empty()) {
        out.push_str("Parameters:\n");
        for (k, v) in params {
            let v = v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string());
            out.push_str(&format!("- {k} = {v}\n"));
        }
    }
    if let Some(extra) = fm.orchestrator.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        out.push_str(&format!("\n## Instructions for this workflow\n\n{extra}\n"));
    }
    out.push_str(&format!("\n## The workflow\n\n{}\n", body.trim()));
    out
}

/// Where a run's orchestrator prompt is written.
pub fn prompt_file(run_id: i64) -> PathBuf {
    paths::runs_dir().join(run_id.to_string()).join("orchestrator-prompt.md")
}

/// Start the orchestrator's session and return its record. The launcher
/// script, the bootstrap prompt and the session's output all go in the
/// run's directory.
pub fn launch(run: &Run, plan: &Plan) -> CliResult<Session> {
    let backend = Backend::new(plan.backend);
    let dir = paths::runs_dir().join(run.id.to_string());
    fs::create_dir_all(&dir)?;
    let prompt_file = prompt_file(run.id);
    fs::write(&prompt_file, &plan.prompt)?;
    let argv = plan.harness.command(&Vars {
        prompt: &plan.prompt,
        prompt_file: &prompt_file.to_string_lossy(),
        run_id: run.id,
        session: &plan.session,
        cwd: &plan.cwd.to_string_lossy(),
    });
    let session = backend.launch(&Launch {
        name: &plan.session,
        title: &plan.title,
        cwd: &plan.cwd,
        argv: &argv,
        env: &session_env(run.id),
        script: &dir.join("orchestrator.sh"),
        log: &dir.join(format!("{ROLE}.log")),
        layout: plan.placement.layout,
        project: run_project(run).as_deref(),
    })?;
    Ok(Session {
        run_id: run.id,
        role: ROLE.to_string(),
        layout: Some(plan.placement.layout.as_str().to_string()),
        harness: Some(plan.harness.name.clone()),
        placement: Some(plan.placement.to_json()),
        ..session
    })
}

/// What a run's agents need to call back into tome.
pub fn session_env(run_id: i64) -> Vec<(String, String)> {
    let mut env = vec![
        ("TOME_RUN_ID".to_string(), run_id.to_string()),
        ("TOME_OUTPUT".to_string(), "json".to_string()),
        ("TOME_HOME".to_string(), paths::tome_home().to_string_lossy().into_owned()),
    ];
    // So that the agent's tome commands reach the same daemon and servers.
    for key in ["TOME_TMUX_SOCKET", "TOME_BACKEND", "TOME_LAYOUT"] {
        if let Some(v) = std::env::var(key).ok().filter(|v| !v.is_empty()) {
            env.push((key.to_string(), v));
        }
    }
    // Make sure `tome` resolves to this tome.
    let path = std::env::var("PATH").unwrap_or_default();
    let path = match std::env::current_exe().ok().as_deref().and_then(Path::parent) {
        Some(bin) if !path.split(':').any(|p| Path::new(p) == bin) => format!("{}:{path}", bin.display()),
        _ => path,
    };
    env.push(("PATH".to_string(), path));
    env
}

/// Kill every session of a run: the recorded ones (on whatever backend and
/// server they were started on) and any other `tome-<id>-*` tmux session on
/// the current server. cmux is shared with the user, so only recorded
/// workspaces are closed there.
pub fn kill_sessions(run_id: i64, recorded: &[Session]) {
    for s in recorded {
        session::kill(s);
    }
    Tmux::from_env().kill_prefix(&session::run_prefix(run_id));
}

/// Tell the user a run ended without finishing itself, naming the workers
/// that were cut off (`cut`). It's always logged (stderr is the daemon
/// log); a run whose orchestrator was in cmux also gets a cmux
/// notification, unless `TOME_NOTIFY=off`.
pub fn notify(run: &Run, sessions: &[Session], cut: &[Worker]) {
    let mut message = run.reason.clone().unwrap_or_default();
    if !cut.is_empty() {
        let names: Vec<&str> = cut.iter().map(|w| w.name.as_str()).collect();
        message.push_str(&format!("; workers cut off: {}", names.join(", ")));
    }
    eprintln!("tome daemon: notify: run {} ({}) {}: {message}", run.id, run.workflow_name, run.status.as_str());
    let in_cmux = sessions.iter().any(|s| s.backend == Kind::Cmux.as_str());
    if in_cmux && std::env::var("TOME_NOTIFY").as_deref() != Ok("off") {
        let title = format!("tome: {} #{} {}", run.workflow_name, run.id, run.status.as_str());
        Cmux.notify(&title, &message);
    }
}

/// A trigger failed to act (`what` names the workflow or project). It's
/// always logged; it also goes out as a cmux notification unless
/// `TOME_NOTIFY=off` (there's no run pane to tie it to, so this is best
/// effort when cmux isn't around).
pub fn notify_trigger(what: &str, message: &str) {
    eprintln!("tome daemon: notify: trigger for {what} failed: {message}");
    if std::env::var("TOME_NOTIFY").as_deref() != Ok("off") {
        Cmux.notify(&format!("tome: trigger for {what} failed"), message);
    }
}

/// A run that claimed an event didn't succeed, so its delivery failed.
/// Logged, and sent as a cmux notification unless `TOME_NOTIFY=off`.
pub fn notify_delivery(title: &str, message: &str) {
    eprintln!("tome daemon: notify: {title}: {message}");
    if std::env::var("TOME_NOTIFY").as_deref() != Ok("off") {
        Cmux.notify(&format!("tome: {title}"), message);
    }
}

/// The daemon's hooks for runs that end without finishing themselves.
pub struct Hooks;

impl RecoveryHooks for Hooks {
    fn kill_sessions(&self, run: &Run, sessions: &[Session]) {
        kill_sessions(run.id, sessions);
    }

    fn notify(&self, run: &Run, sessions: &[Session], cut: &[Worker]) {
        notify(run, sessions, cut);
    }
}
