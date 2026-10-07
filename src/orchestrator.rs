//! Launching a run's orchestrator: an agent CLI, started through the harness
//! adapter in its own tmux session, that reads the workflow and drives the
//! run by calling tome commands.
//!
//! A `single` run (the default mode) starts an agent in the `agent` role
//! instead, which carries out the workflow itself and can't delegate. Both
//! are a run's main session and launch the same way.
//!
//! Everything is taken from the run's workflow snapshot, so a queued run
//! launches later exactly as it was when it was requested.

use crate::harness::{self, Harness, Vars};
use crate::output::{CliError, CliResult};
use crate::paths;
use crate::placement::{self, From, Inputs, Placement, Role, Settings, Workspace};
use crate::recovery::RecoveryHooks;
use crate::session::{self, Anchor, Backend, Cmux, Kind, Launch, Layout, Split, Target, Tmux};
use crate::store::{Run, Session, Worker};
use crate::workflow::{self, Frontmatter, Mode, Workflow};
use std::fs;
use std::path::{Path, PathBuf};

/// The built-in orchestrator prompt: its duties and the tome command reference.
pub const PROMPT: &str = include_str!("orchestrator_prompt.md");
/// The built-in prompt for a single run's agent.
pub const AGENT_PROMPT: &str = include_str!("agent_prompt.md");

pub const ROLE: &str = "orchestrator";
/// A single run's agent.
pub const AGENT_ROLE: &str = "agent";

/// Why a run failed when its orchestrator went away before `tome run finish`.
pub const EXITED: &str = "orchestrator_exited";
/// Why a single run failed when its agent went away before `tome run finish`.
pub const AGENT_EXITED: &str = "agent_exited";
pub const BACKEND_UNAVAILABLE: &str = "backend_unavailable";

/// The role of a run's main session in `mode`.
pub fn role(mode: Mode) -> &'static str {
    match mode {
        Mode::Single => AGENT_ROLE,
        Mode::Orchestrated => ROLE,
    }
}

/// Whether `role` is a run's main session: its orchestrator or its agent.
pub fn is_main(role: &str) -> bool {
    role == ROLE || role == AGENT_ROLE
}

/// What refuses delegation in a single run.
pub fn single_refusal(what: &str) -> CliError {
    CliError::invalid(format!(
        "{what} isn't available: this run is a single-agent run (`mode: single`), so its agent does the work itself"
    ))
    .with_hint("set `mode: orchestrated` in the workflow's frontmatter")
}
/// Why a run failed when its orchestrator couldn't be started.
pub const LAUNCH_FAILED: &str = "launch_failed";

/// The harness a run's main session runs in: for an orchestrator,
/// `defaults.orchestrator_harness`, else `defaults.harness`; for a single
/// run's agent, `defaults.harness`; else `claude`.
pub fn harness_for(fm: &Frontmatter, mode: Mode, project: Option<&Path>) -> CliResult<Harness> {
    let d = &fm.defaults;
    let own = match mode {
        Mode::Orchestrated => d.orchestrator_harness.as_deref(),
        Mode::Single => None,
    };
    harness::resolve(
        own.or(d.harness.as_deref()).unwrap_or(harness::DEFAULT),
        project,
    )
}

/// The model a run's main session runs: for an orchestrator,
/// `defaults.orchestrator_model`, else `defaults.model`; for a single run's
/// agent, `defaults.model`; else the harness's own default.
pub fn model_for(fm: &Frontmatter, mode: Mode) -> Option<&str> {
    let d = &fm.defaults;
    let own = match mode {
        Mode::Orchestrated => d.orchestrator_model.as_deref(),
        Mode::Single => None,
    };
    own.or(d.model.as_deref())
}

/// Where a run's main session goes.
pub fn placement(
    fm: &Frontmatter,
    mode: Mode,
    flags: Option<&Settings>,
    project: Option<&Path>,
) -> CliResult<Placement> {
    let inputs = Inputs {
        role: match mode {
            Mode::Single => Role::Agent,
            Mode::Orchestrated => Role::Orchestrator,
        },
        flags,
        run_flags: None,
        spec: fm.defaults.layout.as_ref(),
    };
    placement::resolve(&inputs, project)
}

/// Where a session placed as `placement` goes: the workspace it names,
/// with `focused` the one recorded when the run started. When that couldn't
/// be told, it's the project workspace, and the note to record on the run
/// says why.
pub fn target(run: &Run, placement: &Placement) -> (Target, Option<String>) {
    match &placement.workspace {
        Workspace::Named(name) => (Target::Named(name.clone()), None),
        Workspace::Focused => {
            let focused = run.placement.as_ref().map(|p| &p["focused"]);
            match focused.and_then(|f| f["id"].as_str()) {
                Some(id) => (Target::Focused(id.to_string()), None),
                None => {
                    let why = focused
                        .and_then(|f| f["unknown"].as_str())
                        .unwrap_or("it wasn't recorded");
                    (
                        Target::Project,
                        Some(format!(
                            "workspace: focused: {why}; used the project workspace"
                        )),
                    )
                }
            }
        }
        Workspace::Project | Workspace::Own => (Target::Project, None),
    }
}

