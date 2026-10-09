//! Sessions: named, visible terminals that agents run in, on one of two
//! backends:
//!
//! - **tmux**: a detached session the user can `tmux attach -t <name>` to.
//!   tome uses the default server, or the one named by `TOME_TMUX_SOCKET`
//!   (passed as `tmux -L`), which tests use to stay isolated.
//! - **cmux**: a workspace in the cmux app the daemon runs under. It needs
//!   the `CMUX_*` environment of a terminal inside cmux (cmux only lets its
//!   own processes in), which a daemon started from one inherits.
//!
//! A run's backend is the workflow's `defaults.backend`, else `TOME_BACKEND`,
//! else `backend:` in the project's `.tome/config.yaml` or the global
//! `~/.tome/config.yaml`, else cmux when the daemon runs
//! inside cmux and tmux otherwise.
//!
//! Where in that backend each session goes is its placement (see
//! [`crate::placement`]); [`Layout`] is the part launches act on.
//!
//! Either way the session's command is a launcher script that sets the
//! agent's environment, captures its output to a log and `exec`s it, so the
//! session ends when the agent does.

mod herdr;
mod moves;
mod split;
mod workspace;

use crate::ids::RunId;
use herdr::Herdr;
pub use moves::{move_to, Move};
pub use split::{Anchor, Split};

use crate::config::Config;
use crate::harness::shell_quote;
use crate::output::{CliError, CliResult};
use crate::paths;
use crate::store::Session;
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::Duration;

/// How long a launched command waits for its output to be captured before
/// starting anyway (tmux attaches capture after the session starts).
const CAPTURE_WAIT: Duration = Duration::from_secs(5);
/// How long to wait for a new cmux workspace to show up in `tree`.
const CMUX_TREE_WAIT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Tmux,
    Cmux,
    Herdr,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Tmux => "tmux",
            Kind::Cmux => "cmux",
            Kind::Herdr => "herdr",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        match s {
            "tmux" => Some(Kind::Tmux),
            "cmux" => Some(Kind::Cmux),
            "herdr" => Some(Kind::Herdr),
            _ => None,
        }
    }

    /// The backend for a run whose workflow asks for `requested` (see the
    /// module docs for the order).
    pub fn choose(requested: Option<&str>, project: Option<&Path>) -> CliResult<Kind> {
        Self::choose_with_caller(requested, project, false)
    }

    pub fn choose_with_caller(
        requested: Option<&str>,
        project: Option<&Path>,
        herdr_caller: bool,
    ) -> CliResult<Kind> {
        let env = std::env::var("TOME_BACKEND").ok().filter(|s| !s.is_empty());
        let (name, source) = match (requested, env) {
            (Some(r), _) => (
                r.to_string(),
                "the workflow's `defaults.backend`".to_string(),
            ),
            (None, Some(e)) => (e, "TOME_BACKEND".to_string()),
            (None, None) => match Config::load(project)?.str("backend")? {
                Some((c, file)) => (c, format!("`backend` in {}", file.label())),
                None => {
                    return Ok(if Cmux::inside() {
                        Kind::Cmux
                    } else if herdr_inside() || herdr_caller {
                        Kind::Herdr
                    } else {
                        Kind::Tmux
                    })
                }
            },
        };
        Kind::parse(&name).ok_or_else(|| {
            CliError::invalid(format!("unknown session backend `{name}` (from {source})"))
                .with_hint("use tmux, cmux or herdr")
        })
    }
}

/// Where a run's sessions go in their backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// A tab in one split of the project's tome workspace (tmux: a window
    /// in its session).
    Tab,
    /// Its own split in the project's tome workspace (tmux: a pane in its
    /// session).
    Split,
    /// Its own workspace (tmux: its own session).
    Workspace,
}

impl Layout {
    pub fn as_str(self) -> &'static str {
        match self {
            Layout::Tab => "tab",
            Layout::Split => "split",
            Layout::Workspace => "workspace",
        }
    }

    pub fn parse(s: &str) -> Option<Layout> {
        match s {
            "tab" => Some(Layout::Tab),
            "split" => Some(Layout::Split),
            "workspace" => Some(Layout::Workspace),
            _ => None,
        }
    }

    /// A recorded session's layout; sessions from before layouts had their
    /// own workspace.
    pub fn of(s: &Session) -> Layout {
        s.layout
            .as_deref()
            .and_then(Layout::parse)
            .unwrap_or(Layout::Workspace)
    }
}

/// A backend to start sessions on.
pub enum Backend {
    Tmux(Tmux),
    Cmux(Cmux),
    Herdr(herdr::Herdr),
}

impl Backend {
    pub fn new(kind: Kind) -> Backend {
        match kind {
            Kind::Tmux => Backend::Tmux(Tmux::from_env()),
            Kind::Cmux => Backend::Cmux(Cmux),
            Kind::Herdr => Backend::Herdr(herdr::Herdr::from_env()),
        }
    }

    pub fn kind(&self) -> Kind {
        match self {
            Backend::Tmux(_) => Kind::Tmux,
            Backend::Cmux(_) => Kind::Cmux,
            Backend::Herdr(_) => Kind::Herdr,
        }
    }

