//! `tome run`, `tome run finish|cancel`, `tome step start|done|fail` and
//! `tome ready`.
//!
//! The run-side commands are what an orchestrator calls. They find their run
//! through `--run <id>` or `TOME_RUN_ID`.

use crate::engine::reason;
use crate::output::{exit, CliError, CliResult, Mode, Report};
use crate::paths;
use crate::rpc;
use crate::workflow::Library;
use chrono::{Local, NaiveDateTime, TimeZone};
use serde_json::{json, Value};
use std::cell::Cell;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

fn call(method: &str, params: Value) -> CliResult<Value> {
    rpc::call(&paths::socket_path(), method, params)
}

/// The run a run-side command applies to.
fn run_id(run: Option<String>) -> CliResult<String> {
    run.filter(|r| !r.trim().is_empty())
        .ok_or_else(|| CliError::invalid("no run given").with_hint("pass the run id, or set TOME_RUN_ID"))
}

/// Resolve a workflow by name or path and ask the daemon to start it. The
/// daemon re-validates it; an invalid workflow is exit `2` and no run is
/// recorded.
pub fn start_params(cwd: &Path, target: &str, params: &[String]) -> CliResult<Value> {
    let wf = Library::discover(cwd).find(target)?;
    Ok(json!({
        "workflow_path": wf.path,
        "source": wf.source,
        "project_path": cwd,
        "params": params,
    }))
}

/// `tome run <wf> --detach`: start the run and return its id right away.
pub fn start_detached(cwd: &Path, target: &str, params: &[String]) -> CliResult<Report> {
    let run = call("run.start", start_params(cwd, target, params)?)?;
    let human = format!("run {} {} ({})", run["id"], run["status"].as_str().unwrap_or("?"), s(&run["workflow_name"]));
    Ok(Report::new(run, human))
}

/// `tome run <wf>`: start the run and stream its events until it finishes,
/// one line per transition (NDJSON in JSON mode). Exits with the run's
/// outcome: `0` succeeded, `1` failed, `130` cancelled.
///
/// Ctrl-C (or SIGTERM/SIGHUP) cancels the run and waits for the daemon to
/// confirm; a second one exits right away. If this process dies instead, the
/// daemon notices the closed connection and cancels the run itself.
pub fn start_attached(cwd: &Path, target: &str, params: &[String], mode: Mode) -> CliResult<Report> {
    let mut start = start_params(cwd, target, params)?;
    start["attach"] = json!(true);
    let mut client = rpc::Client::connect(&paths::socket_path())?;
    install_signal_handlers();

    let run_id: Cell<Option<i64>> = Cell::new(None);
    let mut cancel_sent = false;
    let run = client.stream(
        "run.start",
        start,
        |event| {
            run_id.set(run_id.get().or(event["run_id"].as_i64()));
            print_event(event, mode);
        },
        || {
            match SIGNALS.load(Ordering::SeqCst) {
                0 => {}
                1 => {
                    if let (Some(id), false) = (run_id.get(), cancel_sent) {
                        cancel_sent = true;
                        // Finished in the meantime is fine: its final event is on the way.
                        let _ = call("run.cancel", json!({ "id": id, "reason": reason::INTERRUPTED }));
                    }
                }
                _ => std::process::exit(exit::CANCELLED),
            }
            Ok(())
        },
    )?;
    let code = match run["status"].as_str() {
        Some("succeeded") => exit::OK,
        Some("cancelled") => exit::CANCELLED,
        _ => exit::FAILURE,
    };
    Ok(Report::printed(run, code))
}

/// Signals received while attached.
static SIGNALS: AtomicUsize = AtomicUsize::new(0);

extern "C" fn on_signal(_: libc::c_int) {
    SIGNALS.fetch_add(1, Ordering::SeqCst);
}

fn install_signal_handlers() {
    for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        // SAFETY: the handler only touches an atomic.
        unsafe { libc::signal(sig, on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t) };
    }
}

fn print_event(event: &Value, mode: Mode) {
    let line = match mode {
        Mode::Json => event.to_string(),
        Mode::Human => event_line(event),
    };
    // Nowhere to report a closed stdout; the run carries on regardless.
    let _ = writeln!(std::io::stdout(), "{line}");
}

