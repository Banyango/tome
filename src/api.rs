//! Daemon RPC methods backed by the run store. The daemon owns the only
//! database connection; these handlers run with it locked.

use crate::gc;
use crate::output::{CliError, CliResult};
use crate::placement;
use crate::query;
use crate::store::{self, NewRun, Run, RunFilter, RunStatus, Store};
use crate::workflow::{self, Invalid, Workflow};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

const METHODS: &[&str] = &[
    "run.create",
    "run.get",
    "worktree.add",
    "runs.list",
    "runs.live",
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
        "runs.live" => runs_live(store, params),
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
    let req = StartRequest::from_json(p)?;
    let wf = req.load_workflow()?;
    let (run, _) = create_run(store, &req, &wf, RunStatus::Running, None)?;
    Ok(json!(run))
}

/// A decoded `run.create`/`run.start` request. Everything is checked here,
/// before anything is recorded or launched.
#[derive(Debug, Clone, Default)]
pub struct StartRequest {
    pub workflow_path: PathBuf,
    /// The workflow file's text as the caller read it; the file at
    /// `workflow_path` is read when it's missing.
    pub source: Option<String>,
    pub project_path: Option<PathBuf>,
    /// `--param key=value` overrides.
    pub params: Vec<(String, String)>,
    /// `--harness`/`--model`: replace the workflow's defaults for this run.
    pub harness: Option<String>,
    pub model: Option<String>,
    /// `tome run`'s placement flags.
    pub placement: Option<placement::Settings>,
    /// The cmux or herdr pane that ran `tome run` (`session::caller_env`).
    pub cmux_caller: Option<Value>,
    /// The `{{trigger.*}}` fields of the event that started the run.
    pub trigger: Map<String, Value>,
    /// What started the run, if a trigger did; recorded with it.
    pub cause: Option<Value>,
    /// The topic-trigger delivery the run holds.
    pub delivery_id: Option<i64>,
    /// Cancel an attached run when its caller disconnects.
    pub cancel_on_disconnect: bool,
}

impl StartRequest {
    pub fn new(workflow_path: PathBuf) -> StartRequest {
        StartRequest {
            workflow_path,
            cancel_on_disconnect: true,
            ..Default::default()
        }
    }

    pub fn from_json(p: &Value) -> CliResult<StartRequest> {
        let obj = |key: &str| -> CliResult<Option<&Map<String, Value>>> {
            match p.get(key) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::Object(o)) => Ok(Some(o)),
                Some(_) => Err(CliError::invalid(format!("`{key}` must be an object"))),
            }
        };
        let flag = |key: &str, default: bool| -> CliResult<bool> {
            match p.get(key) {
                None | Some(Value::Null) => Ok(default),
                Some(Value::Bool(b)) => Ok(*b),
                Some(_) => Err(CliError::invalid(format!("`{key}` must be a boolean"))),
            }
        };
        let args: Vec<String> = match p.get("params") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(items)) => items
                .iter()
                .map(|v| v.as_str().map(str::to_string))
                .collect::<Option<_>>()
                .ok_or_else(params_error)?,
            Some(_) => return Err(params_error()),
        };
        let delivery_id = match p.get("delivery_id") {
            None | Some(Value::Null) => None,
            Some(v) => Some(
                v.as_i64()
                    .ok_or_else(|| CliError::invalid("`delivery_id` must be an integer"))?,
            ),
        };
        Ok(StartRequest {
            workflow_path: PathBuf::from(req_str(p, "workflow_path")?),
            source: opt_string(p, "source")?,
            project_path: opt_string(p, "project_path")?.map(PathBuf::from),
            params: workflow::parse_param_args(&args)?,
            harness: opt_string(p, "harness")?,
            model: opt_string(p, "model")?,
            placement: obj("placement")?
                .filter(|o| !o.is_empty())
                .map(|o| placement::Settings::from_json(&Value::Object(o.clone())))
                .transpose()?,
            cmux_caller: obj("cmux_caller")?.map(|o| Value::Object(o.clone())),
            trigger: obj("trigger")?.cloned().unwrap_or_default(),
            cause: obj("cause")?.map(|o| Value::Object(o.clone())),
            delivery_id,
            cancel_on_disconnect: flag("cancel_on_disconnect", true)?,
        })
    }

    /// The workflow the request names: `source` if given (what the CLI
    /// read), otherwise the file at `workflow_path`. Invalid workflows are
    /// `invalid_workflow` errors (exit 2).
    pub fn load_workflow(&self) -> CliResult<Workflow> {
        match &self.source {
            Some(source) => workflow::parse(&self.workflow_path, source),
            None => workflow::load(&self.workflow_path),
        }
        .map_err(Invalid::into_cli_error)
    }

    /// Started by a trigger rather than by someone running `tome run`.
    pub fn by_trigger(&self) -> bool {
        self.cause.is_some()
    }
}

