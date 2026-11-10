//! Bulb's daemon adapter. Bulb serves JSON commands (`{id, cmd, args}` →
//! `{id, ok, result|error}`) on a local WebSocket whose port and token it
//! writes to `$BULB_HOME/daemon.json`. Each connection says which project it
//! acts on with `hello` first. Bulb sessions are terminals; tome opens one
//! per agent with `open` and finds it again by its session id.

use super::ws::Ws;
use super::{Launch, Layout, Target};
use crate::config::Config;
use crate::output::{CliError, CliResult};
use crate::placement::{Direction, Size};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(3);
/// How long a daemon tome started gets to write its `daemon.json`.
const START_WAIT: Duration = Duration::from_secs(10);

/// A Bulb project: the daemon's sessions are grouped by it.
pub struct Bulb {
    pub project: PathBuf,
}

/// Where a running daemon listens.
struct Info {
    port: u16,
    token: String,
}

fn unavailable(why: impl std::fmt::Display) -> CliError {
    CliError::internal(format!("backend_unavailable: Bulb isn't available: {why}")).with_hint(
        "start Bulb, or set `bulb.app_root` in ~/.tome/config.yaml so tome can start its daemon",
    )
}

fn mismatch(why: impl std::fmt::Display) -> CliError {
    CliError::internal(format!("Bulb protocol mismatch: {why}"))
        .with_hint("update Bulb or tome so they speak the same protocol")
}

fn bulb_home() -> PathBuf {
    std::env::var_os("BULB_HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::paths::user_home().join(".bulb"))
}

/// The running daemon, if its info file names a live process.
fn running() -> Option<Info> {
    let text = std::fs::read_to_string(bulb_home().join("daemon.json")).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    let pid = i32::try_from(v["pid"].as_i64()?).ok()?;
    // SAFETY: signal 0 only checks that the process exists.
    if unsafe { libc::kill(pid, 0) } != 0 {
        return None;
    }
    Some(Info {
        port: u16::try_from(v["port"].as_u64()?).ok()?,
        token: v["token"].as_str()?.to_owned(),
    })
}

/// Bulb's install, which holds the daemon's entry point: from Bulb's own
/// environment (`BULB_APP_ROOT`, or the docs path its terminals get), else
/// `bulb.app_root` in the global config.
fn app_root() -> Option<PathBuf> {
    let env = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let configured = || {
        let config = Config::load(None).ok()?;
        let (value, _) = config.get("bulb")?;
        let root = value.get("app_root")?.as_str()?;
        Some(match root.strip_prefix("~/") {
            Some(rest) => crate::paths::user_home().join(rest),
            None => PathBuf::from(root),
        })
    };
    env("BULB_APP_ROOT")
        .or_else(|| env("BULB_DOCS").and_then(|d| Some(d.parent()?.parent()?.to_path_buf())))
        .or_else(configured)
        .filter(|root| root.join("src/daemon/main.ts").is_file())
}

/// The running daemon, started headless when there isn't one.
fn ensure_daemon() -> CliResult<Info> {
    if let Some(info) = running() {
        return Ok(info);
    }
    let root = app_root().ok_or_else(|| {
        unavailable("its daemon isn't running and tome can't find Bulb's install to start it")
    })?;
    let node = std::env::var("BULB_NODE").unwrap_or_else(|_| "node".into());
    Command::new(&node)
        .arg(root.join("src/daemon/main.ts"))
        .env("BULB_APP_ROOT", &root)
        .current_dir(crate::paths::user_home())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| unavailable(format!("couldn't start its daemon with `{node}`: {e}")))?;
    let deadline = std::time::Instant::now() + START_WAIT;
    while std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
        if let Some(info) = running() {
            return Ok(info);
        }
    }
    Err(unavailable("its daemon didn't start"))
}

impl Bulb {
    pub fn new(project: impl Into<PathBuf>) -> Self {
        Self {
            project: project.into(),
        }
    }

