//! Daemon RPC methods backed by the run store. The daemon owns the only
//! database connection; these handlers run with it locked.

use crate::output::{CliError, CliResult};
use crate::gc;
use crate::query;
use crate::store::{self, NewRun, Run, RunFilter, RunStatus, Store};
use crate::workflow::{self, Invalid, Workflow};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const METHODS: &[&str] = &[
    "run.create",
    "run.get",
    "worktree.add",
    "runs.list",
    "runs.show",
    "runs.logs",
    "runs.gc",
    "query",
];

/// `tome runs logs --tail N` reads at most this much from the end of a log.
const LOG_TAIL_MAX_BYTES: u64 = 8 * 1024 * 1024;

pub fn handles(method: &str) -> bool {
    METHODS.contains(&method)
}

pub fn dispatch(store: &mut Store, method: &str, params: &Value) -> CliResult<Value> {
    match method {
        "run.create" => run_create(store, params),
        "run.get" => run_get(store, params),
        "worktree.add" => worktree_add(store, params),
        "runs.list" => runs_list(store, params),
        "runs.show" => runs_show(store, params),
        "runs.logs" => runs_logs(store, params),
        "runs.gc" => gc::collect(store, params),
        "query" => query_sql(store, params),
        _ => Err(CliError::invalid(format!("unknown method `{method}`"))),
    }
}

/// `run.create {workflow_path, project_path?, source?, params?: ["k=v", ...]}`
///
/// Records a running run without launching anything. `run.start` is what
/// `tome run` uses.
fn run_create(store: &mut Store, p: &Value) -> CliResult<Value> {
    let wf = load_workflow(p)?;
    let (run, _) = create_run(store, p, &wf, RunStatus::Running)?;
    Ok(json!(run))
}

/// The workflow a `run.create`/`run.start` request names: `source` if given
/// (what the CLI read), otherwise the file at `workflow_path`. Invalid
/// workflows are `invalid_workflow` errors (exit 2).
pub fn load_workflow(p: &Value) -> CliResult<Workflow> {
    let path = PathBuf::from(req_str(p, "workflow_path")?);
    match p.get("source").and_then(Value::as_str) {
        Some(source) => workflow::parse(&path, source),
        None => workflow::load(&path),
    }
    .map_err(Invalid::into_cli_error)
}

/// Resolve the request's `params` against the workflow and record the run
/// with its resolved snapshot, so later edits to the file only affect new
/// runs. Also returns the rendered body.
pub fn create_run(store: &mut Store, p: &Value, wf: &Workflow, status: RunStatus) -> CliResult<(Run, String)> {
    let args: Vec<String> = match p.get("params") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items.iter().map(|v| v.as_str().unwrap_or_default().to_string()).collect(),
        Some(_) => return Err(CliError::invalid("`params` must be a list of \"key=value\" strings")),
    };
    let overrides = workflow::parse_param_args(&args)?;
    let params = wf.resolve_params(&overrides, true).map_err(Invalid::into_cli_error)?;
    let project = opt_str(p, "project_path").map(PathBuf::from);

    let mut body = String::new();
    let run = store
        .create_run(
            NewRun {
                workflow_name: wf.name(),
                workflow_path: Some(&wf.path),
                project_path: project.as_deref(),
                params: &params,
                status,
            },
            |id| {
                body = wf.render_body(&params, &id.to_string());
                wf.render_snapshot(&params, &id.to_string())
            },
        )
        .map_err(internal)?;
    Ok((run, body))
}

/// `run.get {id}`: the run including its workflow snapshot.
fn run_get(store: &mut Store, p: &Value) -> CliResult<Value> {
    let id = req_id(p)?;
    let run = store
        .get_run(id, true)
        .map_err(|e| CliError::internal(format!("{e:#}")))?
        .ok_or_else(|| CliError::not_found(format!("no run with id {id}")))?;
    Ok(json!(run))
}

