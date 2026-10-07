//! Nodes: other machines with tome installed, reached over SSH and listed
//! under `nodes:` in the global `~/.tome/config.yaml`.
//!
//! A command sent to a node (`--on <node>`, `TOME_NODE`, or a `<node>:<id>`
//! run reference) runs `ssh <dest> <tome> rpc --stdio` and speaks the usual
//! JSON-RPC through it to the node's own daemon. Nothing is shared between
//! daemons: each owns its runs.

use crate::config::{self, Config};
use crate::output::{CliError, CliResult};
use crate::paths;
use crate::rpc;
use serde::Serialize;
use serde_json::{json, Value};
use serde_yaml::{Mapping, Value as Yaml};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The name that means this machine.
pub const LOCAL: &str = "local";

/// The version of the JSON-RPC protocol between a CLI and a node's daemon.
/// Bumped when a change would break an older or newer peer.
pub const PROTOCOL: u64 = 1;

/// SSH's connect timeout, in seconds.
pub const CONNECT_TIMEOUT: u64 = 10;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Node {
    pub name: String,
    /// Anything `ssh` accepts: `user@host`, or a Host alias.
    pub ssh: String,
    /// The tome binary on the node (default: `tome` on its PATH).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tome: Option<String>,
    /// Where this machine's projects are on the node, by directory name.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub projects: BTreeMap<String, String>,
}

impl Node {
    pub fn tome(&self) -> &str {
        self.tome.as_deref().unwrap_or("tome")
    }

    /// `ssh` with batch mode and the connect timeout, then `extra` options,
    /// then the destination.
    pub fn ssh_command(&self, extra: &[&str]) -> std::process::Command {
        let mut cmd = std::process::Command::new(ssh_bin());
        cmd.args(["-o", "BatchMode=yes", "-o"])
            .arg(format!("ConnectTimeout={CONNECT_TIMEOUT}"))
            .args(extra)
            .arg(&self.ssh);
        cmd
    }

    /// The hint for a node that can't be reached.
    pub fn check_hint(&self) -> String {
        format!("run `tome node check {}`", self.name)
    }
}

/// The ssh client (`TOME_SSH` overrides it, for tests).
pub fn ssh_bin() -> String {
    std::env::var("TOME_SSH")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "ssh".into())
}

/// A node name: `[a-z0-9_-]`, and not `local`.
pub fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
    {
        return Err(format!(
            "invalid node name `{name}`: use lowercase letters, digits, `_` and `-`"
        ));
    }
    if name == LOCAL {
        return Err("`local` is reserved: it means this machine".into());
    }
    Ok(())
}

/// The configured nodes, by name.
pub fn all() -> CliResult<BTreeMap<String, Node>> {
    let cfg = Config::load(None)?;
    let mut out = BTreeMap::new();
    for (name, (value, file)) in cfg.entries("nodes")? {
        let node = parse(&name, &value).map_err(|e| file.error(format!("nodes.{name}: {e}")))?;
        out.insert(name, node);
    }
    Ok(out)
}

fn parse(name: &str, value: &Yaml) -> Result<Node, String> {
    check_name(name)?;
    let map = value
        .as_mapping()
        .ok_or("must be a mapping with at least `ssh`")?;
    let text = |key: &str| -> Result<Option<String>, String> {
        match map.get(key) {
            None | Some(Yaml::Null) => Ok(None),
            Some(Yaml::String(s)) if !s.trim().is_empty() => Ok(Some(s.clone())),
            Some(_) => Err(format!("`{key}` must be a non-empty string")),
        }
    };
    for key in map.keys() {
        let key = key.as_str().unwrap_or("?");
        if !matches!(key, "ssh" | "tome" | "projects") {
            return Err(format!("unknown key `{key}` (known: ssh, tome, projects)"));
        }
    }
    let ssh = text("ssh")?.ok_or("`ssh` is required: the destination to `ssh` to")?;
    let mut projects = BTreeMap::new();
    match map.get("projects") {
        None | Some(Yaml::Null) => {}
        Some(Yaml::Mapping(m)) => {
            for (k, v) in m {
                match (k.as_str(), v.as_str()) {
                    (Some(k), Some(v)) => {
                        projects.insert(k.to_string(), v.to_string());
                    }
                    _ => {
                        return Err("`projects` maps a project's directory name to its path".into())
                    }
                }
            }
        }
        Some(_) => return Err("`projects` maps a project's directory name to its path".into()),
    }
    Ok(Node {
        name: name.to_string(),
        ssh,
        tome: text("tome")?,
        projects,
    })
}

