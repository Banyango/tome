//! `tome node add|rm|ls|check`, `tome runs ls --nodes`, `tome session view`
//! and `tome run --view`.

use crate::config::Config;
use crate::node::{self, Node};
use crate::output::{exit, table, CliError, CliResult, ErrorKind, Report};
use crate::paths;
use crate::placement::{self, Settings};
use crate::rpc;
use crate::session::{self, Kind, Layout, Split, Target};
use serde_json::{json, Value};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

fn s(v: &Value) -> String {
    match v {
        Value::Null => "-".into(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

// --- tome node add|rm ---------------------------------------------------

/// `tome node add <name> <ssh> [--tome <path>]`: write the entry, then check
/// it. The entry stays even if the check fails, so it can be fixed in place.
pub fn add(name: &str, ssh: &str, tome: Option<&str>) -> CliResult<Report> {
    let replaced = node::write_entry(name, ssh, tome)?;
    let checked = check(name)?;
    let verb = if replaced { "updated" } else { "added" };
    let mut human = format!(
        "{verb} node {name} ({ssh}) in {}\n\n{}",
        crate::config::global_path().display(),
        checked.human
    );
    if checked.exit_code != exit::OK {
        human.push_str(&format!(
            "\n\nthe entry is kept: fix the failure above, then run `tome node check {name}`"
        ));
    }
    Ok(Report::new(
        json!({ "node": name, "added": !replaced, "replaced": replaced, "check": checked.data }),
        human,
    )
    .with_exit(checked.exit_code))
}

/// `tome node rm <name>`
pub fn rm(name: &str) -> CliResult<Report> {
    if !node::remove_entry(name)? {
        return Err(CliError::not_found(format!("no node named `{name}`"))
            .with_hint("list them with `tome node ls`"));
    }
    Ok(Report::new(
        json!({ "node": name, "removed": true }),
        format!("removed node {name}"),
    ))
}

// --- tome node check ----------------------------------------------------

/// One step of a check.
struct Step {
    name: &'static str,
    detail: String,
    /// Set on the step that failed.
    error: Option<CliError>,
}

/// Run `cmd` on the node over plain ssh: its exit code, stdout and stderr.
fn ssh_run(node: &Node, cmd: &str) -> CliResult<(Option<i32>, String, String)> {
    let out = node
        .ssh_command(&[])
        .arg(cmd)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| {
            CliError::new(
                ErrorKind::Unreachable,
                format!("can't run `{}`: {e}", node::ssh_bin()),
            )
            .with_hint("install an OpenSSH client, or put `ssh` on the PATH")
        })?;
    Ok((
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
        String::from_utf8_lossy(&out.stderr).trim().to_string(),
    ))
}

/// The fix for an ssh that wouldn't connect, from what it said.
fn ssh_fix(node: &Node, stderr: &str) -> String {
    let dest = &node.ssh;
    if stderr.contains("Permission denied") {
        format!("set up key login: `ssh-copy-id {dest}`, and load the key into ssh-agent or name it with `IdentityFile` in ~/.ssh/config")
    } else if stderr.contains("Host key verification failed") {
        format!("connect once by hand to trust its host key: `ssh {dest}`")
    } else if stderr.contains("Could not resolve") {
        format!("check the address `{dest}`, or add a Host entry for it to ~/.ssh/config")
    } else {
        format!("`ssh {dest} true` must work without prompting: check the address, that sshd is on, and your keys")
    }
}

/// The steps of `tome node check`, stopping at the first failure.
fn check_steps(node: &Node, cwd: Option<&Path>) -> Vec<Step> {
    let mut steps = Vec::new();
    let fail = |steps: &mut Vec<Step>, name, e: CliError| {
        steps.push(Step {
            name,
            detail: e.message.clone(),
            error: Some(e),
        });
    };

    // 1. ssh connects without prompting.
    match ssh_run(node, "true") {
        Ok((Some(0), _, _)) => steps.push(Step {
            name: "ssh",
            detail: format!("{} connects without prompting", node.ssh),
            error: None,
        }),
        Ok((code, _, err)) => {
            let last = err.lines().last().unwrap_or("ssh failed").to_string();
            let why = match code {
                Some(c) => format!("{last} (exit {c})"),
                None => last,
            };
            let e = CliError::new(
                ErrorKind::Unreachable,
                format!("can't connect to {}: {why}", node.ssh),
            )
            .with_hint(ssh_fix(node, &err));
            return {
                fail(&mut steps, "ssh", e);
                steps
            };
        }
        Err(e) => {
            fail(&mut steps, "ssh", e);
            return steps;
        }
    }

    // 2. tome is found.
    match ssh_run(node, &format!("{} --version", node.tome())) {
        Ok((Some(0), out, _)) => steps.push(Step {
            name: "tome",
            detail: format!("{} ({})", out, node.tome()),
            error: None,
        }),
        Ok((code, _, err)) => {
            let e = crate::rpc::classify_ssh(node, code, &err);
            fail(&mut steps, "tome", e);
            return steps;
        }
        Err(e) => {
            fail(&mut steps, "tome", e);
            return steps;
        }
    }

    // 3. The protocols match (connecting pings).
    let mut client = match rpc::Client::to_node(node) {
        Ok(c) => {
            steps.push(Step {
                name: "protocol",
                detail: format!("protocol {} on both sides", node::PROTOCOL),
                error: None,
            });
            c
        }
        Err(e) => {
            fail(&mut steps, "protocol", e);
            return steps;
        }
    };

    // 4. The daemon runs (the relay starts it if it wasn't).
    match client.call("daemon.status", json!({})) {
        Ok(status) => {
            let detail = if status["service"] == true {
                format!("running (pid {}, a login service)", status["pid"])
            } else {
                format!(
                    "running (pid {}); tip: run `tome daemon install` on {} so it starts at login",
                    status["pid"], node.name
                )
            };
            steps.push(Step {
                name: "daemon",
                detail,
                error: None,
            });
        }
        Err(e) => {
            fail(&mut steps, "daemon", e);
            return steps;
        }
    }

    // 5. Inside a project, the node has a copy of it.
    if let Some(local) = cwd.and_then(crate::triggerscmd::project_of) {
        match node::locate(&mut client, node, &local, false) {
            Ok(found) => steps.push(Step {
                name: "project",
                detail: format!("{} → {}", local.display(), found.path),
                error: None,
            }),
            Err(e) => fail(&mut steps, "project", e),
        }
    }
    steps
}

/// `tome node check <name>`: each step in turn; the first that fails says
/// how to fix it. Exits with that failure's code.
pub fn check(name: &str) -> CliResult<Report> {
    let node = node::get(name)?;
    let cwd = std::env::current_dir().ok();
    let steps = check_steps(&node, cwd.as_deref());
    let failed = steps.iter().find_map(|s| s.error.as_ref());
    let mut human = format!("node {} ({})\n", node.name, node.ssh);
    for st in &steps {
        let mark = if st.error.is_some() { "✗" } else { "✓" };
        human.push_str(&format!("  {mark} {:<9}{}\n", st.name, st.detail));
        if let Some(hint) = st.error.as_ref().and_then(|e| e.hint.as_deref()) {
            human.push_str(&format!("    fix: {hint}\n"));
        }
    }
    let ok = failed.is_none();
    if ok {
        human.push_str(&format!(
            "{} is ready: try `tome runs ls --on {}`",
            node.name, node.name
        ));
    }
    let data = json!({
        "node": node.name,
        "ssh": node.ssh,
        "ok": ok,
        "steps": steps
            .iter()
            .map(|st| json!({
                "step": st.name,
                "ok": st.error.is_none(),
                "detail": st.detail,
                "fix": st.error.as_ref().and_then(|e| e.hint.clone()),
            }))
            .collect::<Vec<_>>(),
    });
    let code = failed.map_or(exit::OK, |e| e.kind.exit_code());
    Ok(Report::new(data, human.trim_end()).with_exit(code))
}

// --- tome node ls -------------------------------------------------------

/// `tome node ls`: every node, checked in parallel. Always exits 0.
pub fn ls() -> CliResult<Report> {
    let nodes = node::all()?;
    if nodes.is_empty() {
        return Ok(Report::new(
            json!({ "nodes": [] }),
            "no nodes are configured\nhint: add one with `tome node add <name> <ssh-destination>`",
        ));
    }
    let handles: Vec<_> = nodes
        .into_values()
        .map(|n| std::thread::spawn(move || probe(&n)))
        .collect();
    let rows: Vec<Value> = handles.into_iter().filter_map(|h| h.join().ok()).collect();
    let human = table(
        &[
            "NODE",
            "SSH",
            "REACHABLE",
            "VERSION",
            "DAEMON",
            "RTT",
            "NOTE",
        ],
        rows.iter()
            .map(|r| {
                vec![
                    s(&r["node"]),
                    s(&r["ssh"]),
                    if r["reachable"] == true { "yes" } else { "no" }.to_string(),
                    s(&r["version"]),
                    s(&r["daemon"]),
                    r["rtt_ms"]
                        .as_u64()
                        .map_or("-".to_string(), |ms| format!("{ms}ms")),
                    r["error"].as_str().unwrap_or("").to_string(),
                ]
            })
            .collect(),
    );
    Ok(Report::new(json!({ "nodes": rows }), human))
}

/// One node's line in `tome node ls`.
fn probe(node: &Node) -> Value {
    let mut row = json!({
        "node": node.name,
        "ssh": node.ssh,
        "reachable": false,
        "version": null,
        "daemon": null,
        "rtt_ms": null,
        "error": null,
    });
    let status = rpc::Client::to_node(node).and_then(|mut c| {
        row["reachable"] = json!(true);
        if let Some(p) = &c.pong {
            row["version"] = p["version"].clone();
        }
        row["rtt_ms"] = json!(c.rtt.map(|d| d.as_millis() as u64));
        c.call("daemon.status", json!({}))
    });
    match status {
        Ok(st) => {
            row["daemon"] = json!(if st["service"] == true {
                "running (service)"
            } else {
                "running"
            });
        }
        Err(e) => {
            if e.kind != ErrorKind::Unreachable {
                row["reachable"] = json!(true);
            }
            row["error"] = json!(e.message);
        }
    }
    row
}

// --- tome runs ls --nodes -----------------------------------------------

/// `tome runs ls --nodes`: this machine's runs and every node's, queried in
/// parallel, each with its node. `limit` is per node. A node that can't be
/// asked is a warning on stderr, not a failure.
pub fn runs_everywhere(
    status: Option<String>,
    workflow: Option<String>,
    limit: usize,
) -> CliResult<Report> {
    let params = json!({ "status": status, "workflow": workflow, "limit": limit });
    let nodes = node::all()?;
    let local = {
        let params = params.clone();
        std::thread::spawn(move || {
            rpc::Client::local(&paths::socket_path()).and_then(|mut c| c.call("runs.list", params))
        })
    };
    let remote: Vec<_> = nodes
        .into_values()
        .map(|n| {
            let params = params.clone();
            std::thread::spawn(move || {
                let got = rpc::Client::to_node(&n).and_then(|mut c| c.call("runs.list", params));
                (n.name, got)
            })
        })
        .collect();
    let mut answers = vec![(
        node::LOCAL.to_string(),
        local
            .join()
            .unwrap_or_else(|_| Err(CliError::internal("listing panicked"))),
    )];
    answers.extend(remote.into_iter().filter_map(|h| h.join().ok()));

    let mut runs = Vec::new();
    let mut warnings = Vec::new();
    for (name, got) in answers {
        match got {
            Ok(data) => {
                for mut r in data["runs"].as_array().cloned().unwrap_or_default() {
                    r["node"] = json!(name);
                    runs.push(r);
                }
            }
            Err(e) => {
                eprintln!("warning: can't list runs on {name}: {}", e.message);
                warnings.push(json!({ "node": name, "error": e.message, "kind": e.kind.as_str() }));
            }
        }
    }
    let human = if runs.is_empty() {
        "no runs".to_string()
    } else {
        table(
            &["NODE", "ID", "WORKFLOW", "STATUS", "STARTED", "FINISHED"],
            runs.iter()
                .map(|r| {
                    vec![
                        s(&r["node"]),
                        s(&r["id"]),
                        s(&r["workflow_name"]),
                        s(&r["status"]),
                        crate::inspect::ts(&r["created_at"]),
                        crate::inspect::ts(&r["finished_at"]),
                    ]
                })
                .collect(),
        )
    };
    Ok(Report::new(
        json!({ "runs": runs, "warnings": warnings }),
        human,
    ))
}

// --- tome session view --------------------------------------------------

/// `<run>` or `<run>/<worker>`.
fn split_session(r: &str) -> CliResult<(i64, Option<&str>)> {
    let (run, worker) = match r.split_once('/') {
        Some((run, w)) if !w.is_empty() => (run, Some(w)),
        Some((run, _)) => (run, None),
        None => (r, None),
    };
    let id = run.parse::<i64>().map_err(|_| {
        CliError::invalid(format!(
            "`{r}` isn't a run: give `<run>` or `<run>/<worker>`"
        ))
    })?;
    Ok((id, worker))
}

/// `tome session view <run>[/<worker>]`: show a run's session from here. A
/// node's tmux session opens in a new cmux tab (from a cmux pane) or in this
/// terminal; a local one is focused (cmux) or its attach command printed.
pub fn view(session_ref: &str, placement: &Settings) -> CliResult<Report> {
    let (run_id, worker) = split_session(session_ref)?;
    let att = rpc::call(
        &paths::socket_path(),
        "session.attach_command",
        json!({ "run_id": run_id, "worker": worker }),
    )?;
    let command = s(&att["command"]);
    let backend = Kind::parse(att["backend"].as_str().unwrap_or(""));
    let label = match worker {
        Some(w) => format!("run {run_id}{} / {w}", node::on_suffix()),
        None => format!("run {run_id}{}", node::on_suffix()),
    };
    let Some(node) = node::target() else {
        return Ok(match backend {
            Some(Kind::Cmux) => {
                let ok = std::process::Command::new("sh")
                    .args(["-c", &command])
                    .stdin(Stdio::null())
                    .status()
                    .is_ok_and(|st| st.success());
                if !ok {
                    return Err(CliError::internal(format!("cmux couldn't focus {label}"))
                        .with_hint(format!("try it by hand: {command}")));
                }
                Report::new(att, format!("focused {label}"))
            }
            _ => Report::new(att, format!("{label}: attach with\n  {command}")),
        });
    };
    if backend != Some(Kind::Tmux) {
        return Err(CliError::invalid(format!(
            "{label} is a cmux session, which can't be attached over SSH"
        ))
        .with_hint(format!(
            "set `backend: tmux` in ~/.tome/config.yaml on {} for runs you want to watch from here",
            node.name
        )));
    }
    let mut ssh = node.ssh_command(&["-t"]);
    ssh.arg(&command);
    let argv: Vec<String> = std::iter::once(ssh.get_program())
        .chain(ssh.get_args())
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    match session::caller_env() {
        Some(caller) => {
            let title = format!("{label} (view)");
            open_tab(&argv, &title, caller, placement)?;
            Ok(Report::new(
                json!({ "run_id": run_id, "node": node.name, "command": argv, "opened": "cmux" }),
                format!("opened {label} in a new tab"),
            ))
        }
        None => {
            if !placement.is_empty() {
                eprintln!("warning: placement flags need a cmux pane; attaching here");
            }
            let err = std::process::Command::new(&argv[0]).args(&argv[1..]).exec();
            Err(CliError::internal(format!("couldn't run ssh: {err}")))
        }
    }
}

/// Open `argv` in a new local cmux terminal: a tab next to the calling pane
/// unless the placement flags say otherwise.
fn open_tab(argv: &[String], title: &str, caller: Value, flags: &Settings) -> CliResult<()> {
    let cwd = std::env::current_dir()?;
    let project = crate::triggerscmd::project_of(&cwd);
    let mut settings = match &flags.preset {
        Some(name) => {
            let cfg = Config::load(project.as_deref())?;
            let presets = placement::presets(&cfg)?;
            presets
                .get(name)
                .map(|p| p.settings.clone())
                .ok_or_else(|| CliError::invalid(format!("unknown layout preset `{name}`")))?
        }
        None => Settings::default(),
    };
    macro_rules! over {
        ($($f:ident),*) => { $( if flags.$f.is_some() { settings.$f = flags.$f.clone(); } )* };
    }
    over!(layout, workspace, direction, size);

    let surface = caller["surface"].as_str().unwrap_or_default();
    let anchor = session::caller_anchor(surface).ok().map(|(a, _)| a);
    let mut layout = settings.layout.unwrap_or(Layout::Tab);
    let target = match &settings.workspace {
        None => anchor
            .as_ref()
            .map(|a| Target::Caller(a.handle.clone()))
            .unwrap_or_default(),
        Some(placement::Workspace::Project) => Target::Project,
        Some(placement::Workspace::Named(n)) => Target::Named(n.clone()),
        Some(placement::Workspace::Focused) => session::focused(Kind::Cmux)
            .map(Target::Focused)
            .unwrap_or_default(),
        Some(placement::Workspace::Own) => {
            layout = Layout::Workspace;
            Target::Project
        }
    };
    let split = Split {
        direction: settings.direction.unwrap_or(placement::Direction::Right),
        size: settings.size,
        anchors: anchor.into_iter().collect(),
        ..Split::default()
    };
    let dir = paths::tome_home().join("views");
    std::fs::create_dir_all(&dir)?;
    let stamp = std::process::id();
    let backend = session::Backend::new(Kind::Cmux);
    backend.launch(&session::Launch {
        name: title,
        title,
        cwd: &cwd,
        argv,
        env: &[],
        script: &dir.join(format!("view-{stamp}.sh")),
        log: &dir.join(format!("view-{stamp}.log")),
        layout,
        split: &split,
        target: &target,
        project: project.as_deref(),
    })?;
    Ok(())
}

/// How long `tome run --view` waits for the run's agent to start.
const VIEW_WAIT: Duration = Duration::from_secs(300);

/// `tome run <wf> --view`: start detached, wait for the agent's handshake,
/// then view its session.
pub fn run_and_view(
    cwd: &Path,
    workflow: &str,
    params: &[String],
    placement: &Settings,
) -> CliResult<Report> {
    let started = crate::runcmd::start_detached(cwd, workflow, params, placement)?;
    let id = started.data["id"]
        .as_i64()
        .ok_or_else(|| CliError::internal("the daemon didn't say which run it started"))?;
    eprintln!("{}", started.human);
    let deadline = Instant::now() + VIEW_WAIT;
    let mut client = rpc::Client::connect(&paths::socket_path())?;
    loop {
        let shown = client.call("runs.show", json!({ "id": id }))?;
        let status = shown["run"]["status"].as_str().unwrap_or("");
        if !matches!(status, "queued" | "running") {
            return Err(CliError::new(
                ErrorKind::Internal,
                format!(
                    "run {id}{} {status} before its agent started",
                    node::on_suffix()
                ),
            )
            .with_hint(format!("see why: `tome runs show {}`", run_ref(id))));
        }
        let main = shown["handshake"]
            .as_array()
            .into_iter()
            .flatten()
            .rfind(|h| h["worker"].is_null())
            .and_then(|h| h["state"].as_str().map(str::to_string));
        if main.as_deref() == Some(crate::handshake::state::READY) {
            break;
        }
        if Instant::now() >= deadline {
            return Err(CliError::internal(format!(
                "run {id}{}'s agent hasn't started yet",
                node::on_suffix()
            ))
            .with_hint(format!(
                "view it later: `tome session view {}`",
                run_ref(id)
            )));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    drop(client);
    view(&id.to_string(), &Settings::default())
}

/// How to refer to run `id` from here: `mini:12` on a node.
fn run_ref(id: i64) -> String {
    match node::target() {
        Some(n) => format!("{}:{id}", n.name),
        None => id.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_refs() {
        assert_eq!(split_session("12").unwrap(), (12, None));
        assert_eq!(split_session("12/w1").unwrap(), (12, Some("w1")));
        assert!(split_session("w1").is_err());
    }

    #[test]
    fn ssh_fixes_follow_what_ssh_said() {
        let node = Node {
            name: "mini".into(),
            ssh: "kyle@mini".into(),
            tome: None,
            projects: Default::default(),
        };
        assert!(ssh_fix(&node, "kyle@mini: Permission denied (publickey).").contains("ssh-copy-id"));
        assert!(ssh_fix(&node, "Host key verification failed.").contains("trust its host key"));
    }
}