/// `worktree.add {run_id, path, repo_path?, branch?}`: record a worktree a run
/// created so gc can clean it up.
fn worktree_add(store: &mut Store, p: &Value) -> CliResult<Value> {
    let run_id = p.get("run_id").and_then(Value::as_i64).ok_or_else(|| CliError::invalid("missing integer `run_id`"))?;
    store.require_run(run_id)?;
    let path = PathBuf::from(req_str(p, "path")?);
    store
        .add_worktree(run_id, &path, opt_str(p, "repo_path").map(Path::new), opt_str(p, "branch"))
        .map_err(|e| CliError::internal(format!("{e:#}")))?;
    Ok(json!({ "run_id": run_id, "path": path }))
}

/// `runs.list {status?, workflow?, limit?}`: newest first.
fn runs_list(store: &mut Store, p: &Value) -> CliResult<Value> {
    let status = match opt_str(p, "status") {
        None => None,
        Some(s) => Some(RunStatus::parse(s).ok_or_else(|| {
            CliError::invalid(format!("invalid status `{s}` (use queued, running, succeeded, failed or cancelled)"))
        })?),
    };
    let filter = RunFilter {
        status,
        workflow: opt_str(p, "workflow").map(str::to_string),
        limit: p.get("limit").and_then(Value::as_u64).map(|n| n as usize),
    };
    Ok(json!({ "runs": store.list_runs(&filter).map_err(internal)? }))
}

/// `runs.show {id, snapshot?}`: the run with its steps, step history,
/// worktrees and (freshly indexed) log files.
fn runs_show(store: &mut Store, p: &Value) -> CliResult<Value> {
    let id = req_id(p)?;
    store.require_run(id)?;
    let snapshot = p.get("snapshot").and_then(Value::as_bool).unwrap_or(false);
    let run = store.get_run(id, snapshot).map_err(internal)?;
    Ok(json!({
        "run": run,
        "steps": store.steps(id).map_err(internal)?,
        "history": store.step_history(id).map_err(internal)?,
        "worktrees": store.worktrees(id).map_err(internal)?,
        "logs": store.index_logs(id).map_err(internal)?,
    }))
}

/// `runs.logs {id, step?, tail?}`: log contents, whole or the last `tail`
/// lines.
fn runs_logs(store: &mut Store, p: &Value) -> CliResult<Value> {
    let id = req_id(p)?;
    store.require_run(id)?;
    let tail = p.get("tail").and_then(Value::as_u64).map(|n| n as usize);
    let mut logs = store.index_logs(id).map_err(internal)?;
    if let Some(step) = opt_str(p, "step") {
        let stem = store::log_file_stem(step);
        logs.retain(|l| l.step == stem);
        if logs.is_empty() {
            return Err(CliError::not_found(format!("run {id} has no log for step `{step}`")));
        }
    }
    let logs: Vec<Value> = logs
        .into_iter()
        .map(|l| {
            let path = Path::new(&l.path);
            let content = match tail {
                Some(n) => store::read_tail_bytes(path, n, LOG_TAIL_MAX_BYTES),
                None => std::fs::read(path).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default(),
            };
            json!({ "step": l.step, "path": l.path, "size": l.size, "content": content })
        })
        .collect();
    Ok(json!({ "logs": logs }))
}

/// `query {sql}`: read-only SQL against the store.
fn query_sql(store: &mut Store, p: &Value) -> CliResult<Value> {
    let result = query::run(store, req_str(p, "sql")?)?;
    Ok(json!({ "columns": result.columns, "rows": result.rows }))
}

pub fn internal(e: anyhow::Error) -> CliError {
    CliError::internal(format!("{e:#}"))
}

pub fn req_id(p: &Value) -> CliResult<i64> {
    req_id_at(p, "id")
}

/// A run id given as a number or a string like `42` / `#42`.
pub fn req_id_at(p: &Value, key: &str) -> CliResult<i64> {
    match p.get(key) {
        Some(Value::Number(n)) => n.as_i64(),
        Some(Value::String(s)) => s.trim().trim_start_matches('#').parse().ok(),
        _ => None,
    }
    .ok_or_else(|| CliError::invalid(format!("missing or invalid run `{key}`")))
}

pub fn req_str<'a>(p: &'a Value, key: &str) -> CliResult<&'a str> {
    p.get(key).and_then(Value::as_str).ok_or_else(|| CliError::invalid(format!("missing string `{key}`")))
}

pub fn opt_str<'a>(p: &'a Value, key: &str) -> Option<&'a str> {
    p.get(key).and_then(Value::as_str)
}