/// `[14:02:31] Implement: done (tests pass)` / `[14:02:31] run 42 succeeded: shipped`
fn event_line(ev: &Value) -> String {
    let time = ev["time"].as_str().and_then(clock).unwrap_or_default();
    match ev["type"].as_str() {
        Some("step") => {
            let what = match ev["event"].as_str() {
                Some("start") => "started",
                Some("done") => "done",
                _ => "failed",
            };
            let mut line = format!("[{time}] {}: {what}", s(&ev["step"]));
            if let Some(m) = ev["message"].as_str() {
                line.push_str(&format!(" ({m})"));
            }
            line
        }
        // `[14:02:31] worker w1 (g): done (tests pass)`
        Some("worker") => {
            let mut line = format!("[{time}] worker {}", s(&ev["worker"]));
            if let Some(g) = ev["group"].as_str() {
                line.push_str(&format!(" ({g})"));
            }
            line.push_str(&format!(": {}", s(&ev["event"])));
            if let Some(m) = ev["message"].as_str().and_then(|m| m.lines().next()) {
                line.push_str(&format!(" ({m})"));
            }
            line
        }
        // `[14:02:31] orchestrator start: nudged (no tome call in 60s; ...)`
        Some("handshake") => {
            let who = match ev["worker"].as_str() {
                Some(w) => format!("worker {w}"),
                None => "orchestrator".to_string(),
            };
            let mut line = format!("[{time}] {who} start: {}", s(&ev["event"]));
            if let Some(m) = ev["message"].as_str() {
                line.push_str(&format!(" ({m})"));
            }
            line
        }
        // `[14:02:31] trigger: file specs/**/*.md fired (specs/a.md (modified))`
        Some("trigger") => format!("[{time}] trigger: {}", s(&ev["message"])),
        Some("publish") => format!("[{time}] published {}", s(&ev["message"])),
        Some("group") => {
            let mut line = format!("[{time}] group {}: {}", s(&ev["group"]), s(&ev["event"]));
            if let Some(m) = ev["message"].as_str() {
                line.push_str(&format!(" ({m})"));
            }
            line
        }
        _ => {
            let status = s(&ev["status"]);
            let mut line = format!("[{time}] run {} {status}", ev["run_id"]);
            match ev["summary"].as_str().or(ev["reason"].as_str()) {
                Some(detail) if status != "running" && status != "queued" => line.push_str(&format!(": {detail}")),
                _ => line.push_str(&format!(" ({})", s(&ev["workflow"]))),
            }
            line
        }
    }
}

/// A stored UTC timestamp as local `HH:MM:SS`.
fn clock(utc: &str) -> Option<String> {
    let naive = NaiveDateTime::parse_from_str(utc, "%Y-%m-%dT%H:%M:%S%.fZ").ok()?;
    Some(Local.from_utc_datetime(&naive).format("%H:%M:%S").to_string())
}

/// `tome run finish --status succeeded|failed [--summary ...]`
pub fn finish(run: Option<String>, status: &str, summary: Option<String>) -> CliResult<Report> {
    if !matches!(status, "succeeded" | "failed") {
        return Err(CliError::invalid(format!("invalid --status `{status}` (use succeeded or failed)")));
    }
    let id = run_id(run)?;
    let run = call("run.finish", json!({ "id": id, "status": status, "summary": summary }))?;
    Ok(Report::new(run.clone(), finished_line(&run)))
}

/// `tome run cancel [<id>]`
pub fn cancel(run: Option<String>) -> CliResult<Report> {
    let id = run_id(run)?;
    let run = call("run.cancel", json!({ "id": id }))?;
    Ok(Report::new(run.clone(), finished_line(&run)))
}

/// `tome ready`: an agent tome started saying it has. It only works from
/// the agent's session, where `TOME_RUN_ID` (and, for a worker,
/// `TOME_WORKER_ID`) are set.
pub fn ready() -> CliResult<Report> {
    let var = |k| std::env::var(k).ok().filter(|v: &String| !v.trim().is_empty());
    let id = var("TOME_RUN_ID").ok_or_else(|| {
        CliError::invalid("`tome ready` is for agents tome started (TOME_RUN_ID isn't set)")
    })?;
    let ready = call("agent.ready", json!({ "run_id": id, "worker": var("TOME_WORKER_ID") }))?;
    let human = match ready["worker"].as_str() {
        Some(w) => format!("ready: worker {w} of run {id}"),
        None => format!("ready: orchestrator of run {id}"),
    };
    Ok(Report::new(ready, human))
}

/// `tome step start|done|fail [<name>] [--message ...]`
pub fn step(event: &str, name: Option<String>, message: Option<String>, run: Option<String>) -> CliResult<Report> {
    let id = run_id(run)?;
    let step = call("step.report", json!({ "run_id": id, "step": name, "event": event, "message": message }))?;
    let verb = match event {
        "start" => "started",
        "done" => "done",
        _ => "failed",
    };
    let mut human = format!("step \"{}\" {verb}", s(&step["name"]));
    if let Some(m) = step["message"].as_str().filter(|_| event != "start") {
        human.push_str(&format!(": {m}"));
    }
    Ok(Report::new(step, human))
}

fn finished_line(run: &Value) -> String {
    let mut line = format!("run {} {}", run["id"], s(&run["status"]));
    if let Some(detail) = run["summary"].as_str().or(run["reason"].as_str()) {
        line.push_str(&format!(": {detail}"));
    }
    line
}

fn s(v: &Value) -> String {
    v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())
}