    /// The Bulb project this process runs in, else the current directory.
    pub fn from_env() -> Self {
        let project = std::env::var_os("BULB_PROJECT")
            .filter(|p| !p.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(crate::paths::user_home);
        Self::new(project)
    }

    /// Run `cmd` on the daemon, starting it if `start`.
    fn call_with(&self, start: bool, cmd: &str, args: Value) -> CliResult<Value> {
        let info = match start {
            true => ensure_daemon()?,
            false => running().ok_or_else(|| unavailable("its daemon isn't running"))?,
        };
        let mut ws = Ws::connect(info.port, &format!("/?token={}", info.token), TIMEOUT)
            .map_err(unavailable)?;
        let send = |ws: &mut Ws, id: u64, cmd: &str, args: &Value| {
            ws.send_text(&json!({"id": id, "cmd": cmd, "args": args}).to_string())
                .map_err(unavailable)
        };
        send(&mut ws, 1, "hello", &json!({"project": self.project}))?;
        send(&mut ws, 2, cmd, &args)?;
        loop {
            let text = ws.recv_text().map_err(unavailable)?;
            let reply: Value = serde_json::from_str(&text).map_err(mismatch)?;
            // Broadcast events have no id; the hello reply is checked by
            // the command failing without it.
            match reply["id"].as_u64() {
                Some(1) if reply["ok"] == false => {
                    return Err(mismatch(format!(
                        "hello: {}",
                        reply["error"].as_str().unwrap_or("refused")
                    )))
                }
                Some(2) if reply["ok"] == true => return Ok(reply["result"].clone()),
                Some(2) => {
                    let error = reply["error"].as_str().unwrap_or("request failed");
                    return Err(match error.starts_with("unknown command") {
                        true => mismatch(error),
                        false => CliError::internal(format!("Bulb {cmd}: {error}")),
                    });
                }
                _ => {}
            }
        }
    }

    fn call(&self, cmd: &str, args: Value) -> CliResult<Value> {
        self.call_with(true, cmd, args)
    }

    /// The project's sessions; `None` if the daemon didn't answer, and
    /// empty if it isn't running (its sessions end with it).
    fn sessions(&self) -> Option<Vec<Value>> {
        running()?;
        match self.call_with(false, "session.list", json!({})) {
            Ok(Value::Array(list)) => Some(list),
            _ => None,
        }
    }

    fn session(&self, id: &str) -> Option<Option<Value>> {
        let all = match running() {
            Some(_) => self.sessions()?,
            None => return Some(None),
        };
        Some(all.into_iter().find(|s| s["id"] == id))
    }

    /// Open `l` as a Bulb terminal and return its session id.
    pub fn launch(&self, l: &Launch, warnings: &mut Vec<String>) -> CliResult<String> {
        std::fs::write(l.script, super::script(l, super::Capture::Script))?;
        let mut args = json!({
            "title": l.title,
            "cwd": l.cwd,
            // The login shell Bulb starts runs this; `exec` ends the session
            // with the agent.
            "command": format!("exec sh {}", crate::text::shell_quote(&l.script.to_string_lossy())),
        });
        let live = |id: &str| self.session(id).flatten().is_some();
        let o = opening(l, live, warnings);
        args["relation"] = json!(o.relation);
        for (key, value) in [("near", o.near), ("hint", o.hint), ("size", o.size)] {
            if let Some(v) = value {
                args[key] = json!(v);
            }
        }
        let opened = self.call("open", args)?;
        opened["session"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| mismatch("`open` didn't return a session id"))
    }

    /// Whether the session is running; `None` if Bulb didn't answer.
    pub fn alive(&self, id: &str) -> Option<bool> {
        Some(self.session(id)?.is_some())
    }

    pub fn close(&self, id: &str) -> bool {
        self.alive(id) == Some(true)
            && self
                .call_with(false, "session.close", json!({"session": id}))
                .is_ok()
    }

    pub fn send_line(&self, id: &str, text: &str) -> bool {
        self.alive(id) == Some(true)
            && self
                .call_with(
                    false,
                    "session.input",
                    json!({"session": id, "data": format!("{text}\r")}),
                )
                .is_ok()
    }

    /// The agent status its harness hooks last reported, in tome's terms:
    /// Bulb's `waiting` is `blocked`. `None` when there is none to report
    /// (no hooks, or Bulb didn't answer).
    pub fn agent_status(&self, id: &str) -> Option<String> {
        let session = self.session(id)??;
        let status = session["agent"]["status"].as_str()?;
        Some(match status {
            "waiting" => "blocked".to_owned(),
            other => other.to_owned(),
        })
    }

    /// Best effort: a Bulb notification, if its daemon is running.
    pub fn notify(&self, title: &str, body: &str) {
        let _ = self.call_with(false, "notify", json!({"title": title, "body": body}));
    }

    /// The `from: caller` anchor for the Bulb session `id` in `project`.
    pub fn caller_anchor(
        project: &str,
        id: &str,
    ) -> Result<(super::split::Anchor, String), String> {
        let bulb = Bulb::new(project);
        let session = bulb
            .session(id)
            .ok_or("Bulb isn't answering")?
            .ok_or("the caller's Bulb session is gone")?;
        let label = session["title"].as_str().unwrap_or(id).to_owned();
        Ok((
            super::split::Anchor {
                handle: project.to_owned(),
                pane: id.to_owned(),
            },
            label,
        ))
    }
}

/// Inside a Bulb terminal.
pub fn inside() -> bool {
    std::env::var("BULB_SESSION").is_ok_and(|s| !s.is_empty())
}

/// The Bulb project a launch opens in: the caller's, else the run's.
pub fn project_of(l: &Launch) -> PathBuf {
    match l.target {
        Target::Caller(project) => PathBuf::from(project),
        _ => l
            .project
            .map(Path::to_path_buf)
            .unwrap_or_else(|| Bulb::from_env().project),
    }
}

/// How `open` places a session.
#[derive(Debug, PartialEq, Eq)]
struct Opening {
    relation: &'static str,
    near: Option<String>,
    hint: Option<String>,
    size: Option<String>,
}

/// Placement as `open` arguments: tabs (and own workspaces, which Bulb
/// hasn't got) are relation `tab`; a split is a `child` of the caller or a
/// `sibling` of an earlier session, with its direction as the hint. Bulb's
/// arrangement decides the rest.
fn opening(l: &Launch, live: impl Fn(&str) -> bool, warnings: &mut Vec<String>) -> Opening {
    if let Target::Named(_) | Target::Focused(_) = l.target {
        warnings.push(format!(
            "workspace: Bulb has no workspaces; opened in the Bulb project instead of {}",
            l.target.label(l.project)
        ));
    }
    let caller = matches!(l.target, Target::Caller(_));
    let near = match caller {
        true => l.split.anchors.first().map(|a| a.pane.clone()),
        false => l
            .split
            .anchors
            .iter()
            .chain(l.split.recent.iter())
            .map(|a| a.pane.clone())
            .find(|p| live(p)),
    };
    let near_sel = near.map(|n| format!("session:{n}"));
    if l.layout != Layout::Split {
        return Opening {
            relation: "tab",
            near: near_sel,
            hint: None,
            size: None,
        };
    }
    let size = match l.split.size {
        Some(Size::Percent(n)) => Some(format!("{}%", n.clamp(10, 90))),
        Some(s @ Size::Cells(_)) => {
            warnings.push(format!(
                "split.size {s}: Bulb sizes splits in pixels or percent; skipped"
            ));
            None
        }
        None => None,
    };
    let hint = match l.split.direction {
        Direction::Right => "right",
        Direction::Left => "left",
        Direction::Down => "down",
        Direction::Up => "up",
    };
    Opening {
        relation: match (caller, &near_sel) {
            (false, Some(_)) => "sibling",
            _ => "child",
        },
        near: near_sel,
        hint: Some(hint.to_owned()),
        size,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{Anchor, Split};

    fn open(layout: Layout, target: Target, split: Split) -> (Opening, Vec<String>) {
        let dir = Path::new("/p");
        let argv = vec!["agent".to_string()];
        let l = Launch {
            name: "tome-1-build",
            title: "tome: build #1",
            cwd: dir,
            argv: &argv,
            env: &[],
            script: dir,
            log: dir,
            layout,
            split: &split,
            target: &target,
            project: Some(dir),
            workspace_label: None,
        };
        let mut warnings = Vec::new();
        let o = opening(&l, |p| p == "s2", &mut warnings);
        (o, warnings)
    }

    fn anchor(pane: &str) -> Anchor {
        Anchor {
            handle: "/p".into(),
            pane: pane.into(),
        }
    }

    #[test]
    fn tabs_and_workspaces_open_as_tabs() {
        for layout in [Layout::Tab, Layout::Workspace] {
            let (o, warnings) = open(layout, Target::Project, Split::default());
            assert_eq!(o.relation, "tab");
            assert_eq!(o.near, None);
            assert!(warnings.is_empty());
        }
    }

    #[test]
    fn a_caller_split_is_a_child_of_the_callers_session() {
        let mut split = Split::default();
        split.anchors = vec![anchor("s1")];
        split.direction = Direction::Down;
        split.size = Some(Size::Percent(30));
        let (o, _) = open(Layout::Split, Target::Caller("/p".into()), split);
        assert_eq!(
            o,
            Opening {
                relation: "child",
                near: Some("session:s1".into()),
                hint: Some("down".into()),
                size: Some("30%".into()),
            }
        );
    }

    #[test]
    fn a_split_is_a_sibling_of_a_live_earlier_session() {
        let mut split = Split::default();
        split.recent = vec![anchor("gone"), anchor("s2")];
        split.size = Some(Size::Cells(80));
        let (o, warnings) = open(Layout::Split, Target::Project, split);
        assert_eq!(o.relation, "sibling");
        assert_eq!(o.near.as_deref(), Some("session:s2"));
        assert_eq!(o.size, None);
        assert!(warnings[0].contains("split.size 80"), "{warnings:?}");
    }

    #[test]
    fn named_workspaces_warn() {
        let (o, warnings) = open(Layout::Tab, Target::Named("ops".into()), Split::default());
        assert_eq!(o.relation, "tab");
        assert!(
            warnings[0].contains("Bulb has no workspaces"),
            "{warnings:?}"
        );
    }

    #[test]
    fn a_stopped_daemon_means_dead_sessions() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("BULB_HOME", dir.path());
        assert_eq!(Bulb::new("/p").alive("s1"), Some(false));
        std::env::remove_var("BULB_HOME");
    }
}

/// Against a real Bulb: `TOME_BULB_E2E=<bulb checkout> cargo test --bin tome
/// session::bulb::e2e -- --ignored`. Uses its own `BULB_HOME`.
#[cfg(test)]
mod e2e {
    use super::*;
    use crate::session::Split;

