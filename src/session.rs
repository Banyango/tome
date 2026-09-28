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
//! else `backend:` in `~/.tome/config.yaml`, else cmux when the daemon runs
//! inside cmux and tmux otherwise.
//!
//! Its [`Layout`] decides where in that backend its sessions go, and
//! resolves the same way (`defaults.layout`, `TOME_LAYOUT`, `layout:`), else
//! `tab`.
//!
//! Either way the session's command is a launcher script that sets the
//! agent's environment, captures its output to a log and `exec`s it, so the
//! session ends when the agent does.

use crate::harness::{self, shell_quote};
use crate::output::{CliError, CliResult};
use crate::store::Session;
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::Duration;

/// How long a launched command waits for its output to be captured before
/// starting anyway (tmux attaches capture after the session starts).
const CAPTURE_WAIT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Tmux,
    Cmux,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Tmux => "tmux",
            Kind::Cmux => "cmux",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        match s {
            "tmux" => Some(Kind::Tmux),
            "cmux" => Some(Kind::Cmux),
            _ => None,
        }
    }

    /// The backend for a run whose workflow asks for `requested` (see the
    /// module docs for the order).
    pub fn choose(requested: Option<&str>) -> CliResult<Kind> {
        let env = std::env::var("TOME_BACKEND").ok().filter(|s| !s.is_empty());
        let (name, source) = match (requested, env) {
            (Some(r), _) => (r.to_string(), "the workflow's `defaults.backend`"),
            (None, Some(e)) => (e, "TOME_BACKEND"),
            (None, None) => match harness::config_str("backend")? {
                Some(c) => (c, "`backend` in the tome config"),
                None => return Ok(if Cmux::inside() { Kind::Cmux } else { Kind::Tmux }),
            },
        };
        Kind::parse(&name).ok_or_else(|| {
            CliError::invalid(format!("unknown session backend `{name}` (from {source})")).with_hint("use tmux or cmux")
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
    pub const NAMES: &'static [&'static str] = &["tab", "split", "workspace"];

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

    /// The layout for a run whose workflow asks for `requested` (see the
    /// module docs for the order).
    pub fn choose(requested: Option<&str>) -> CliResult<Layout> {
        let env = std::env::var("TOME_LAYOUT").ok().filter(|s| !s.is_empty());
        let (name, source) = match (requested, env) {
            (Some(r), _) => (r.to_string(), "the workflow's `defaults.layout`"),
            (None, Some(e)) => (e, "TOME_LAYOUT"),
            (None, None) => match harness::config_str("layout")? {
                Some(c) => (c, "`layout` in the tome config"),
                None => return Ok(Layout::Tab),
            },
        };
        Layout::parse(&name).ok_or_else(|| {
            CliError::invalid(format!("unknown session layout `{name}` (from {source})"))
                .with_hint(format!("use {}", Layout::NAMES.join(", ")))
        })
    }

    /// A recorded session's layout; sessions from before layouts had their
    /// own workspace.
    pub fn of(s: &Session) -> Layout {
        s.layout.as_deref().and_then(Layout::parse).unwrap_or(Layout::Workspace)
    }
}

/// A backend to start sessions on.
pub enum Backend {
    Tmux(Tmux),
    Cmux(Cmux),
}

impl Backend {
    pub fn new(kind: Kind) -> Backend {
        match kind {
            Kind::Tmux => Backend::Tmux(Tmux::from_env()),
            Kind::Cmux => Backend::Cmux(Cmux),
        }
    }

    pub fn kind(&self) -> Kind {
        match self {
            Backend::Tmux(_) => Kind::Tmux,
            Backend::Cmux(_) => Kind::Cmux,
        }
    }

    /// Start `launch` and return how to find it again: the session record
    /// minus the run, role, layout and harness, which the caller fills in.
    pub fn launch(&self, launch: &Launch) -> CliResult<Session> {
        let (socket, handle, pane) = match self {
            Backend::Tmux(t) => (t.socket.clone(), None, t.launch(launch)?),
            Backend::Cmux(c) => {
                let (workspace, surface) = c.launch(launch)?;
                (None, Some(workspace), surface)
            }
        };
        Ok(Session {
            run_id: 0,
            name: launch.name.to_string(),
            role: String::new(),
            backend: self.kind().as_str().to_string(),
            socket,
            handle,
            pane: Some(pane),
            layout: None,
            harness: None,
            created_at: String::new(),
        })
    }
}

// Recorded sessions are found by their own pane or tab (`pane`: a tmux pane
// id, a cmux surface id). Sessions from before that are found by their tmux
// session name or cmux workspace id instead.

/// Whether a recorded session's command is still running; `None` if that
/// can't be told right now (e.g. cmux isn't answering).
pub fn is_alive(s: &Session) -> Option<bool> {
    match Kind::parse(&s.backend) {
        Some(Kind::Tmux) => {
            let tmux = Tmux { socket: s.socket.clone() };
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
            let tmux = Tmux { socket: s.socket.clone() };
            match (&s.pane, layout) {
                (Some(pane), Layout::Tab) => tmux.kill_window_of(pane),
                (Some(pane), Layout::Split) => tmux.kill_pane(pane),
                _ => tmux.kill(&s.name),
            }
        }
        Some(Kind::Cmux) => match (&s.handle, &s.pane, layout) {
            (Some(workspace), Some(surface), Layout::Tab | Layout::Split) => Cmux.close_surface(workspace, surface),
            (Some(workspace), _, _) => Cmux.kill(workspace),
            (None, _, _) => false,
        },
        None => false,
    }
}

/// Type a line into a recorded session, as if the user had typed it and
/// pressed Enter. Returns whether it was delivered.
pub fn send_line(s: &Session, text: &str) -> bool {
    match Kind::parse(&s.backend) {
        Some(Kind::Tmux) => {
            let tmux = Tmux { socket: s.socket.clone() };
            match &s.pane {
                Some(pane) => tmux.pane_alive(pane) && tmux.send_keys(pane, text),
                None => tmux.send_line(&s.name, text),
            }
        }
        Some(Kind::Cmux) => {
            s.handle.as_deref().is_some_and(|id| Cmux.send_line(id, s.pane.as_deref(), text))
        }
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
}

// --- tmux ------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct Tmux {
    /// `tmux -L <socket>`; `None` is the default server.
    pub socket: Option<String>,
}

impl Tmux {
    pub fn from_env() -> Tmux {
        Tmux { socket: std::env::var("TOME_TMUX_SOCKET").ok().filter(|s| !s.is_empty()) }
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
        Command::new("tmux").arg("-V").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
    }

    /// Start `launch.argv` in a new detached session with `launch.env` set,
    /// capturing its output to `launch.log`. Returns its pane's id.
    pub fn launch(&self, launch: &Launch) -> CliResult<String> {
        if !Tmux::available() {
            return Err(CliError::internal("tmux isn't installed").with_hint("install tmux, or use `backend: cmux`"));
        }
        if self.is_alive(launch.name) {
            return Err(CliError::internal(format!("tmux session `{}` already exists", launch.name)));
        }
        let ready = launch.script.with_extension("ready");
        let _ = fs::remove_file(&ready);
        fs::write(launch.script, script(launch, Capture::AfterStart(&ready)))?;

        let cwd = launch.cwd.to_string_lossy();
        let script = launch.script.to_string_lossy();
        let out = self.run(&[
            "new-session", "-d", "-P", "-F", "#{pane_id}", "-s", launch.name, "-n", launch.title, "-c", &cwd, "-x", "200", "-y",
            "50", "sh", &script,
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

    /// Whether the pane (`%<n>`) exists and its command is still running.
    pub fn pane_alive(&self, pane: &str) -> bool {
        match self.run(&["list-panes", "-t", pane, "-F", "#{pane_id} #{pane_dead}"]) {
            Ok(out) if out.status.success() => {
                String::from_utf8_lossy(&out.stdout).lines().any(|l| l.trim() == format!("{pane} 0"))
            }
            _ => false,
        }
    }

    /// Kill a pane. Returns whether it existed.
    pub fn kill_pane(&self, pane: &str) -> bool {
        self.pane_exists(pane) && self.run(&["kill-pane", "-t", pane]).is_ok_and(|o| o.status.success())
    }

    /// Kill the window a pane is in. Returns whether it existed.
    pub fn kill_window_of(&self, pane: &str) -> bool {
        self.pane_exists(pane) && self.run(&["kill-window", "-t", pane]).is_ok_and(|o| o.status.success())
    }

    /// Whether the pane exists, running or not. (A missing `-t` target
    /// would otherwise fall back to the current pane.)
    fn pane_exists(&self, pane: &str) -> bool {
        match self.run(&["list-panes", "-t", pane, "-F", "#{pane_id}"]) {
            Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).lines().any(|l| l.trim() == pane),
            _ => false,
        }
    }

    /// Type `text` into a pane (or other target), then Enter.
    fn send_keys(&self, target: &str, text: &str) -> bool {
        self.run(&["send-keys", "-t", target, "-l", text]).is_ok_and(|o| o.status.success())
            && self.run(&["send-keys", "-t", target, "Enter"]).is_ok_and(|o| o.status.success())
    }

    /// Whether the session exists and its command is still running.
    pub fn is_alive(&self, name: &str) -> bool {
        match self.run(&["list-panes", "-s", "-t", &format!("={name}"), "-F", "#{pane_dead}"]) {
            // With `remain-on-exit` a finished pane lingers; it's dead then.
            Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).lines().any(|l| l.trim() == "0"),
            _ => false,
        }
    }

    /// Kill a session. Returns whether it existed.
    pub fn kill(&self, name: &str) -> bool {
        self.run(&["kill-session", "-t", &format!("={name}")]).is_ok_and(|o| o.status.success())
    }

    /// Type `text` into the session's active pane, then Enter.
    pub fn send_line(&self, name: &str, text: &str) -> bool {
        if !self.is_alive(name) {
            return false;
        }
        self.send_keys(&format!("={name}:"), text)
    }

    /// Names of all sessions on the server.
    pub fn list(&self) -> Vec<String> {
        match self.run(&["list-sessions", "-F", "#{session_name}"]) {
            Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).lines().map(str::to_string).collect(),
            _ => Vec::new(),
        }
    }

    /// Kill every session whose name starts with `prefix`; returns their names.
    pub fn kill_prefix(&self, prefix: &str) -> Vec<String> {
        self.list().into_iter().filter(|n| n.starts_with(prefix) && self.kill(n)).collect()
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
        ["CMUX_SOCKET_PATH", "CMUX_WORKSPACE_ID"].iter().any(|k| std::env::var_os(k).is_some_and(|v| !v.is_empty()))
    }

    /// Whether cmux answers.
    pub fn available(&self) -> bool {
        self.run(&["ping"]).is_ok_and(|o| o.status.success())
    }

    fn unavailable(&self) -> CliError {
        let why = if Cmux::inside() { "cmux isn't answering" } else { "the tome daemon isn't running inside cmux" };
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
        let reference = stdout.trim().strip_prefix("OK ").filter(|_| out.status.success()).ok_or_else(|| {
            CliError::internal(format!(
                "cmux couldn't open a workspace for `{}`: {}",
                launch.name,
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        })?;
        let found = self.surfaces().and_then(|all| all.into_iter().find(|s| s.workspace_ref == reference && !s.id.is_empty()));
        match found {
            Some(s) => Ok((s.workspace, s.id)),
            None => {
                let _ = self.run(&["close-workspace", "--workspace", reference]);
                Err(CliError::internal(format!("cmux opened {reference} but tome couldn't find its id")))
            }
        }
    }

    /// Every terminal in every workspace of every window, as cmux's tree
    /// has them; `None` if cmux didn't answer.
    fn surfaces(&self) -> Option<Vec<Surface>> {
        let out = self.run(&["--id-format", "both", "tree", "--all", "--json"]).ok().filter(|o| o.status.success())?;
        let tree: Value = serde_json::from_slice(&out.stdout).ok()?;
        let mut all = Vec::new();
        for w in tree["windows"].as_array()?.iter().flat_map(|w| w["workspaces"].as_array().into_iter().flatten()) {
            let (Some(workspace), Some(workspace_ref)) = (w["id"].as_str(), w["ref"].as_str()) else { continue };
            let panes = w["panes"].as_array().into_iter().flatten();
            let surfaces = panes.flat_map(|p| {
                p["surfaces"].as_array().into_iter().flatten().filter_map(move |s| Some((p["id"].as_str()?, s["id"].as_str()?)))
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
        self.run(&["close-workspace", "--workspace", id]).is_ok_and(|o| o.status.success())
    }

    /// Close one surface (tab) of a workspace. Returns whether it was open.
    pub fn close_surface(&self, workspace: &str, surface: &str) -> bool {
        self.surface_alive(surface) == Some(true)
            && self.run(&["close-surface", "--workspace", workspace, "--surface", surface]).is_ok_and(|o| o.status.success())
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
        self.run(&["notify", "--title", title, "--body", body]).is_ok_and(|o| o.status.success())
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
pub fn run_session_name(run_id: i64, workflow: &str, role: &str) -> String {
    let slug: String =
        workflow.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '-' }).collect();
    match role {
        "orchestrator" => format!("{}{slug}", run_prefix(run_id)),
        role => format!("{}{slug}-{role}", run_prefix(run_id)),
    }
}

/// Every session of a run starts with this.
pub fn run_prefix(run_id: i64) -> String {
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
    struct Server(Tmux);

    impl Server {
        fn new(tag: &str) -> Option<Server> {
            if !Tmux::available() {
                eprintln!("skipping: tmux isn't installed");
                return None;
            }
            Some(Server(Tmux { socket: Some(format!("tome-test-{tag}-{}", std::process::id())) }))
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            let _ = self.0.run(&["kill-server"]);
        }
    }

    fn launch(tmux: &Tmux, dir: &Path, name: &str, argv: &[&str]) {
        let argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
        let env = vec![("TOME_RUN_ID".to_string(), "7".to_string()), ("GREETING".to_string(), "it's me".to_string())];
        tmux.launch(&Launch {
            name,
            title: "tome: test #7",
            cwd: dir,
            argv: &argv,
            env: &env,
            script: &dir.join(format!("{name}.sh")),
            log: &dir.join(format!("{name}.log")),
        })
        .unwrap();
    }

    #[test]
    fn launched_command_gets_env_cwd_and_its_output_is_captured() {
        let Some(server) = Server::new("env") else { return };
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path().canonicalize().unwrap();
        launch(&server.0, &dir, "tome-7-env", &["sh", "-c", "echo \"run=$TOME_RUN_ID $GREETING in $(pwd)\"; sleep 0.3"]);
        let log = dir.join("tome-7-env.log");
        let want = format!("run=7 it's me in {}", dir.display());
        assert!(
            wait_until(Duration::from_secs(5), || fs::read_to_string(&log).is_ok_and(|s| s.contains(&want))),
            "log: {:?}",
            fs::read_to_string(&log)
        );
        // Exits on its own and the session goes away.
        assert!(wait_until(Duration::from_secs(5), || !server.0.is_alive("tome-7-env")));
    }

    #[test]
    fn sessions_can_be_listed_and_killed_by_run() {
        let Some(server) = Server::new("kill") else { return };
        let tmux = &server.0;
        let dir = tempfile::tempdir().unwrap();
        launch(tmux, dir.path(), "tome-7-build", &["sleep", "30"]);
        launch(tmux, dir.path(), "tome-7-build-worker", &["sleep", "30"]);
        launch(tmux, dir.path(), "tome-70-build", &["sleep", "30"]);
        assert!(tmux.is_alive("tome-7-build"));
        assert!(!tmux.is_alive("tome-7-buil"), "names match exactly");

        let mut killed = tmux.kill_prefix(&run_prefix(7));
        killed.sort();
        assert_eq!(killed, ["tome-7-build", "tome-7-build-worker"]);
        assert!(!tmux.is_alive("tome-7-build"));
        assert!(tmux.is_alive("tome-70-build"));
        assert!(tmux.kill("tome-70-build"));
        assert!(!tmux.kill("tome-70-build"));
    }

    #[test]
    fn dead_pane_with_remain_on_exit_is_not_alive() {
        let Some(server) = Server::new("remain") else { return };
        let tmux = &server.0;
        let dir = tempfile::tempdir().unwrap();
        launch(tmux, dir.path(), "tome-8-keep", &["sleep", "30"]);
        tmux.run(&["set-option", "-g", "remain-on-exit", "on"]).unwrap();
        tmux.run(&["set-option", "-t", "=tome-8-keep", "remain-on-exit", "on"]).unwrap();
        assert!(tmux.is_alive("tome-8-keep"));
        tmux.run(&["send-keys", "-t", "=tome-8-keep:", "C-c"]).unwrap();
        assert!(wait_until(Duration::from_secs(5), || !tmux.is_alive("tome-8-keep")));
        assert!(tmux.list().contains(&"tome-8-keep".to_string()), "pane lingers");
    }

    #[test]
    fn lines_can_be_typed_into_a_session() {
        let Some(server) = Server::new("send") else { return };
        let tmux = &server.0;
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("got.txt");
        let cmd = format!("read line; echo \"$line\" > {}; sleep 5", out.display());
        launch(tmux, dir.path(), "tome-9-read", &["sh", "-c", &cmd]);
        assert!(tmux.send_line("tome-9-read", "worker w1 done; it's $HOME"));
        assert!(wait_until(Duration::from_secs(5), || fs::read_to_string(&out).is_ok_and(|s| s.trim() == "worker w1 done; it's $HOME")));
        assert!(!tmux.send_line("tome-9-gone", "hello"));
    }

    #[test]
    fn run_session_names_are_tmux_safe() {
        assert_eq!(run_session_name(42, "review.loop:v2", "orchestrator"), "tome-42-review-loop-v2");
        assert_eq!(run_session_name(42, "build", "worker"), "tome-42-build-worker");
        assert!(run_session_name(42, "build", "orchestrator").starts_with(&run_prefix(42)));
    }
}