/// A configured node by name.
pub fn get(name: &str) -> CliResult<Node> {
    if let Err(e) = check_name(name) {
        if name != LOCAL {
            return Err(CliError::invalid(e));
        }
    }
    let nodes = all()?;
    nodes.get(name).cloned().ok_or_else(|| {
        let known = if nodes.is_empty() {
            "no nodes are configured".to_string()
        } else {
            format!(
                "configured: {}",
                nodes.keys().cloned().collect::<Vec<_>>().join(", ")
            )
        };
        CliError::invalid(format!("no node named `{name}` ({known})")).with_hint(format!(
            "add it with `tome node add {name} <ssh-destination>`"
        ))
    })
}

// --- the node this command talks to ------------------------------------

static TARGET: OnceLock<Node> = OnceLock::new();

/// Send this process's daemon requests to `node`.
pub fn set_target(node: Node) {
    let _ = TARGET.set(node);
}

/// The node this process's daemon requests go to, if not this machine.
pub fn target() -> Option<&'static Node> {
    TARGET.get()
}

/// ` on mini` when talking to a node, else nothing.
pub fn on_suffix() -> String {
    target()
        .map(|n| format!(" on {}", n.name))
        .unwrap_or_default()
}

/// Split a `<node>:<id>` run reference.
pub fn split_ref(r: &str) -> (Option<&str>, &str) {
    match r.split_once(':') {
        Some((node, id)) if !node.is_empty() => (Some(node), id),
        _ => (None, r),
    }
}

/// The node a command goes to: `--on` (or `TOME_NODE`) and a `<node>:<id>`
/// reference must agree; `local` means this machine.
pub fn resolve(on: Option<&str>, from_ref: Option<&str>) -> CliResult<Option<Node>> {
    let on = on.map(str::trim).filter(|s| !s.is_empty());
    let name = match (on, from_ref) {
        (Some(a), Some(b)) if a != b => {
            return Err(CliError::invalid(format!(
                "--on {a} disagrees with the run reference `{b}:…`"
            ))
            .with_hint("give the node once: either --on or <node>:<id>"))
        }
        (Some(a), _) => a,
        (None, Some(b)) => b,
        (None, None) => return Ok(None),
    };
    if name == LOCAL {
        return Ok(None);
    }
    get(name).map(Some)
}

// --- projects on a node -------------------------------------------------

/// Where a node's copy of this machine's project `local` might be: its
/// `projects.<dirname>` entry, then the same path relative to the home
/// directory.
pub fn candidates(node: &Node, local: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let name = dir_name(local);
    if let Some(p) = node.projects.get(&name) {
        out.push(p.clone());
    }
    let home = paths::user_home();
    let home = home.canonicalize().unwrap_or(home);
    if let Ok(rel) = local.strip_prefix(&home) {
        let rel = format!("~/{}", rel.display());
        if !out.contains(&rel) {
            out.push(rel);
        }
    }
    out
}

fn dir_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A project found on a node, with its git state if asked for.
pub struct Located {
    pub path: String,
    pub git: Option<Value>,
}

/// Find the node's copy of `local` (a project root here), over `client`.
pub fn locate(
    client: &mut rpc::Client,
    node: &Node,
    local: &Path,
    git: bool,
) -> CliResult<Located> {
    let tried = candidates(node, local);
    let name = dir_name(local);
    let key = format!("nodes.{}.projects.{name}", node.name);
    if tried.is_empty() {
        return Err(CliError::not_found(format!(
            "don't know where {} is on {}: it isn't under your home directory",
            local.display(),
            node.name
        ))
        .with_hint(format!("set `{key}` in ~/.tome/config.yaml")));
    }
    let out = client.call("project.locate", json!({ "candidates": tried, "git": git }))?;
    match out["path"].as_str() {
        Some(path) => Ok(Located {
            path: path.to_string(),
            git: Some(out["git"].clone()).filter(|g| !g.is_null()),
        }),
        None => {
            let tried: Vec<String> = out["tried"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|t| t.as_str().map(str::to_string))
                .collect();
            Err(CliError::not_found(format!(
                "project {name} isn't on {}: no .tome/ in {}",
                node.name,
                tried.join(" or ")
            ))
            .with_hint(format!(
                "clone it there, or set `{key}` in ~/.tome/config.yaml to its path on {}",
                node.name
            )))
        }
    }
}