fn params_error() -> CliError {
    CliError::invalid("`params` must be a list of \"key=value\" strings")
}

/// Where `tome run` was run from, for placement: what was focused and which
/// pane asked. Recorded with the run for its workers.
#[derive(Debug, Clone)]
pub struct Origin {
    pub focused: Value,
    pub caller: Value,
}

/// Resolve the request's params against the workflow and record the run
/// with its resolved snapshot, so later edits to the file only affect new
/// runs. Also returns the rendered body.
pub fn create_run(
    store: &mut Store,
    req: &StartRequest,
    wf: &Workflow,
    status: RunStatus,
    origin: Option<&Origin>,
) -> CliResult<(Run, String)> {
    let params = wf
        .resolve_params(&req.params, true)
        .map_err(Invalid::into_cli_error)?;
    // A run queued by a `while_running: queue` trigger has more paths merged
    // in while it waits, so its placeholders are filled in when it starts.
    let deferred = crate::triggers::waits_for_idle(req.cause.as_ref());
    // `tome run`'s placement flags, kept for the run's workers, and what
    // was focused and which cmux pane asked for it.
    let mut placement = Map::new();
    if let Some(flags) = req.placement.as_ref().filter(|f| !f.is_empty()) {
        placement.insert("flags".into(), json!(flags));
    }
    if let Some(origin) = origin {
        placement.insert("focused".into(), origin.focused.clone());
        placement.insert("caller".into(), origin.caller.clone());
    }
    let placement = (!placement.is_empty()).then(|| Value::Object(placement));

    let mut body = String::new();
    let run = store
        .create_run(
            NewRun {
                workflow_name: wf.name(),
                workflow_path: Some(&wf.path()),
                project_path: req.project_path.as_deref(),
                params: &params,
                status,
                trigger: req.cause.as_ref(),
                placement: placement.as_ref(),
                mode: wf.frontmatter().mode,
            },
            |id| {
                if deferred {
                    body = wf.body().to_string();
                    return wf.template_snapshot();
                }
                body = wf.render_body(&params, &id.to_string(), &req.trigger);
                wf.render_snapshot(&params, &id.to_string(), &req.trigger)
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

/// `worktree.add {run_id, path, repo_path?, branch?, base?}`: record a worktree a run
/// created so gc can clean it up.
fn worktree_add(store: &mut Store, p: &Value) -> CliResult<Value> {
    let run_id = p
        .get("run_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| CliError::invalid("missing integer `run_id`"))?;
    store.require_run(run_id)?;
    let path = PathBuf::from(req_str(p, "path")?);
    store
        .add_worktree(
            run_id,
            &store::NewWorktree {
                path: &path,
                repo_path: opt_str(p, "repo_path").map(Path::new),
                branch: opt_str(p, "branch"),
                base: opt_str(p, "base"),
                worker: None,
            },
        )
        .map_err(|e| CliError::internal(format!("{e:#}")))?;
    Ok(json!({ "run_id": run_id, "path": path }))
}

/// `runs.list {status?, workflow?, limit?}`: newest first.
fn runs_list(store: &mut Store, p: &Value) -> CliResult<Value> {
    let status = match opt_str(p, "status") {
        None => None,
        Some(s) => Some(RunStatus::parse(s).ok_or_else(|| {
            CliError::invalid(format!(
                "invalid status `{s}` (use queued, running, succeeded, failed or cancelled)"
            ))
        })?),
    };
    let filter = RunFilter {
        status,
        workflow: opt_str(p, "workflow").map(str::to_string),
        limit: p.get("limit").and_then(Value::as_u64).map(|n| n as usize),
    };
    Ok(json!({ "runs": store.list_runs(&filter).map_err(internal)? }))
}

/// `runs.live {workflow_path}`: the running and queued runs of that exact
/// file, oldest first. `tome workflow rm` asks this before deleting it.
fn runs_live(store: &mut Store, p: &Value) -> CliResult<Value> {
    let path = PathBuf::from(req_str(p, "workflow_path")?);
    let runs: Vec<Run> = store
        .in_progress_runs()
        .map_err(internal)?
        .into_iter()
        .filter(|r| {
            r.workflow_path
                .as_deref()
                .is_some_and(|w| workflow::same_path(Path::new(w), &path))
        })
        .collect();
    Ok(json!({ "runs": runs }))
}

/// `runs.show {id, snapshot?}`: the run with its steps, step history,
/// agents' start handshakes, worktrees and (freshly indexed) log files.
fn runs_show(store: &mut Store, p: &Value) -> CliResult<Value> {
    let id = req_id(p)?;
    store.require_run(id)?;
    let snapshot = p.get("snapshot").and_then(Value::as_bool).unwrap_or(false);
    let run = store.get_run(id, snapshot).map_err(internal)?;
    Ok(json!({
        "run": run,
        "steps": store.steps(id).map_err(internal)?,
        "history": store.step_history(id).map_err(internal)?,
        "workers": store.workers(id)?,
        "handshake": store
            .worker_history(id)?
            .iter()
            .filter(|h| h.group.is_none() && crate::handshake::state::ALL.contains(&h.event.as_str()))
            .map(|h| json!({ "worker": h.worker, "state": h.event, "message": h.message, "occurred_at": h.occurred_at }))
            .collect::<Vec<_>>(),
        "groups": store.groups(id)?,
        "worktrees": store.worktrees(id).map_err(internal)?,
        "sessions": store
            .sessions(id)
            .map_err(internal)?
            .iter()
            .map(|x| {
                let mut v = json!(x);
                v["attach"] = json!(crate::session::attach_command(x));
                v
            })
            .collect::<Vec<_>>(),
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
            return Err(CliError::not_found(format!(
                "run {id} has no log for step `{step}`"
            )));
        }
    }
    let logs: Vec<Value> = logs
        .into_iter()
        .map(|l| {
            let path = Path::new(&l.path);
            let content = match tail {
                Some(n) => store::read_tail_bytes(path, n, LOG_TAIL_MAX_BYTES),
                None => std::fs::read(path)
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .unwrap_or_default(),
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
    p.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| CliError::invalid(format!("missing string `{key}`")))
}

pub fn opt_str<'a>(p: &'a Value, key: &str) -> Option<&'a str> {
    p.get(key).and_then(Value::as_str)
}

/// An optional string field: absent and `null` are `None`, anything else
/// that isn't a string is an error.
fn opt_string(p: &Value, key: &str) -> CliResult<Option<String>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(CliError::invalid(format!("`{key}` must be a string"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_request_decodes_typed_fields() {
        let req = StartRequest::from_json(&json!({
            "workflow_path": "/p/build.md",
            "project_path": "/p",
            "params": ["base=dev", "n=1"],
            "harness": "codex",
            "placement": { "layout": "tab" },
            "delivery_id": 7,
            "cancel_on_disconnect": false,
        }))
        .unwrap();
        assert_eq!(req.workflow_path, PathBuf::from("/p/build.md"));
        assert_eq!(req.project_path.as_deref(), Some(Path::new("/p")));
        assert_eq!(
            req.params,
            vec![("base".into(), "dev".into()), ("n".into(), "1".into())]
        );
        assert_eq!(req.harness.as_deref(), Some("codex"));
        assert!(req.placement.as_ref().is_some_and(|p| p.layout.is_some()));
        assert_eq!(req.delivery_id, Some(7));
        assert!(!req.cancel_on_disconnect);
        assert!(!req.by_trigger());
    }

    #[test]
    fn start_request_defaults() {
        let req =
            StartRequest::from_json(&json!({ "workflow_path": "w.md", "params": null })).unwrap();
        assert!(req.params.is_empty());
        assert!(req.placement.is_none());
        assert!(req.cancel_on_disconnect);
        assert!(req.trigger.is_empty());
    }

    #[test]
    fn start_request_rejects_malformed_fields() {
        for (bad, field) in [
            (json!({}), "workflow_path"),
            (json!({ "workflow_path": 3 }), "workflow_path"),
            (json!({ "workflow_path": "w", "params": "a=b" }), "params"),
            (json!({ "workflow_path": "w", "params": [1] }), "params"),
            (
                json!({ "workflow_path": "w", "params": ["nokey"] }),
                "--param",
            ),
            (json!({ "workflow_path": "w", "harness": 1 }), "harness"),
            (
                json!({ "workflow_path": "w", "placement": "tab" }),
                "placement",
            ),
            (
                json!({ "workflow_path": "w", "placement": { "layout": "nope" } }),
                "--layout",
            ),
            (json!({ "workflow_path": "w", "trigger": [] }), "trigger"),
            (json!({ "workflow_path": "w", "cause": "x" }), "cause"),
            (
                json!({ "workflow_path": "w", "delivery_id": "7" }),
                "delivery_id",
            ),
            (
                json!({ "workflow_path": "w", "cancel_on_disconnect": "no" }),
                "cancel_on_disconnect",
            ),
        ] {
            let err = StartRequest::from_json(&bad).unwrap_err();
            assert_eq!(err.kind, crate::output::ErrorKind::Invalid, "{bad}");
            assert!(err.message.contains(field), "{bad}: {}", err.message);
        }
    }
}
