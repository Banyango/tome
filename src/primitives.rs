//! `tome worker|group|worktree|queue`: what an orchestrator (and its
//! workers) call to delegate work. They find their run through `--run <id>`
//! or `TOME_RUN_ID`, and a worker is known by `TOME_WORKER_ID`.

use crate::output::{exit, table, CliError, CliResult, Report};
use crate::{duration, paths, rpc};
use serde_json::{json, Value};
use std::io::Read;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How often the `wait` forms ask the daemon again.
const WAIT_POLL: Duration = Duration::from_millis(250);

fn call(method: &str, params: Value) -> CliResult<Value> {
    rpc::call(&paths::socket_path(), method, params)
}

fn run_id(run: Option<String>) -> CliResult<String> {
    run.filter(|r| !r.trim().is_empty()).ok_or_else(|| {
        CliError::invalid("no run given").with_hint("pass --run <id>, or set TOME_RUN_ID")
    })
}

/// The worker this process is, if it's one.
fn me() -> Option<String> {
    std::env::var("TOME_WORKER_ID")
        .ok()
        .filter(|s| !s.is_empty())
}

pub struct Spawn {
    pub run: Option<String>,
    pub name: Option<String>,
    pub group: Option<String>,
    pub worktree: bool,
    pub base: Option<String>,
    pub branch: Option<String>,
    pub harness: Option<String>,
    pub keep_open: bool,
    pub placement: crate::placement::Settings,
    pub prompt: Option<String>,
    pub prompt_file: Option<PathBuf>,
    pub command: Vec<String>,
}

/// `tome worker spawn ...`
pub fn spawn(a: Spawn) -> CliResult<Report> {
    let run = run_id(a.run)?;
    let prompt = match (a.prompt, a.prompt_file) {
        (Some(_), Some(_)) => {
            return Err(CliError::invalid(
                "give --prompt or --prompt-file, not both",
            ))
        }
        (Some(p), None) => Some(p),
        (None, Some(path)) => Some(std::fs::read_to_string(&path).map_err(|e| {
            CliError::invalid(format!("can't read prompt file {}: {e}", path.display()))
        })?),
        (None, None) => None,
    };
    if prompt.is_some() && !a.command.is_empty() {
        return Err(CliError::invalid(
            "give a worker a task (--prompt) or a command (after `--`), not both",
        ));
    }
    let w = call(
        "worker.spawn",
        json!({
            "run_id": run,
            "name": a.name,
            "group": a.group,
            "worktree": a.worktree,
            "base": a.base,
            "branch": a.branch,
            "harness": a.harness,
            "keep_open": a.keep_open,
            "placement": (!a.placement.is_empty()).then_some(&a.placement),
            "prompt": prompt,
            "command": a.command,
            "caller": me(),
            "cwd": std::env::current_dir().ok(),
        }),
    )?;
    let mut human = format!("{}", s(&w["name"]));
    if let Some(wt) = w["worktree"].as_str() {
        human.push_str(&format!("\nworktree {wt} (branch {})", s(&w["branch"])));
    }
    Ok(Report::new(w, human))
}

/// `tome session move <run>/<name> [placement flags]`
pub fn session_move(session: &str, placement: &crate::placement::Settings) -> CliResult<Report> {
    let (run, name) = match session.split_once('/') {
        Some((run, name)) => (run.to_string(), name),
        None => (
            run_id(std::env::var("TOME_RUN_ID").ok())
                .map_err(|e| e.with_hint("give the session as <run>/<name>"))?,
            session,
        ),
    };
    if name.is_empty() {
        return Err(CliError::invalid(
            "which session? give it as <run>/<name>, e.g. 42/orchestrator or 42/w1",
        ));
    }
    let mut params = json!({ "run_id": run, "name": name, "placement": placement });
    if let Some(caller) = crate::session::caller_env() {
        params["cmux_caller"] = caller;
    }
    let moved = call("session.move", params)?;
    let p = &moved["placement"];
    let mut human = format!("moved {name}: {}", s(&p["layout"]));
    match (p["caller"].as_str(), p["workspace"].as_str()) {
        (Some(at), _) => human.push_str(&format!(" next to {at}")),
        (None, Some(w)) => human.push_str(&format!(" in workspace {w}")),
        (None, None) => {}
    }
    for w in p["warnings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        human.push_str(&format!("\nwarning: {w}"));
    }
    human.push_str(&format!("\nattach: {}", s(&moved["attach_command"])));
    Ok(Report::new(moved, human))
}