    #[test]
    #[ignore]
    fn opens_reads_and_closes_a_session() {
        let root = std::env::var("TOME_BULB_E2E").expect("TOME_BULB_E2E names a Bulb checkout");
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        std::env::set_var("BULB_HOME", home.path());
        std::env::set_var("BULB_APP_ROOT", &root);
        let argv = vec!["sh".to_string(), "-c".into(), "echo tome-was-here; sleep 30".into()];
        let script = project.path().join("agent.sh");
        let log = project.path().join("agent.log");
        let split = Split::default();
        let l = Launch {
            name: "tome-1-e2e",
            title: "tome: e2e #1",
            cwd: project.path(),
            argv: &argv,
            env: &[],
            script: &script,
            log: &log,
            layout: Layout::Tab,
            split: &split,
            target: &Target::Project,
            project: Some(project.path()),
            workspace_label: None,
        };
        let bulb = Bulb::new(project.path());
        let id = bulb.launch(&l, &mut Vec::new()).unwrap();
        assert_eq!(bulb.alive(&id), Some(true));
        std::thread::sleep(Duration::from_secs(2));
        let read = bulb.call_with(false, "session.read", json!({"session": id})).unwrap();
        assert!(read["data"].as_str().unwrap().contains("tome-was-here"), "{read}");
        assert!(bulb.send_line(&id, "true"));
        assert_eq!(bulb.agent_status(&id), None);
        assert!(bulb.close(&id));
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(bulb.alive(&id), Some(false));
        let _ = bulb.call_with(false, "daemon.stop", json!({}));
    }
}