/// The project a command acts on: the current one here, or its copy on the
/// target node. Located once per process.
pub fn project(cwd: &Path) -> CliResult<Option<PathBuf>> {
    let Some(local) = crate::triggerscmd::project_of(cwd) else {
        return Ok(None);
    };
    map_project(&local).map(Some)
}

/// A project root here as the target node knows it (itself, without one).
pub fn map_project(local: &Path) -> CliResult<PathBuf> {
    let Some(node) = target() else {
        return Ok(local.to_path_buf());
    };
    static FOUND: OnceLock<std::sync::Mutex<BTreeMap<PathBuf, PathBuf>>> = OnceLock::new();
    let found = FOUND.get_or_init(Default::default);
    if let Some(p) = found.lock().unwrap_or_else(|p| p.into_inner()).get(local) {
        return Ok(p.clone());
    }
    let mut client = rpc::Client::to_node(node)?;
    let path = PathBuf::from(locate(&mut client, node, local, false)?.path);
    found
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(local.to_path_buf(), path.clone());
    Ok(path)
}

/// `project.locate {candidates, git?}` (daemon side): the first candidate
/// (`~/` is this machine's home) that holds `.tome/`, with its git `HEAD`
/// and whether it has uncommitted changes when asked.
pub fn locate_rpc(p: &Value) -> CliResult<Value> {
    let candidates: Vec<String> = p["candidates"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c.as_str().map(str::to_string))
        .collect();
    let tried: Vec<PathBuf> = candidates.iter().map(|c| expand_home(c)).collect();
    let found = tried
        .iter()
        .find(|p| p.join(".tome").is_dir())
        .map(|p| p.canonicalize().unwrap_or_else(|_| p.clone()));
    let git = match (&found, p["git"] == true) {
        (Some(path), true) => git_state(path),
        _ => None,
    };
    Ok(json!({ "path": found, "tried": tried, "git": git }))
}

/// `workflow.resolve {name, project_path?}` (daemon side): the workflow
/// called `name` in the project, else in the global workflows. Names only:
/// a path from another machine means nothing here.
pub fn resolve_workflow_rpc(p: &Value) -> CliResult<Value> {
    let name = crate::api::req_str(p, "name")?;
    if name.contains('/') || name.ends_with(".md") {
        return Err(CliError::invalid(format!(
            "`{name}` is a path; a node resolves workflows by name"
        )));
    }
    let dir = crate::api::opt_str(p, "project_path")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"));
    let library = crate::workflow::Library::discover(&dir);
    let wf = match library.locate(name)? {
        Ok(wf) => wf.path().to_path_buf(),
        Err(inv) => inv.path,
    };
    let scope = match library.scope_of(&wf) {
        crate::workflow::Scope::Global => "global",
        crate::workflow::Scope::Project => "project",
    };
    Ok(json!({ "name": name, "path": wf, "scope": scope }))
}

/// `~/x` against this machine's home.
pub fn expand_home(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => paths::user_home().join(rest),
        None if p == "~" => paths::user_home(),
        None => PathBuf::from(p),
    }
}

/// A repo's `HEAD` commit and whether it has uncommitted changes; `None`
/// outside a git repo.
pub fn git_state(dir: &Path) -> Option<Value> {
    let git = |args: &[&str]| -> Option<String> {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let head = git(&["rev-parse", "HEAD"])?;
    let status = git(&["status", "--porcelain", "--untracked-files=no"])?;
    Some(json!({ "head": head, "dirty": !status.is_empty() }))
}

/// The warnings for code that differs between here and a node.
pub fn drift(
    node: &str,
    project: &str,
    here: Option<&Value>,
    there: Option<&Value>,
) -> Vec<String> {
    let (Some(here), Some(there)) = (here, there) else {
        return Vec::new();
    };
    let short = |v: &Value| {
        v["head"]
            .as_str()
            .unwrap_or("?")
            .chars()
            .take(7)
            .collect::<String>()
    };
    let mut out = Vec::new();
    if here["head"] != there["head"] {
        out.push(format!(
            "{node}'s {project} is at {}, yours is at {}",
            short(there),
            short(here)
        ));
    }
    if there["dirty"] == true {
        out.push(format!("{node}'s {project} has uncommitted changes"));
    }
    if here["dirty"] == true {
        out.push(format!(
            "your {project} has uncommitted changes, which {node} doesn't have"
        ));
    }
    out
}

// --- this machine -------------------------------------------------------

/// This machine's short host name: how nodes name it as the origin of
/// forwarded events.
pub fn host_name() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: gethostname writes at most `len` bytes into the buffer.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    let name = if rc == 0 {
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        String::from_utf8_lossy(&buf[..end]).into_owned()
    } else {
        String::new()
    };
    let short = name.split('.').next().unwrap_or("").trim().to_lowercase();
    if short.is_empty() {
        "unknown".into()
    } else {
        short
    }
}

