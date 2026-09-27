//! Run control, daemon side: starting runs and recording the transitions an
//! orchestrator (or a user) reports. The daemon owns one `Engine`, which owns
//! the only database connection.

use crate::api::{self, internal, opt_str, req_id, req_id_at, req_str};
use crate::output::{CliError, CliResult};
use crate::store::{Run, RunStatus, StepEvent, Store};
use serde_json::{json, Value};
use std::sync::Mutex;

/// Why a run was cancelled, recorded as the run's `reason`.
pub mod reason {
    /// `tome run cancel`.
    pub const USER_CANCELLED: &str = "user_cancelled";
}

const METHODS: &[&str] = &["run.start", "run.finish", "run.cancel", "step.report"];

pub struct Engine {
    /// `None` only once shutdown has closed the database.
    store: Mutex<Option<Store>>,
}

impl Engine {
    pub fn new(store: Store) -> Engine {
        Engine { store: Mutex::new(Some(store)) }
    }

    pub fn with_store<T>(&self, f: impl FnOnce(&mut Store) -> CliResult<T>) -> CliResult<T> {
        let mut guard = self.store.lock().unwrap_or_else(|p| p.into_inner());
        match guard.as_mut() {
            Some(store) => f(store),
            None => Err(CliError::internal("the daemon is shutting down")),
        }
    }

    /// Close (and checkpoint) the database.
    pub fn close(&self) {
        drop(self.store.lock().unwrap_or_else(|p| p.into_inner()).take());
    }

    pub fn handles(method: &str) -> bool {
        METHODS.contains(&method)
    }

    pub fn dispatch(&self, method: &str, p: &Value) -> CliResult<Value> {
        match method {
            "run.start" => self.start(p).map(|run| json!(run)),
            "run.finish" => self.finish(p).map(|run| json!(run)),
            "run.cancel" => self.cancel(req_id(p)?, opt_str(p, "reason").unwrap_or(reason::USER_CANCELLED)).map(|run| json!(run)),
            "step.report" => self.step(p),
            _ => Err(CliError::invalid(format!("unknown method `{method}`"))),
        }
    }

    /// `run.start {workflow_path, source?, project_path?, params?}`: validate
    /// the workflow, record the run with its resolved snapshot and return it
    /// straight away.
    fn start(&self, p: &Value) -> CliResult<Run> {
        let wf = api::load_workflow(p)?;
        self.with_store(|store| {
            let (run, _body) = api::create_run(store, p, &wf, RunStatus::Running)?;
            Ok(run)
        })
    }

    /// `run.finish {id, status, reason?, summary?}`
    fn finish(&self, p: &Value) -> CliResult<Run> {
        let id = req_id(p)?;
        let status = req_str(p, "status")?;
        let status = RunStatus::parse(status)
            .filter(|s| s.is_finished())
            .ok_or_else(|| CliError::invalid(format!("invalid status `{status}` (use succeeded, failed or cancelled)")))?;
        self.with_store(|store| {
            if status == RunStatus::Cancelled {
                return store.cancel_run(id, opt_str(p, "reason").unwrap_or(reason::USER_CANCELLED));
            }
            store.finish_run(id, status, opt_str(p, "reason"), opt_str(p, "summary"))
        })
    }

    /// Cancel an unfinished run (`run.cancel {id, reason?}`).
    pub fn cancel(&self, id: i64, reason: &str) -> CliResult<Run> {
        self.with_store(|store| store.cancel_run(id, reason))
    }

    /// `step.report {run_id, step?, event: start|done|fail, message?}`.
    /// `done`/`fail` without a step name apply to the step that's running.
    fn step(&self, p: &Value) -> CliResult<Value> {
        let run_id = req_id_at(p, "run_id")?;
        let event = req_str(p, "event")?;
        let event = StepEvent::parse(event)
            .ok_or_else(|| CliError::invalid(format!("invalid event `{event}` (use start, done or fail)")))?;
        self.with_store(|store| {
            let step = match (opt_str(p, "step").map(str::trim).filter(|s| !s.is_empty()), event) {
                (Some(step), _) => step.to_string(),
                (None, StepEvent::Start) => return Err(CliError::invalid("`tome step start` needs a step name")),
                (None, _) => {
                    store.require_run(run_id)?;
                    store.current_step(run_id).map_err(internal)?.ok_or_else(|| {
                        CliError::invalid(format!("run {run_id} has no running step; name the step to report"))
                    })?
                }
            };
            Ok(json!(store.report_step(run_id, &step, event, opt_str(p, "message"))?))
        })
    }
}
