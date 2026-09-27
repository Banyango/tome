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
use crate::session::{self, Launch, Tmux};
use crate::store::{Run, Session};
use crate::workflow::{self, Frontmatter};
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
pub fn harness_for(fm: &Frontmatter) -> CliResult<Harness> {
    let d = &fm.defaults;
    harness::resolve(d.orchestrator_harness.as_deref().or(d.harness.as_deref()).unwrap_or(harness::DEFAULT))
}

/// A run's orchestrator, ready to launch.
pub struct Plan {
    pub harness: Harness,
    pub session: String,
    pub title: String,
    pub cwd: PathBuf,
    pub prompt: String,
}

pub fn plan(run: &Run) -> CliResult<Plan> {
    let snapshot = run
        .workflow_snapshot
        .as_deref()
        .ok_or_else(|| CliError::internal(format!("run {} has no workflow snapshot", run.id)))?;
    let path = PathBuf::from(run.workflow_path.clone().unwrap_or_default());
    let wf = workflow::parse_snapshot(&path, snapshot).map_err(|inv| inv.into_cli_error())?;
    let cwd = run.project_path.as_ref().map(PathBuf::from).filter(|p| p.is_dir()).unwrap_or_else(paths::user_home);
    Ok(Plan {
        harness: harness_for(&wf.frontmatter)?,
        session: session::run_session_name(run.id, &run.workflow_name, ROLE),
        title: format!("tome: {} #{}", run.workflow_name, run.id),
        cwd,
        prompt: bootstrap(run, &wf.frontmatter, &wf.body),
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

/// Start the orchestrator's session. The launcher script, the bootstrap
/// prompt and the session's output all go in the run's directory.
pub fn launch(tmux: &Tmux, run: &Run, plan: &Plan) -> CliResult<()> {
    let dir = paths::runs_dir().join(run.id.to_string());
    fs::create_dir_all(&dir)?;
    let prompt_file = dir.join("orchestrator-prompt.md");
    fs::write(&prompt_file, &plan.prompt)?;
    let argv = plan.harness.command(&Vars {
        prompt: &plan.prompt,
        prompt_file: &prompt_file.to_string_lossy(),
        run_id: run.id,
        session: &plan.session,
        cwd: &plan.cwd.to_string_lossy(),
    });
    tmux.launch(&Launch {
        name: &plan.session,
        title: &plan.title,
        cwd: &plan.cwd,
        argv: &argv,
        env: &session_env(tmux, run.id),
        script: &dir.join("orchestrator.sh"),
        log: &dir.join(format!("{ROLE}.log")),
    })
}

/// What a run's agents need to call back into tome.
fn session_env(tmux: &Tmux, run_id: i64) -> Vec<(String, String)> {
    let mut env = vec![
        ("TOME_RUN_ID".to_string(), run_id.to_string()),
        ("TOME_OUTPUT".to_string(), "json".to_string()),
        ("TOME_HOME".to_string(), paths::tome_home().to_string_lossy().into_owned()),
    ];
    if let Some(socket) = &tmux.socket {
        env.push(("TOME_TMUX_SOCKET".to_string(), socket.clone()));
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

/// Kill every session of a run: the recorded ones (on the server they were
/// started on) and any other `tome-<id>-*` session on the current server.
pub fn kill_sessions(run_id: i64, recorded: &[Session]) {
    for s in recorded {
        Tmux { socket: s.socket.clone() }.kill(&s.name);
    }
    Tmux::from_env().kill_prefix(&session::run_prefix(run_id));
}

/// The daemon's hooks for runs that end without finishing themselves.
pub struct Hooks;

impl RecoveryHooks for Hooks {
    fn kill_sessions(&self, run: &Run) {
        kill_sessions(run.id, &[]);
    }

    /// Notifications are a later feature; for now the daemon log says it.
    fn notify(&self, run: &Run) {
        eprintln!(
            "tome daemon: notify: run {} ({}) {}: {}",
            run.id,
            run.workflow_name,
            run.status.as_str(),
            run.reason.as_deref().unwrap_or("")
        );
    }
}