/// `daemon.ping`: who answers, and which protocol it speaks.
pub fn ping() -> Value {
    json!({
        "pong": true,
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": PROTOCOL,
        "host": host_name(),
    })
}

/// Check a node's `daemon.ping` answer against this tome.
pub fn check_protocol(node: &Node, pong: &Value) -> CliResult<()> {
    let theirs = pong["protocol"].as_u64();
    let version = pong["version"].as_str().unwrap_or("unknown");
    let ours = env!("CARGO_PKG_VERSION");
    match theirs {
        Some(p) if p == PROTOCOL => Ok(()),
        Some(p) => {
            let (msg, side) = if p < PROTOCOL {
                (
                    format!(
                        "{} runs tome {version} (protocol {p}); this tome is {ours} (protocol {PROTOCOL})",
                        node.name
                    ),
                    format!("upgrade tome on {}", node.name),
                )
            } else {
                (
                    format!(
                        "{} runs tome {version} (protocol {p}), newer than this tome {ours} (protocol {PROTOCOL})",
                        node.name
                    ),
                    "upgrade tome here".to_string(),
                )
            };
            Err(CliError::internal(msg).with_hint(side))
        }
        None => Err(CliError::internal(format!(
            "{} runs a tome without node support (protocol 0); this tome is {ours} (protocol {PROTOCOL})",
            node.name
        ))
        .with_hint(format!("upgrade tome on {}", node.name))),
    }
}

// --- the relay: `tome rpc --stdio` --------------------------------------

/// Relay newline-delimited JSON-RPC between stdin/stdout and this machine's
/// daemon, starting the daemon first if it isn't running. What a node runs
/// at the other end of `ssh`.
pub fn relay() -> anyhow::Result<()> {
    if crate::lifecycle::probe()?.is_none() {
        crate::lifecycle::start()?;
    }
    let socket = UnixStream::connect(paths::socket_path())?;
    let mut to_daemon = socket.try_clone()?;
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut std::io::stdin().lock(), &mut to_daemon);
        // The caller went away: tell the daemon, as a closed socket would.
        let _ = to_daemon.shutdown(std::net::Shutdown::Write);
    });
    let mut from_daemon = socket;
    let mut out = std::io::stdout().lock();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = match from_daemon.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        if out.write_all(&buf[..n]).and_then(|_| out.flush()).is_err() {
            break;
        }
    }
    Ok(())
}

// --- editing the config: `tome node add|rm` -----------------------------