/// `tome worker done|fail [--summary ...]`
pub fn report(
    event: &str,
    summary: Option<String>,
    name: Option<String>,
    run: Option<String>,
) -> CliResult<Report> {
    let run = run_id(run)?;
    let w = call(
        "worker.report",
        json!({ "run_id": run, "name": name, "caller": me(), "event": event, "summary": summary }),
    )?;
    let human = worker_line(&w);
    Ok(Report::new(w, human))
}

/// `tome worker status [<name>]`, or with `wait`, block until it finishes.
pub fn worker_status(name: Option<String>, wait: bool, run: Option<String>) -> CliResult<Report> {
    let run = run_id(run)?;
    let Some(name) = name else {
        let all = call("worker.status", json!({ "run_id": run }))?;
        let human = workers_table(
            all["workers"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default(),
        );
        return Ok(Report::new(all, human));
    };
    loop {
        let w = call("worker.status", json!({ "run_id": run, "name": name }))?;
        if !wait || is_final(&w["status"]) {
            let human = worker_detail(&w);
            return Ok(Report::new(w, human));
        }
        std::thread::sleep(WAIT_POLL);
    }
}

/// `tome worker kill <name>`
pub fn kill(name: String, run: Option<String>) -> CliResult<Report> {
    let run = run_id(run)?;
    let w = call("worker.kill", json!({ "run_id": run, "name": name }))?;
    let human = worker_line(&w);
    Ok(Report::new(w, human))
}

/// `tome group create <g> [--fail-fast]`
pub fn group_create(name: String, fail_fast: bool, run: Option<String>) -> CliResult<Report> {
    let run = run_id(run)?;
    let g = call(
        "group.create",
        json!({ "run_id": run, "name": name, "fail_fast": fail_fast }),
    )?;
    let human = format!(
        "group {} created{}",
        s(&g["name"]),
        if fail_fast { " (fail-fast)" } else { "" }
    );
    Ok(Report::new(g, human))
}

/// `tome group close|status|wait <g>`
pub fn group(method: &str, name: String, wait: bool, run: Option<String>) -> CliResult<Report> {
    let run = run_id(run)?;
    loop {
        let g = call(method, json!({ "run_id": run, "name": name, "wait": wait }))?;
        if !wait || g["group"]["status"] == "finished" {
            let human = format!(
                "group {} {}\n{}",
                s(&g["group"]["name"]),
                s(&g["group"]["status"]),
                workers_table(
                    g["workers"]
                        .as_array()
                        .map(Vec::as_slice)
                        .unwrap_or_default()
                )
            );
            return Ok(Report::new(g, human));
        }
        std::thread::sleep(WAIT_POLL);
    }
}

/// `tome worktree create <name> [--base <ref>] [--branch <name>]`
pub fn worktree_create(
    name: String,
    base: Option<String>,
    branch: Option<String>,
    run: Option<String>,
) -> CliResult<Report> {
    let run = run_id(run)?;
    let wt = call(
        "worktree.create",
        json!({ "run_id": run, "name": name, "base": base, "branch": branch, "cwd": std::env::current_dir().ok() }),
    )?;
    let human = format!(
        "{} (branch {}, from {})",
        s(&wt["path"]),
        s(&wt["branch"]),
        s(&wt["base"])
    );
    Ok(Report::new(wt, human))
}

/// `tome queue push <q> <text|->`
pub fn push(queue: String, text: String, run: Option<String>) -> CliResult<Report> {
    let run = run_id(run)?;
    let body = if text == "-" {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        buf
    } else {
        text
    };
    let m = call(
        "queue.push",
        json!({ "run_id": run, "queue": queue, "body": body, "caller": me() }),
    )?;
    let human = format!("message {} on {}", m["id"], s(&m["queue"]));
    Ok(Report::new(m, human))
}

/// `tome queue pull <q> [--wait [<dur>]]`: exit `3` when there's nothing to
/// claim (`status` is `empty`, or `closed` when nothing more will come).
pub fn pull(queue: String, wait: Option<String>, run: Option<String>) -> CliResult<Report> {
    let run = run_id(run)?;
    let deadline = match wait.as_deref() {
        None => Some(Instant::now()),
        Some("forever") => None,
        Some(d) => Some(
            Instant::now()
                + duration::parse(d)
                    .map_err(|e| CliError::invalid(format!("invalid --wait: {e}")))?,
        ),
    };
    loop {
        let r = call(
            "queue.pull",
            json!({ "run_id": run, "queue": queue, "caller": me() }),
        )?;
        match r["status"].as_str() {
            Some("message") => {
                let human = format!("{}\t{}", r["message"]["id"], s(&r["message"]["body"]));
                return Ok(Report::new(r, human));
            }
            Some("closed") => {
                return Ok(Report::new(r, format!("queue {queue} is closed and empty"))
                    .with_exit(exit::EMPTY));
            }
            _ if deadline.is_some_and(|d| Instant::now() >= d) => {
                return Ok(Report::new(r, format!("queue {queue} is empty")).with_exit(exit::EMPTY));
            }
            _ => std::thread::sleep(WAIT_POLL),
        }
    }
}

/// `tome queue ack <id>`
pub fn ack(id: String, run: Option<String>) -> CliResult<Report> {
    let run = run_id(run)?;
    let m = call("queue.ack", json!({ "run_id": run, "id": id }))?;
    let human = format!("message {} acked", m["id"]);
    Ok(Report::new(m, human))
}

/// `tome queue close <q>`
pub fn close(queue: String, run: Option<String>) -> CliResult<Report> {
    let run = run_id(run)?;
    let q = call("queue.close", json!({ "run_id": run, "queue": queue }))?;
    let human = format!("queue {} closed ({} pending)", s(&q["name"]), q["pending"]);
    Ok(Report::new(q, human))
}

/// `tome queue ls`
pub fn ls(run: Option<String>) -> CliResult<Report> {
    let run = run_id(run)?;
    let all = call("queue.ls", json!({ "run_id": run }))?;
    let rows = all["queues"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|q| {
            vec![
                s(&q["name"]),
                if q["closed"] == true {
                    "closed"
                } else {
                    "open"
                }
                .to_string(),
                q["pending"].to_string(),
                q["claimed"].to_string(),
            ]
        })
        .collect();
    let human = table(&["QUEUE", "STATE", "PENDING", "CLAIMED"], rows);
    Ok(Report::new(all, human))
}

fn is_final(status: &Value) -> bool {
    matches!(status.as_str(), Some("done" | "failed" | "cancelled"))
}

fn worker_line(w: &Value) -> String {
    let mut line = format!("worker {} {}", s(&w["name"]), s(&w["status"]));
    if let Some(detail) = w["summary"].as_str().or(w["reason"].as_str()) {
        line.push_str(&format!(": {detail}"));
    }
    line
}

fn worker_detail(w: &Value) -> String {
    let mut out = worker_line(w);
    for (label, key) in [
        ("worktree", "worktree"),
        ("branch", "branch"),
        ("session", "session"),
    ] {
        if let Some(v) = w[key].as_str() {
            out.push_str(&format!("\n  {label}: {v}"));
        }
    }
    out
}

pub fn workers_table(workers: &[Value]) -> String {
    let rows = workers
        .iter()
        .map(|w| {
            vec![
                s(&w["name"]),
                s(&w["kind"]),
                s(&w["status"]),
                w["group"].as_str().unwrap_or("-").to_string(),
                w["branch"].as_str().unwrap_or("-").to_string(),
                w["summary"]
                    .as_str()
                    .or(w["reason"].as_str())
                    .unwrap_or("")
                    .lines()
                    .next()
                    .unwrap_or("")
                    .to_string(),
            ]
        })
        .collect();
    table(
        &["WORKER", "KIND", "STATUS", "GROUP", "BRANCH", "SUMMARY"],
        rows,
    )
}

fn s(v: &Value) -> String {
    v.as_str()
        .map(str::to_string)
        .unwrap_or_else(|| v.to_string())
}