    /// Start `launch` and return how to find it again (the session record
    /// minus the run, role, layout, harness and placement, which the caller
    /// fills in) and any placement warnings.
    pub fn launch(&self, launch: &Launch) -> CliResult<(Session, Vec<String>)> {
        let mut warnings = Vec::new();
        let home = launch
            .project
            .map(Path::to_path_buf)
            .unwrap_or_else(paths::user_home);
        let (socket, handle, pane) = match (self, launch.layout) {
            (Backend::Tmux(t), Layout::Workspace) => (t.socket.clone(), None, t.launch(launch)?),
            (Backend::Cmux(c), Layout::Workspace) => {
                let (workspace, surface) = c.launch(launch)?;
                (None, Some(workspace), surface)
            }
            (Backend::Tmux(t), layout) => {
                let _held = workspace::lock();
                let places = workspace::Places::open();
                let id =
                    t.workspace(&places, launch.target, launch.project, &home, &mut warnings)?;
                let pane = t.launch_in(&id, layout, launch, &mut warnings)?;
                (t.socket.clone(), Some(id), pane)
            }
            (Backend::Cmux(c), layout) => {
                let _held = workspace::lock();
                let places = workspace::Places::open();
                let (record, kept) =
                    c.workspace(&places, launch.target, launch.project, &home, &mut warnings)?;
                let surface = c.launch_in(
                    kept.then_some(&places),
                    record.clone(),
                    layout,
                    launch,
                    &mut warnings,
                )?;
                (None, Some(record.id), surface)
            }
            (Backend::Herdr(h), _layout) => {
                let (workspace, pane) = h.launch(launch, &mut warnings)?;
                (
                    Some(h.socket.to_string_lossy().into_owned()),
                    Some(workspace),
                    pane,
                )
            }
        };
        let session = Session {
            run_id: RunId::new(0),
            name: launch.name.to_string(),
            role: String::new(),
            backend: self.kind().as_str().to_string(),
            socket,
            handle,
            pane: Some(pane),
            layout: None,
            harness: None,
            placement: None,
            agent_status: None,
            blocked_at: None,
            created_at: String::new(),
        };
        Ok((session, warnings))
    }
}

/// Which workspace a `tab` or `split` session goes in.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Target {
    /// The project's `<project>-orchestrator`.
    #[default]
    Project,
    /// `<project>-<name>`.
    Named(String),
    /// A workspace (tmux session) of the user's, by id: the one focused
    /// when the run started.
    Focused(String),
    /// The cmux workspace of the pane that ran `tome run` (`from: caller`).
    Caller(String),
}

impl Target {
    /// The tome workspace's name, for [`workspace::Places`]; `None` for the
    /// project's (and a focused one, which isn't tome's).
    fn name(&self) -> Option<&str> {
        match self {
            Target::Named(n) => Some(n),
            _ => None,
        }
    }

    /// How messages name it.
    fn label(&self, project: Option<&Path>) -> String {
        match self {
            Target::Focused(id) => format!("the focused workspace ({id})"),
            Target::Caller(id) => format!("the caller's workspace ({id})"),
            t => workspace::name(project, t.name()),
        }
    }
}

/// Note that the focused workspace `target` names is gone, so the session
/// goes in the project's instead.
fn gone(target: &Target, warnings: &mut Vec<String>) {
    match target {
        Target::Focused(id) => warnings.push(format!(
            "workspace: focused: {id} is gone; opened in the project workspace"
        )),
        Target::Caller(id) => warnings.push(format!(
            "from: caller: its workspace {id} is gone; opened in the project workspace"
        )),
        _ => {}
    }
}

/// The workspace (cmux) or tmux session the user has focused on `kind`,
/// or why that can't be told.
pub fn focused(kind: Kind) -> Result<String, String> {
    match kind {
        Kind::Tmux => {
            let tmux = Tmux::from_env();
            let out = tmux
                .run(&["list-clients", "-F", "#{client_activity} #{session_id}"])
                .ok()
                .filter(|o| o.status.success())
                .ok_or("no tmux client is attached")?;
            let text = String::from_utf8_lossy(&out.stdout).into_owned();
            text.lines()
                .filter_map(|l| l.split_once(' '))
                .filter_map(|(at, id)| Some((at.parse::<u64>().ok()?, id.to_string())))
                .max_by_key(|(at, _)| *at)
                .map(|(_, id)| id)
                .ok_or_else(|| "no tmux client is attached".to_string())
        }
        Kind::Cmux => {
            let cmux = Cmux;
            if !Cmux::inside() || !cmux.available() {
                return Err("cmux isn't answering".into());
            }
            let out = cmux
                .run(&["--id-format", "uuids", "identify", "--json"])
                .ok()
                .filter(|o| o.status.success());
            let v: Value = out
                .and_then(|o| serde_json::from_slice(&o.stdout).ok())
                .unwrap_or_default();
            v["focused"]["workspace_id"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| "cmux has no focused workspace".into())
        }
        Kind::Herdr => Herdr::focused(),
    }
}

/// The cmux pane this process runs in, from its environment:
/// `{surface, workspace}`; `None` outside cmux.
pub fn caller_env() -> Option<Value> {
    let var = |k| {
        std::env::var(k)
            .ok()
            .filter(|v: &String| !v.trim().is_empty())
    };
    if let Some(pane) = var("HERDR_PANE_ID") {
        return Some(
            serde_json::json!({"herdr_pane": pane, "herdr_workspace": var("HERDR_WORKSPACE_ID")}),
        );
    }
    let surface = var("CMUX_SURFACE_ID")?;
    Some(serde_json::json!({ "surface": surface, "workspace": var("CMUX_WORKSPACE_ID") }))
}

/// Where the cmux surface `surface` is now, as an anchor to open from (its
/// workspace and the surface), or why it can't be used.
pub fn caller_anchor(surface: &str) -> Result<(split::Anchor, String), String> {
    let all = Cmux.surfaces().ok_or("cmux isn't answering")?;
    let s = all
        .into_iter()
        .find(|s| s.id == surface)
        .ok_or("the caller's pane is gone")?;
    let pane = s.pane.clone().unwrap_or_default();
    Ok((
        split::Anchor {
            handle: s.workspace,
            pane: surface.to_string(),
        },
        pane,
    ))
}

pub fn herdr_caller_anchor(pane: &str) -> Result<(split::Anchor, String), String> {
    Herdr::caller_anchor(pane)
}