/// Where `from: caller` opens a session on `backend`: next to the cmux
/// surface `caller` recorded (`{surface}`, else `{unknown: why}`), found
/// where it is now. Returns the anchor and its pane, or why it can't.
pub fn caller_anchor(
    backend: Kind,
    caller: Option<&serde_json::Value>,
) -> Result<(Anchor, String), String> {
    if backend == Kind::Tmux {
        return Err("`from: caller` needs cmux or herdr, and this session is on tmux".into());
    }
    let caller = caller.ok_or("no caller was recorded")?;
    if backend == Kind::Herdr {
        let pane = caller["herdr_pane"].as_str().ok_or_else(|| {
            caller["unknown"]
                .as_str()
                .unwrap_or("no herdr caller was recorded")
                .to_string()
        })?;
        return session::herdr_caller_anchor(pane);
    }
    let surface = caller["surface"].as_str().ok_or_else(|| {
        caller["unknown"]
            .as_str()
            .unwrap_or("no caller was recorded")
            .to_string()
    })?;
    session::caller_anchor(surface)
}

/// Open next to the caller (`found`) if `placement` says `from: caller`: a
/// tab in its pane or a split off it, in its workspace, which wins over
/// `workspace` (so an own workspace becomes a tab). Returns the layout to
/// open with. When the caller can't be used, the session goes where it
/// would without `from`, and a warning says why.
pub fn use_caller(
    placement: &mut Placement,
    found: Result<(Anchor, String), String>,
    split: &mut Split,
    target: &mut Target,
    warnings: &mut Vec<String>,
) -> Layout {
    if placement.from != Some(From::Caller) {
        return placement.layout;
    }
    match found {
        Ok((anchor, pane)) => {
            placement.caller = Some(format!(
                "pane {pane} (surface {}) in workspace {}",
                anchor.pane, anchor.handle
            ));
            *target = Target::Caller(anchor.handle.clone());
            split.anchors = vec![anchor];
            if placement.layout == Layout::Workspace {
                placement.layout = Layout::Tab;
                placement.sources.insert(
                    "layout".into(),
                    "`from: caller` (its workspace wins over `workspace: own`)".into(),
                );
            }
        }
        Err(why) => {
            warnings.push(format!(
                "from: caller: {why}; opened where it would go without `from`"
            ));
            split.from = None;
        }
    }
    placement.layout
}

/// The placement flags `tome run` was given for this run, if any.
pub fn run_flags(run: &Run) -> CliResult<Option<Settings>> {
    run.placement
        .as_ref()
        .and_then(|p| p.get("flags"))
        .map(Settings::from_json)
        .transpose()
}

/// A run's main session, ready to launch.
pub struct Plan {
    /// `orchestrator` or `agent`.
    pub role: &'static str,
    pub harness: Harness,
    pub model: Option<String>,
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
    run.project_path
        .as_ref()
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
}

pub fn plan(run: &Run) -> CliResult<Plan> {
    let wf = snapshot(run)?;
    let cwd = run_cwd(run);
    let project = run_project(run);
    let project = project.as_deref();
    // The mode stored on the run, not the snapshot's: runs from before
    // modes were orchestrated.
    let mode = run.mode;
    let harness = harness_for(&wf.frontmatter(), mode, project)?;
    let model = model_for(&wf.frontmatter(), mode).map(str::to_string);
    harness.check_model(model.as_deref())?;
    let role = role(mode);
    Ok(Plan {
        role,
        harness,
        model,
        backend: Kind::choose_with_caller(
            wf.frontmatter().defaults.backend.as_deref(),
            project,
            run.placement
                .as_ref()
                .is_some_and(|p| p["caller"]["herdr_pane"].is_string()),
        )?,
        placement: placement(&wf.frontmatter(), mode, run_flags(run)?.as_ref(), project)?,
        session: session::run_session_name(run.id, &run.workflow_name, role),
        title: format!("tome: {} #{}", run.workflow_name, run.id),
        cwd,
        prompt: bootstrap(run, &wf.frontmatter(), &wf.body()),
        start_timeout: crate::handshake::timeout(&wf.frontmatter()),
    })
}

/// The main session's first message: the built-in prompt for the run's
/// mode, the resolved workflow and any extra instructions the workflow gives
/// it.
pub fn bootstrap(run: &Run, fm: &Frontmatter, body: &str) -> String {
    let prompt = match run.mode {
        Mode::Single => AGENT_PROMPT,
        Mode::Orchestrated => PROMPT,
    };
    let mut out = String::from(prompt.trim_end());
    out.push_str(&format!(
        "\n\n## This run\n\nRun #{} of workflow `{}`",
        run.id, run.workflow_name
    ));
    if let Some(desc) = &fm.description {
        out.push_str(&format!(" ({desc})"));
    }
    out.push_str(".\n");
    if let Some(params) = run.params.as_object().filter(|p| !p.is_empty()) {
        out.push_str("Parameters:\n");
        for (k, v) in params {
            let v = v
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| v.to_string());
            out.push_str(&format!("- {k} = {v}\n"));
        }
    }
    if let Some(extra) = fm
        .orchestrator
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        out.push_str(&format!("\n## Instructions for this workflow\n\n{extra}\n"));
    }
    out.push_str(&format!("\n## The workflow\n\n{}\n", body.trim()));
    out
}