/// Read the global config as a YAML mapping (empty if there's none).
fn read_global() -> CliResult<(PathBuf, Mapping)> {
    let path = config::global_path();
    let doc = match std::fs::read_to_string(&path) {
        Ok(text) => match serde_yaml::from_str(&text)
            .map_err(|e| CliError::invalid(format!("{}: {e}", path.display())))?
        {
            Yaml::Null => Mapping::new(),
            Yaml::Mapping(m) => m,
            _ => {
                return Err(CliError::invalid(format!(
                    "{}: expected a mapping at the top level",
                    path.display()
                )))
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Mapping::new(),
        Err(e) => {
            return Err(CliError::internal(format!(
                "reading {}: {e}",
                path.display()
            )))
        }
    };
    Ok((path, doc))
}

fn write_global(path: &Path, doc: &Mapping) -> CliResult<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_yaml::to_string(doc).map_err(|e| CliError::internal(e.to_string()))?;
    std::fs::write(path, text)?;
    Ok(())
}

/// Add or replace a node's entry, keeping its `projects`. Returns whether
/// it replaced one.
pub fn write_entry(name: &str, ssh: &str, tome: Option<&str>) -> CliResult<bool> {
    check_name(name).map_err(CliError::invalid)?;
    let (path, mut doc) = read_global()?;
    let nodes = doc
        .entry(Yaml::from("nodes"))
        .or_insert_with(|| Yaml::Mapping(Mapping::new()));
    if nodes.is_null() {
        *nodes = Yaml::Mapping(Mapping::new());
    }
    let nodes = nodes.as_mapping_mut().ok_or_else(|| {
        CliError::invalid(format!("{}: `nodes` must be a mapping", path.display()))
    })?;
    let old = nodes.get(name).and_then(Yaml::as_mapping).cloned();
    let mut entry = Mapping::new();
    entry.insert("ssh".into(), ssh.into());
    if let Some(t) = tome {
        entry.insert("tome".into(), t.into());
    }
    if let Some(p) = old.as_ref().and_then(|o| o.get("projects")) {
        entry.insert("projects".into(), p.clone());
    }
    nodes.insert(name.into(), Yaml::Mapping(entry));
    write_global(&path, &doc)?;
    Ok(old.is_some())
}

/// Remove a node's entry; false if there was none.
pub fn remove_entry(name: &str) -> CliResult<bool> {
    let (path, mut doc) = read_global()?;
    let Some(nodes) = doc.get_mut("nodes").and_then(Yaml::as_mapping_mut) else {
        return Ok(false);
    };
    if nodes.remove(name).is_none() {
        return Ok(false);
    }
    if nodes.is_empty() {
        doc.remove("nodes");
    }
    write_global(&path, &doc)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::ErrorKind;

    #[test]
    fn names_are_checked() {
        assert!(check_name("mini").is_ok());
        assert!(check_name("gpu-box_2").is_ok());
        assert!(check_name("Mini").is_err());
        assert!(check_name("a.b").is_err());
        assert!(check_name("").is_err());
        assert!(check_name("local").unwrap_err().contains("reserved"));
    }

    #[test]
    fn entries_parse() {
        let yaml: Yaml = serde_yaml::from_str(
            "ssh: kyle@mini.local\ntome: ~/.cargo/bin/tome\nprojects:\n  tome-cli: ~/src/tome-cli\n",
        )
        .unwrap();
        let node = parse("mini", &yaml).unwrap();
        assert_eq!(node.ssh, "kyle@mini.local");
        assert_eq!(node.tome(), "~/.cargo/bin/tome");
        assert_eq!(node.projects["tome-cli"], "~/src/tome-cli");

        let bare: Yaml = serde_yaml::from_str("ssh: mini\n").unwrap();
        assert_eq!(parse("mini", &bare).unwrap().tome(), "tome");
        let none: Yaml = serde_yaml::from_str("tome: x\n").unwrap();
        assert!(parse("mini", &none)
            .unwrap_err()
            .contains("`ssh` is required"));
        let typo: Yaml = serde_yaml::from_str("ssh: mini\nprojcts: {}\n").unwrap();
        assert!(parse("mini", &typo).unwrap_err().contains("unknown key"));
    }

    #[test]
    fn run_refs_split_on_the_first_colon() {
        assert_eq!(split_ref("mini:12"), (Some("mini"), "12"));
        assert_eq!(split_ref("mini:12/w1"), (Some("mini"), "12/w1"));
        assert_eq!(split_ref("12"), (None, "12"));
        assert_eq!(split_ref(":12"), (None, ":12"));
    }

    #[test]
    fn local_and_disagreeing_nodes() {
        assert_eq!(resolve(Some("local"), None).unwrap(), None);
        assert_eq!(resolve(None, None).unwrap(), None);
        let err = resolve(Some("a"), Some("b")).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Invalid);
    }

    #[test]
    fn drift_warnings() {
        let a = json!({ "head": "abc1234567", "dirty": false });
        let b = json!({ "head": "def4567890", "dirty": true });
        assert!(drift("mini", "p", Some(&a), Some(&a)).is_empty());
        let w = drift("mini", "p", Some(&a), Some(&b));
        assert_eq!(w[0], "mini's p is at def4567, yours is at abc1234");
        assert!(w[1].contains("uncommitted"));
        assert!(drift("mini", "p", None, Some(&b)).is_empty());
    }

    #[test]
    fn protocol_mismatches_name_the_side_to_upgrade() {
        let node = Node {
            name: "mini".into(),
            ssh: "mini".into(),
            tome: None,
            projects: BTreeMap::new(),
        };
        assert!(check_protocol(&node, &ping()).is_ok());
        let old = check_protocol(&node, &json!({ "pong": true })).unwrap_err();
        assert_eq!(old.hint.as_deref(), Some("upgrade tome on mini"));
        let newer = check_protocol(
            &node,
            &json!({ "protocol": PROTOCOL + 1, "version": "9.0.0" }),
        )
        .unwrap_err();
        assert_eq!(newer.hint.as_deref(), Some("upgrade tome here"));
        assert_eq!(newer.kind.exit_code(), crate::output::exit::FAILURE);
    }
}