pub fn notify(kind: Kind, title: &str, body: &str) {
    match kind {
        Kind::Cmux => {
            Cmux.notify(title, body);
        }
        Kind::Herdr => Herdr::notify(title, body),
        Kind::Tmux => {}
    }
}

pub fn agent_status(s: &Session) -> Option<String> {
    if Kind::parse(&s.backend) != Some(Kind::Herdr) {
        return None;
    }
    Herdr::from_socket(s.socket.as_deref())
        .ok()?
        .agent_status(s.pane.as_deref()?)
}

// Recorded sessions are found by their own pane or tab (`pane`: a tmux pane
// id, a cmux surface id). Sessions from before that are found by their tmux
// session name or cmux workspace id instead.

/// Whether a recorded session's command is still running; `None` if that
/// can't be told right now (e.g. cmux isn't answering).
pub fn is_alive(s: &Session) -> Option<bool> {
    match Kind::parse(&s.backend) {
        Some(Kind::Tmux) => {
            let tmux = Tmux {
                socket: s.socket.clone(),
            };
            Some(match &s.pane {
                Some(pane) => tmux.pane_alive(pane),
                None => tmux.is_alive(&s.name),
            })
        }
        Some(Kind::Cmux) => match (&s.pane, &s.handle) {
            (Some(surface), _) => Cmux.surface_alive(surface),
            (None, Some(id)) => Cmux.is_alive(id),
            (None, None) => Some(false),
        },
        Some(Kind::Herdr) => Herdr::from_socket(s.socket.as_deref())
            .ok()?
            .alive(s.pane.as_deref()?),
        None => Some(false),
    }
}

/// Kill a recorded session: its tab or pane, or under the `workspace`
/// layout the whole workspace (tmux session) it has to itself. Returns
/// whether it existed.
pub fn kill(s: &Session) -> bool {
    let layout = Layout::of(s);
    match Kind::parse(&s.backend) {
        Some(Kind::Tmux) => {
            let tmux = Tmux {
                socket: s.socket.clone(),
            };
            match (&s.pane, layout) {
                (Some(pane), Layout::Tab) => tmux.kill_window_of(pane),
                (Some(pane), Layout::Split) => tmux.kill_pane(pane),
                _ => tmux.kill(&s.name),
            }
        }
        Some(Kind::Cmux) => match (&s.handle, &s.pane, layout) {
            (Some(workspace), Some(surface), Layout::Tab | Layout::Split) => {
                Cmux.close_surface(workspace, surface)
            }
            (Some(workspace), _, _) => Cmux.kill(workspace),
            (None, _, _) => false,
        },
        Some(Kind::Herdr) => s
            .pane
            .as_deref()
            .is_some_and(|p| Herdr::from_socket(s.socket.as_deref()).is_ok_and(|h| h.close(p))),
        None => false,
    }
}

/// Type a line into a recorded session, as if the user had typed it and
/// pressed Enter. Returns whether it was delivered.
pub fn send_line(s: &Session, text: &str) -> bool {
    match Kind::parse(&s.backend) {
        Some(Kind::Tmux) => {
            let tmux = Tmux {
                socket: s.socket.clone(),
            };
            match &s.pane {
                Some(pane) => tmux.pane_alive(pane) && tmux.send_keys(pane, text),
                None => tmux.send_line(&s.name, text),
            }
        }
        Some(Kind::Cmux) => s
            .handle
            .as_deref()
            .is_some_and(|id| Cmux.send_line(id, s.pane.as_deref(), text)),
        Some(Kind::Herdr) => s.pane.as_deref().is_some_and(|p| {
            Herdr::from_socket(s.socket.as_deref()).is_ok_and(|h| h.send_line(p, text))
        }),
        None => false,
    }
}

/// How to attach to (or show) a recorded session: select its workspace
/// (tmux session), then its tab or pane.
pub fn attach_command(s: &Session) -> String {
    let own = Layout::of(s) == Layout::Workspace;
    match (Kind::parse(&s.backend), &s.handle, &s.pane) {
        (Some(Kind::Cmux), Some(id), Some(surface)) if !own => {
            format!("cmux select-workspace --workspace {id} && cmux focus-panel --panel {surface} --workspace {id}")
        }
        (Some(Kind::Cmux), Some(id), _) => format!("cmux select-workspace --workspace {id}"),
        (Some(Kind::Herdr), Some(id), _) => format!(
            "HERDR_SOCKET_PATH={} herdr workspace focus {}",
            shell_quote(&s.socket.clone().unwrap_or_default()),
            shell_quote(id)
        ),
        _ => {
            let tmux = match &s.socket {
                Some(sock) => format!("tmux -L {sock}"),
                None => "tmux".to_string(),
            };
            match (&s.handle, &s.pane) {
                (Some(session), Some(pane)) if !own => format!(
                    "{tmux} select-window -t {pane} \\; select-pane -t {pane} \\; attach -t {}",
                    shell_quote(session)
                ),
                _ => format!("{tmux} attach -t {}", s.name),
            }
        }
    }
}

fn herdr_inside() -> bool {
    std::env::var("HERDR_ENV").is_ok_and(|v| v == "1") || std::env::var("HERDR_PANE_ID").is_ok()
}

/// What to run in a new session.
pub struct Launch<'a> {
    pub name: &'a str,
    /// Window (tmux) or workspace (cmux) title, e.g. `tome: build #42`.
    pub title: &'a str,
    pub cwd: &'a Path,
    pub argv: &'a [String],
    pub env: &'a [(String, String)],
    /// Where to write the launcher script.
    pub script: &'a Path,
    /// Where to append the session's output.
    pub log: &'a Path,
    pub layout: Layout,
    /// How a `split` (or cmux's tab split) opens.
    pub split: &'a Split,
    /// Which workspace the `tab` and `split` layouts use.
    pub target: &'a Target,
    /// The run's project, whose tome workspaces the `tab` and `split`
    /// layouts use; `None` for runs outside a project.
    pub project: Option<&'a Path>,
}