/// Where the prompt of a run's main session (`role`) is written.
pub fn prompt_file(run_id: i64, role: &str) -> PathBuf {
    paths::runs_dir()
        .join(run_id.to_string())
        .join(format!("{role}-prompt.md"))
}

/// Start the main session and return its record. The launcher
/// script, the bootstrap prompt and the session's output all go in the
/// run's directory. Also returns a note to record on the run, if any.
pub fn launch(
    run: &Run,
    plan: &Plan,
    recorded: &[Session],
) -> CliResult<(Session, Option<String>)> {
    let backend = Backend::new(plan.backend);
    let dir = paths::runs_dir().join(run.id.to_string());
    fs::create_dir_all(&dir)?;
    let prompt_file = prompt_file(run.id, plan.role);
    fs::write(&prompt_file, &plan.prompt)?;
    let argv = plan.harness.command(&Vars {
        prompt: &plan.prompt,
        prompt_file: &prompt_file.to_string_lossy(),
        run_id: run.id,
        session: &plan.session,
        cwd: &plan.cwd.to_string_lossy(),
        model: plan.model.as_deref(),
    });
    // An orchestrator relaunched after a restart may open from what's left.
    let mut placement = plan.placement.clone();
    let p = &placement;
    let mut split = Split::new(p.direction, p.size, p.from, recorded);
    let (mut target, note) = target(run, p);
    let mut warnings = Vec::new();
    let found = match p.from {
        Some(From::Caller) => {
            caller_anchor(plan.backend, run.placement.as_ref().map(|r| &r["caller"]))
        }
        _ => Err(String::new()),
    };
    let layout = use_caller(
        &mut placement,
        found,
        &mut split,
        &mut target,
        &mut warnings,
    );
    let (session, more) = backend.launch(&Launch {
        name: &plan.session,
        title: &plan.title,
        cwd: &plan.cwd,
        argv: &argv,
        env: &session_env(run.id),
        script: &dir.join(format!("{}.sh", plan.role)),
        log: &dir.join(format!("{}.log", plan.role)),
        layout,
        split: &split,
        target: &target,
        project: run_project(run).as_deref(),
    })?;
    warnings.extend(more);
    placement.warnings = warnings;
    let session = Session {
        run_id: run.id,
        role: plan.role.to_string(),
        layout: Some(layout.as_str().to_string()),
        harness: Some(plan.harness.name.clone()),
        placement: Some(placement.to_json()),
        ..session
    };
    Ok((session, note))
}

/// What a run's agents need to call back into tome.
pub fn session_env(run_id: i64) -> Vec<(String, String)> {
    let mut env = vec![
        ("TOME_RUN_ID".to_string(), run_id.to_string()),
        ("TOME_OUTPUT".to_string(), "json".to_string()),
        (
            "TOME_HOME".to_string(),
            paths::tome_home().to_string_lossy().into_owned(),
        ),
    ];
    // So that the agent's tome commands reach the same daemon and servers.
    for key in ["TOME_TMUX_SOCKET", "TOME_BACKEND", "TOME_LAYOUT"] {
        if let Some(v) = std::env::var(key).ok().filter(|v| !v.is_empty()) {
            env.push((key.to_string(), v));
        }
    }
    // Make sure `tome` resolves to this tome.
    let path = std::env::var("PATH").unwrap_or_default();
    let path = match std::env::current_exe()
        .ok()
        .as_deref()
        .and_then(Path::parent)
    {
        Some(bin) if !path.split(':').any(|p| Path::new(p) == bin) => {
            format!("{}:{path}", bin.display())
        }
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
    eprintln!(
        "tome daemon: notify: run {} ({}) {}: {message}",
        run.id,
        run.workflow_name,
        run.status.as_str()
    );
    let backend = sessions.iter().find_map(|s| Kind::parse(&s.backend));
    if let Some(backend) = backend.filter(|k| *k != Kind::Tmux) {
        if std::env::var("TOME_NOTIFY").as_deref() == Ok("off") {
            return;
        }
        let title = format!(
            "tome: {} #{} {}",
            run.workflow_name,
            run.id,
            run.status.as_str()
        );
        session::notify(backend, &title, &message);
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

pub fn notify_blocked(run: &Run, who: &str) {
    let message = format!("run {}: {who} is waiting for input", run.id);
    eprintln!("tome daemon: notify: {message}");
    if std::env::var("TOME_NOTIFY").as_deref() != Ok("off") {
        session::notify(
            Kind::Herdr,
            &format!("tome: {} #{} blocked", run.workflow_name, run.id),
            &message,
        );
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
