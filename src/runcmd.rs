//! `tome run`, `tome run finish|cancel` and `tome step start|done|fail`.
//!
//! The run-side commands are what an orchestrator calls. They find their run
//! through `--run <id>` or `TOME_RUN_ID`.

use crate::output::{CliError, CliResult, Report};
use crate::paths;
use crate::rpc;
use crate::workflow::Library;
use serde_json::{json, Value};
use std::path::Path;

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