// --- tmux ------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct Tmux {
    /// `tmux -L <socket>`; `None` is the default server.
    pub socket: Option<String>,
}

impl Tmux {
    pub fn from_env() -> Tmux {
        Tmux {
            socket: std::env::var("TOME_TMUX_SOCKET")
                .ok()
                .filter(|s| !s.is_empty()),
        }
    }

    fn cmd(&self) -> Command {
        let mut cmd = Command::new("tmux");
        if let Some(socket) = &self.socket {
            cmd.args(["-L", socket]);
        }
        // Inside tmux, `$TMUX` would point commands at the caller's server.
        cmd.env_remove("TMUX").stdin(Stdio::null());
        cmd
    }

    fn run(&self, args: &[&str]) -> std::io::Result<Output> {
        self.cmd().args(args).output()
    }

    /// Whether tmux is installed.
    pub fn available() -> bool {
        Command::new("tmux")
            .arg("-V")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    /// Start `launch.argv` in a new detached session with `launch.env` set,
    /// capturing its output to `launch.log`. Returns its pane's id.
    pub fn launch(&self, launch: &Launch) -> CliResult<String> {
        if !Tmux::available() {
            return Err(CliError::internal("tmux isn't installed")
                .with_hint("install tmux, or use `backend: cmux`"));
        }
        if self.is_alive(launch.name) {
            return Err(CliError::internal(format!(
                "tmux session `{}` already exists",
                launch.name
            )));
        }
        let ready = launch.script.with_extension("ready");
        let _ = fs::remove_file(&ready);
        fs::write(launch.script, script(launch, Capture::AfterStart(&ready)))?;

        let cwd = launch.cwd.to_string_lossy();
        let script = launch.script.to_string_lossy();
        let out = self.run(&[
            "new-session",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "-s",
            launch.name,
            "-n",
            launch.title,
            "-c",
            &cwd,
            "-x",
            "200",
            "-y",
            "50",
            "sh",
            &script,
        ])?;
        if !out.status.success() {
            return Err(CliError::internal(format!(
                "tmux couldn't start session `{}`: {}",
                launch.name,
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        let pane = String::from_utf8_lossy(&out.stdout).trim().to_string();
        // Keep the title; agents like to set their own.
        let _ = self.run(&["set-option", "-w", "-t", &pane, "automatic-rename", "off"]);
        let _ = self.run(&["set-option", "-w", "-t", &pane, "allow-rename", "off"]);
        let pipe = format!("cat >> {}", shell_quote(&launch.log.to_string_lossy()));
        let _ = self.run(&["pipe-pane", "-t", &pane, "-o", &pipe]);
        // Let the command start now that its output is being captured.
        fs::write(&ready, "")?;
        Ok(pane)
    }

    /// Start `launch.argv` in the tome session `id`: in a new window under
    /// the `tab` layout (`from` doesn't apply to tmux windows), in a new pane
    /// as `launch.split` says under `split`. Returns its pane's id.
    pub fn launch_in(
        &self,
        id: &str,
        layout: Layout,
        launch: &Launch,
        warnings: &mut Vec<String>,
    ) -> CliResult<String> {
        let ready = launch.script.with_extension("ready");
        let _ = fs::remove_file(&ready);
        fs::write(launch.script, script(launch, Capture::AfterStart(&ready)))?;

        let cwd = launch.cwd.to_string_lossy();
        let script = launch.script.to_string_lossy();
        let target = format!("{id}:");
        let mut anchored = false;
        let out = match layout {
            Layout::Split => {
                let (out, from_anchor) = self.split_window(
                    id,
                    launch.split,
                    split::Opening::New {
                        cwd: &cwd,
                        argv: &["sh", &script],
                    },
                    warnings,
                )?;
                anchored = from_anchor;
                out
            }
            _ => self.run(&[
                "new-window",
                "-d",
                "-t",
                &target,
                "-n",
                launch.title,
                "-c",
                &cwd,
                "-P",
                "-F",
                "#{pane_id}",
                "sh",
                &script,
            ])?,
        };
        if !out.status.success() {
            return Err(CliError::internal(format!(
                "tmux couldn't start `{}` in {}: {}",
                launch.name,
                launch.target.label(launch.project),
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        let pane = String::from_utf8_lossy(&out.stdout).trim().to_string();
        match layout {
            Layout::Split => {
                self.even_out(&pane, launch.split, anchored);
                let _ = self.run(&["select-pane", "-t", &pane, "-T", launch.title]);
            }
            _ => {
                let _ = self.run(&["set-option", "-w", "-t", &pane, "automatic-rename", "off"]);
                let _ = self.run(&["set-option", "-w", "-t", &pane, "allow-rename", "off"]);
            }
        }
        let pipe = format!("cat >> {}", shell_quote(&launch.log.to_string_lossy()));
        let _ = self.run(&["pipe-pane", "-t", &pane, "-o", &pipe]);
        fs::write(&ready, "")?;
        Ok(pane)
    }

    /// The session `target` names: the focused one if it's still there,
    /// else the tome session, made if needed (`places` must be locked).
    fn workspace(
        &self,
        places: &workspace::Places,
        target: &Target,
        project: Option<&Path>,
        home: &Path,
        warnings: &mut Vec<String>,
    ) -> CliResult<String> {
        match target {
            Target::Focused(id) if self.session_exists(id) => Ok(id.clone()),
            target => {
                gone(target, warnings);
                places.tmux(self, project, target.name(), home)
            }
        }
    }

    /// Whether the session (by id, `$<n>`) exists.
    fn session_exists(&self, id: &str) -> bool {
        match self.run(&["list-sessions", "-F", "#{session_id}"]) {
            Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
                .lines()
                .any(|l| l.trim() == id),
            _ => false,
        }
    }

    /// Whether the pane (`%<n>`) exists and its command is still running.
    pub fn pane_alive(&self, pane: &str) -> bool {
        match self.run(&["list-panes", "-t", pane, "-F", "#{pane_id} #{pane_dead}"]) {
            Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
                .lines()
                .any(|l| l.trim() == format!("{pane} 0")),
            _ => false,
        }
    }

    /// Kill a pane. Returns whether it existed.
    pub fn kill_pane(&self, pane: &str) -> bool {
        self.pane_exists(pane)
            && self
                .run(&["kill-pane", "-t", pane])
                .is_ok_and(|o| o.status.success())
    }

    /// Kill the window a pane is in. Returns whether it existed.
    pub fn kill_window_of(&self, pane: &str) -> bool {
        self.pane_exists(pane)
            && self
                .run(&["kill-window", "-t", pane])
                .is_ok_and(|o| o.status.success())
    }

    /// Whether the pane exists, running or not. (A missing `-t` target
    /// would otherwise fall back to the current pane.)
    fn pane_exists(&self, pane: &str) -> bool {
        match self.run(&["list-panes", "-t", pane, "-F", "#{pane_id}"]) {
            Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
                .lines()
                .any(|l| l.trim() == pane),
            _ => false,
        }
    }

    /// Type `text` into a pane (or other target), then Enter.
    fn send_keys(&self, target: &str, text: &str) -> bool {
        self.run(&["send-keys", "-t", target, "-l", text])
            .is_ok_and(|o| o.status.success())
            && self
                .run(&["send-keys", "-t", target, "Enter"])
                .is_ok_and(|o| o.status.success())
    }

    /// Whether the session exists and its command is still running.
    pub fn is_alive(&self, name: &str) -> bool {
        match self.run(&[
            "list-panes",
            "-s",
            "-t",
            &format!("={name}"),
            "-F",
            "#{pane_dead}",
        ]) {
            // With `remain-on-exit` a finished pane lingers; it's dead then.
            Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
                .lines()
                .any(|l| l.trim() == "0"),
            _ => false,
        }
    }

    /// Kill a session. Returns whether it existed.
    pub fn kill(&self, name: &str) -> bool {
        self.run(&["kill-session", "-t", &format!("={name}")])
            .is_ok_and(|o| o.status.success())
    }

    /// Type `text` into the session's active pane, then Enter.
    pub fn send_line(&self, name: &str, text: &str) -> bool {
        if !self.is_alive(name) {
            return false;
        }
        self.send_keys(&format!("={name}:"), text)
    }

    /// Names of all sessions on the server.
    #[cfg(test)]
    pub fn list(&self) -> Vec<String> {
        match self.run(&["list-sessions", "-F", "#{session_name}"]) {
            Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(str::to_string)
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Kill every session whose name starts with `prefix`, except tome
    /// workspaces; returns their names.
    pub fn kill_prefix(&self, prefix: &str) -> Vec<String> {
        let format = format!("#{{session_name}}\t#{{{}}}", workspace::TMUX_OPTION);
        let sessions = match self.run(&["list-sessions", "-F", &format]) {
            Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).into_owned(),
            _ => return Vec::new(),
        };
        sessions
            .lines()
            .filter_map(|l| l.split_once('\t'))
            .filter(|(name, tag)| tag.is_empty() && name.starts_with(prefix))
            .map(|(name, _)| name.to_string())
            .filter(|name| self.kill(name))
            .collect()
    }
}

// --- cmux ------------------------------------------------------------------

/// The cmux app, through its CLI. Sessions are workspaces, known by id: the
/// app is shared with the user (and any other tome home), so tome only ever
/// touches workspaces it recorded, never ones that merely look like its own.
#[derive(Debug, Clone, Copy, Default)]
pub struct Cmux;

impl Cmux {
    fn cmd(&self) -> Command {
        let mut cmd = Command::new("cmux");
        cmd.env("CMUX_QUIET", "1").stdin(Stdio::null());
        cmd
    }

    fn run(&self, args: &[&str]) -> std::io::Result<Output> {
        self.cmd().args(args).output()
    }

    /// Whether this process runs inside cmux (and so may use it).
    pub fn inside() -> bool {
        ["CMUX_SOCKET_PATH", "CMUX_WORKSPACE_ID"]
            .iter()
            .any(|k| std::env::var_os(k).is_some_and(|v| !v.is_empty()))
    }

    /// Whether cmux answers.
    pub fn available(&self) -> bool {
        self.run(&["ping"]).is_ok_and(|o| o.status.success())
    }

    fn unavailable(&self) -> CliError {
        let why = if Cmux::inside() {
            "cmux isn't answering"
        } else {
            "the tome daemon isn't running inside cmux"
        };
        CliError::internal(format!("can't start a cmux workspace: {why}")).with_hint(
            "start the daemon from a cmux terminal (`tome daemon stop`, then any tome command there), or use `backend: tmux`",
        )
    }

    /// Open a workspace (in the background) running `launch`, and return
    /// its id and its terminal's surface id. Its output is captured with
    /// `script`, which keeps the agent on a terminal.
    pub fn launch(&self, launch: &Launch) -> CliResult<(String, String)> {
        if !self.available() {
            return Err(self.unavailable());
        }
        fs::write(launch.script, script(launch, Capture::Script))?;
        let out = self.run(&[
            "new-workspace",
            "--name",
            launch.title,
            "--cwd",
            &launch.cwd.to_string_lossy(),
            "--command",
            &exec_command(launch),
            "--focus",
            "false",
        ])?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        let reference = stdout
            .trim()
            .strip_prefix("OK ")
            .filter(|_| out.status.success())
            .ok_or_else(|| {
                CliError::internal(format!(
                    "cmux couldn't open a workspace for `{}`: {}",
                    launch.name,
                    String::from_utf8_lossy(&out.stderr).trim()
                ))
            })?;
        match self.wait_for_surface(|s| s.workspace_ref == reference && !s.id.is_empty()) {
            Some(s) => Ok((s.workspace, s.id)),
            None => {
                self.kill(reference);
                Err(CliError::internal(format!(
                    "cmux opened {reference} but tome couldn't find its id"
                )))
            }
        }
    }

    /// Start `launch` in the workspace `record`, as `launch.split` says:
    /// under the `tab` layout as a tab next to the anchor pane, else in the
    /// workspace's tab split (made if it's gone; in a workspace tome doesn't
    /// keep in `places`, the run's latest pane there); under `split` in a
    /// new split off the anchor pane, else off the last one. Returns its
    /// surface id.
    pub fn launch_in(
        &self,
        places: Option<&workspace::Places>,
        mut record: workspace::Record,
        layout: Layout,
        launch: &Launch,
        warnings: &mut Vec<String>,
    ) -> CliResult<String> {
        let split = launch.split;
        fs::write(launch.script, script(launch, Capture::Script))?;
        let failed = |what: &str, out: &Output| {
            CliError::internal(format!(
                "cmux couldn't open a {what} for `{}` in {}: {}",
                launch.name,
                launch.target.label(launch.project),
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        };
        let here: Vec<Surface> = self
            .surfaces()
            .unwrap_or_default()
            .into_iter()
            .filter(|s| s.workspace == record.id && !s.id.is_empty())
            .collect();
        let tabs = match places {
            Some(_) => record
                .tabs
                .clone()
                .filter(|p| here.iter().any(|s| s.pane.as_deref() == Some(p.as_str()))),
            None => split
                .recent
                .iter()
                .find_map(|a| here.iter().find(|s| s.id == a.pane)?.pane.clone()),
        };
        let anchor = split.anchors.iter().find_map(|a| {
            here.iter()
                .find(|s| a.handle == record.id && s.id == a.pane)
        });
        split.missing_anchor(anchor.is_some(), warnings);
        let join = anchor
            .and_then(|a| a.pane.clone())
            .filter(|_| layout == Layout::Tab);
        let surface = match (layout, join.or(tabs)) {
            (Layout::Tab, Some(pane)) => {
                let out = self.run(&[
                    "--id-format",
                    "uuids",
                    "new-surface",
                    "--workspace",
                    &record.id,
                    "--pane",
                    &pane,
                    "--focus",
                    "false",
                ])?;
                first_id(&out).ok_or_else(|| failed("tab", &out))?
            }
            _ => {
                // Off the anchor, else the last pane (cmux lists them left
                // to right).
                let mut args = vec![
                    "--id-format",
                    "uuids",
                    "new-split",
                    split.direction.as_str(),
                    "--workspace",
                    &record.id,
                ];
                if let Some(from) = anchor.or(here.last()) {
                    args.extend(["--surface", from.id.as_str()]);
                }
                args.extend(["--focus", "false"]);
                let out = self.run(&args)?;
                let surface = first_id(&out).ok_or_else(|| failed("split", &out))?;
                self.size_pane(&record.id, &surface, split, warnings);
                if let (Layout::Tab, Some(places)) = (layout, places) {
                    let pane = self
                        .surfaces()
                        .and_then(|all| all.into_iter().find(|s| s.id == surface)?.pane);
                    record.tabs = pane;
                    places.put(&record)?;
                }
                surface
            }
        };
        let _ = self.run(&[
            "rename-tab",
            "--workspace",
            &record.id,
            "--surface",
            &surface,
            launch.title,
        ]);
        if !self.send_line(&record.id, Some(&surface), &exec_command(launch)) {
            self.close_surface(&record.id, &surface);
            return Err(CliError::internal(format!(
                "cmux couldn't start `{}` in its new terminal",
                launch.name
            )));
        }
        Ok(surface)
    }

    /// The workspace `target` names: the focused one if it's still open,
    /// else the tome workspace, made if needed (`places` must be locked),
    /// and whether tome keeps it in `places`.
    fn workspace(
        &self,
        places: &workspace::Places,
        target: &Target,
        project: Option<&Path>,
        home: &Path,
        warnings: &mut Vec<String>,
    ) -> CliResult<(workspace::Record, bool)> {
        match target {
            Target::Focused(id) | Target::Caller(id) if self.is_alive(id) == Some(true) => {
                Ok((workspace::Record::unkept("cmux", id), false))
            }
            target => {
                gone(target, warnings);
                Ok((places.cmux(self, project, target.name(), home)?, true))
            }
        }
    }

    /// Every terminal in every workspace of every window, as cmux's tree
    /// has them; `None` if cmux didn't answer.
    fn surfaces(&self) -> Option<Vec<Surface>> {
        let out = self
            .run(&["--id-format", "both", "tree", "--all", "--json"])
            .ok()
            .filter(|o| o.status.success())?;
        let tree: Value = serde_json::from_slice(&out.stdout).ok()?;
        let mut all = Vec::new();
        for w in tree["windows"]
            .as_array()?
            .iter()
            .flat_map(|w| w["workspaces"].as_array().into_iter().flatten())
        {
            let (Some(workspace), Some(workspace_ref)) = (w["id"].as_str(), w["ref"].as_str())
            else {
                continue;
            };
            let panes = w["panes"].as_array().into_iter().flatten();
            let surfaces = panes.flat_map(|p| {
                p["surfaces"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(move |s| Some((p["id"].as_str()?, s["id"].as_str()?)))
            });
            let mut any = false;
            for (pane, id) in surfaces {
                any = true;
                all.push(Surface {
                    id: id.to_string(),
                    pane: Some(pane.to_string()),
                    workspace: workspace.to_string(),
                    workspace_ref: workspace_ref.to_string(),
                });
            }
            // An empty workspace still counts as open.
            if !any {
                all.push(Surface {
                    id: String::new(),
                    pane: None,
                    workspace: workspace.to_string(),
                    workspace_ref: workspace_ref.to_string(),
                });
            }
        }
        Some(all)
    }

    /// The first surface in cmux's tree that `matches`. cmux answers
    /// `new-workspace` before the workspace (and its terminal) show up in
    /// `tree`, so this polls for a moment.
    fn wait_for_surface(&self, matches: impl Fn(&Surface) -> bool) -> Option<Surface> {
        let deadline = std::time::Instant::now() + CMUX_TREE_WAIT;
        loop {
            if let Some(found) = self
                .surfaces()
                .and_then(|all| all.into_iter().find(&matches))
            {
                return Some(found);
            }
            if std::time::Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Whether the workspace is still open; `None` if cmux didn't answer.
    pub fn is_alive(&self, id: &str) -> Option<bool> {
        Some(self.surfaces()?.iter().any(|s| s.workspace == id))
    }

    /// Whether the surface (tab) is still open (it closes when its command
    /// exits); `None` if cmux didn't answer.
    pub fn surface_alive(&self, id: &str) -> Option<bool> {
        Some(!id.is_empty() && self.surfaces()?.iter().any(|s| s.id == id))
    }

    /// Close a workspace. Returns whether it was open.
    pub fn kill(&self, id: &str) -> bool {
        self.close_forced(&["close-workspace", "--workspace", id])
    }

    /// Close one surface (tab) of a workspace. Returns whether it was open.
    pub fn close_surface(&self, workspace: &str, surface: &str) -> bool {
        self.surface_alive(surface) == Some(true)
            && self.close_forced(&[
                "close-surface",
                "--workspace",
                workspace,
                "--surface",
                surface,
            ])
    }

    /// Run a cmux close command with `--force`, which cmux asks for when a
    /// live process would be killed; older cmux doesn't know the flag, so
    /// it's retried without.
    fn close_forced(&self, args: &[&str]) -> bool {
        let forced = [args, &["--force"]].concat();
        [forced.as_slice(), args]
            .iter()
            .any(|args| self.run(args).is_ok_and(|o| o.status.success()))
    }

    /// Type `text` into a surface of the workspace (its focused one if
    /// `None`), then Enter.
    pub fn send_line(&self, workspace: &str, surface: Option<&str>, text: &str) -> bool {
        let mut target = vec!["--workspace", workspace];
        target.extend(surface.iter().flat_map(|s| ["--surface", *s]));
        let run = |cmd: &str, rest: &[&str]| {
            let mut args = vec![cmd];
            args.extend_from_slice(&target);
            args.extend_from_slice(rest);
            self.run(&args).is_ok_and(|o| o.status.success())
        };
        run("send", &["--", text]) && run("send-key", &["enter"])
    }

    /// Post a cmux notification.
    pub fn notify(&self, title: &str, body: &str) -> bool {
        self.run(&["notify", "--title", title, "--body", body])
            .is_ok_and(|o| o.status.success())
    }
}

/// A terminal in cmux's tree.
struct Surface {
    /// Empty for the stand-in of a workspace with no terminals.
    id: String,
    pane: Option<String>,
    workspace: String,
    /// `workspace:<n>`
    workspace_ref: String,
}

/// The first id in a cmux `OK <id> ...` answer.
fn first_id(out: &Output) -> Option<String> {
    let stdout = String::from_utf8_lossy(&out.stdout);
    stdout
        .trim()
        .strip_prefix("OK ")
        .filter(|_| out.status.success())?
        .split_whitespace()
        .next()
        .map(str::to_string)
}

/// What to type into a cmux terminal to run the launcher: the leading space
/// keeps it out of shell history, and `exec` closes the terminal with it.
fn exec_command(launch: &Launch) -> String {
    format!(" exec sh {}", shell_quote(&launch.script.to_string_lossy()))
}

// --- launcher --------------------------------------------------------------

enum Capture<'a> {
    /// The backend captures output once the session has started (tmux
    /// `pipe-pane`), then creates this file: wait for it.
    AfterStart(&'a Path),
    /// Record it with `script`, which runs the command on a new terminal.
    Script,
}

/// The launcher: set the environment, arrange for output capture, then
/// become the command.
fn script(launch: &Launch, capture: Capture) -> String {
    let mut s = String::from("#!/bin/sh\n");
    // Terminals opened inside an existing workspace start in its directory.
    s.push_str(&format!(
        "cd {} || exit 1\n",
        shell_quote(&launch.cwd.to_string_lossy())
    ));
    for (k, v) in launch.env {
        s.push_str(&format!("export {k}={}\n", shell_quote(v)));
    }
    let argv: Vec<String> = launch.argv.iter().map(|a| shell_quote(a)).collect();
    match capture {
        Capture::AfterStart(ready) => {
            let tries = CAPTURE_WAIT.as_millis() / 50;
            s.push_str(&format!(
                "i=0; while [ ! -e {} ] && [ $i -lt {tries} ]; do sleep 0.05; i=$((i+1)); done\n",
                shell_quote(&ready.to_string_lossy())
            ));
            s.push_str(&format!("exec {}\n", argv.join(" ")));
        }
        Capture::Script => {
            // BSD script: -a append, -F flush each write, -q no banner.
            let log = shell_quote(&launch.log.to_string_lossy());
            s.push_str(&format!("exec script -q -a -F {log} {}\n", argv.join(" ")));
        }
    }
    s
}

/// A session name for a run: `tome-<id>-<workflow>`. tmux doesn't allow `.`
/// or `:` in names, so anything but letters, digits, `_` and `-` becomes `-`.
pub fn run_session_name(run_id: RunId, workflow: &str, role: &str) -> String {
    let slug: String = workflow
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    match role {
        "orchestrator" => format!("{}{slug}", run_prefix(run_id)),
        role => format!("{}{slug}-{role}", run_prefix(run_id)),
    }
}

/// Every session of a run starts with this.
pub fn run_prefix(run_id: RunId) -> String {
    format!("tome-{run_id}-")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// Poll until `f` holds or `limit` passes.
    pub fn wait_until(limit: Duration, mut f: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + limit;
        loop {
            if f() {
                return true;
            }
            if Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// A private tmux server, killed on drop.
    pub struct Server(pub Tmux);

    impl Server {
        pub fn new(tag: &str) -> Option<Server> {
            if !Tmux::available() {
                eprintln!("skipping: tmux isn't installed");
                return None;
            }
            Some(Server(Tmux {
                socket: Some(format!("tome-test-{tag}-{}", std::process::id())),
            }))
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            let _ = self.0.run(&["kill-server"]);
        }
    }

    fn launch(tmux: &Tmux, dir: &Path, name: &str, argv: &[&str]) {
        let argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
        let env = vec![
            ("TOME_RUN_ID".to_string(), "7".to_string()),
            ("GREETING".to_string(), "it's me".to_string()),
        ];
        tmux.launch(&Launch {
            name,
            title: "tome: test #7",
            cwd: dir,
            argv: &argv,
            env: &env,
            script: &dir.join(format!("{name}.sh")),
            log: &dir.join(format!("{name}.log")),
            layout: Layout::Workspace,
            split: &Split::default(),
            target: &Target::Project,
            project: None,
        })
        .unwrap();
    }

    #[test]
    fn launched_command_gets_env_cwd_and_its_output_is_captured() {
        let Some(server) = Server::new("env") else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path().canonicalize().unwrap();
        launch(
            &server.0,
            &dir,
            "tome-7-env",
            &[
                "sh",
                "-c",
                "echo \"run=$TOME_RUN_ID $GREETING in $(pwd)\"; sleep 0.3",
            ],
        );
        let log = dir.join("tome-7-env.log");
        let want = format!("run=7 it's me in {}", dir.display());
        assert!(
            wait_until(Duration::from_secs(5), || fs::read_to_string(&log)
                .is_ok_and(|s| s.contains(&want))),
            "log: {:?}",
            fs::read_to_string(&log)
        );
        // Exits on its own and the session goes away.
        assert!(wait_until(Duration::from_secs(5), || !server
            .0
            .is_alive("tome-7-env")));
    }

    #[test]
    fn sessions_can_be_listed_and_killed_by_run() {
        let Some(server) = Server::new("kill") else {
            return;
        };
        let tmux = &server.0;
        let dir = tempfile::tempdir().unwrap();
        launch(tmux, dir.path(), "tome-7-build", &["sleep", "30"]);
        launch(tmux, dir.path(), "tome-7-build-worker", &["sleep", "30"]);
        launch(tmux, dir.path(), "tome-70-build", &["sleep", "30"]);
        assert!(tmux.is_alive("tome-7-build"));
        assert!(!tmux.is_alive("tome-7-buil"), "names match exactly");

        let mut killed = tmux.kill_prefix(&run_prefix(RunId::new(7)));
        killed.sort();
        assert_eq!(killed, ["tome-7-build", "tome-7-build-worker"]);
        assert!(!tmux.is_alive("tome-7-build"));
        assert!(tmux.is_alive("tome-70-build"));
        assert!(tmux.kill("tome-70-build"));
        assert!(!tmux.kill("tome-70-build"));
    }

    #[test]
    fn dead_pane_with_remain_on_exit_is_not_alive() {
        let Some(server) = Server::new("remain") else {
            return;
        };
        let tmux = &server.0;
        let dir = tempfile::tempdir().unwrap();
        launch(tmux, dir.path(), "tome-8-keep", &["sleep", "30"]);
        tmux.run(&["set-option", "-g", "remain-on-exit", "on"])
            .unwrap();
        tmux.run(&["set-option", "-t", "=tome-8-keep", "remain-on-exit", "on"])
            .unwrap();
        assert!(tmux.is_alive("tome-8-keep"));
        tmux.run(&["send-keys", "-t", "=tome-8-keep:", "C-c"])
            .unwrap();
        assert!(wait_until(Duration::from_secs(5), || !tmux.is_alive("tome-8-keep")));
        assert!(
            tmux.list().contains(&"tome-8-keep".to_string()),
            "pane lingers"
        );
    }

    #[test]
    fn lines_can_be_typed_into_a_session() {
        let Some(server) = Server::new("send") else {
            return;
        };
        let tmux = &server.0;
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("got.txt");
        let cmd = format!("read line; echo \"$line\" > {}; sleep 5", out.display());
        launch(tmux, dir.path(), "tome-9-read", &["sh", "-c", &cmd]);
        assert!(tmux.send_line("tome-9-read", "worker w1 done; it's $HOME"));
        assert!(wait_until(Duration::from_secs(5), || fs::read_to_string(
            &out
        )
        .is_ok_and(|s| s.trim() == "worker w1 done; it's $HOME")));
        assert!(!tmux.send_line("tome-9-gone", "hello"));
    }

    #[test]
    fn run_session_names_are_tmux_safe() {
        assert_eq!(
            run_session_name(RunId::new(42), "review.loop:v2", "orchestrator"),
            "tome-42-review-loop-v2"
        );
        assert_eq!(
            run_session_name(RunId::new(42), "build", "worker"),
            "tome-42-build-worker"
        );
        assert!(run_session_name(RunId::new(42), "build", "orchestrator")
            .starts_with(&run_prefix(RunId::new(42))));
    }
}
