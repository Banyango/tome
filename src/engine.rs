//! Run control, daemon side: starting runs, recording the transitions an
//! orchestrator (or a user) reports, and streaming them to watchers. The
//! daemon owns one `Engine`, which owns the only database connection.
//!
//! Events are derived from the store rather than emitted by each code path:
//! after every change, [`Engine::sync`] sends watchers the step history rows
//! and the run status change they haven't seen yet. Whatever changed a run
//! (a report, a cancel failing its running step, ...) is streamed the same
//! way.

use crate::api::{self, internal, opt_str, req_id, req_id_at, req_str};
use crate::orchestrator;
use crate::output::{CliError, CliResult};
use crate::session;
use crate::store::{self, Run, RunStatus, StepEvent, StepHistory, Store, WorkerHistory, WorkerStatus};
use crate::triggers;
use crate::workers;
use serde_json::{json, Value};
use crate::workflow::{self, OnConflict};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Why a run was cancelled, recorded as the run's `reason`.
pub mod reason {
    /// `tome run cancel`.
    pub const USER_CANCELLED: &str = "user_cancelled";
    /// Ctrl-C (or SIGTERM/SIGHUP) on an attached `tome run`.
    pub const INTERRUPTED: &str = "interrupted";
    /// The process running an attached `tome run` went away.
    pub const CALLER_EXITED: &str = "caller_exited";
}

/// How often a watch checks whether its caller is still there.
const WATCH_POLL: Duration = Duration::from_millis(200);
/// How often a watch re-syncs from the store, as a safety net.
const WATCH_RESYNC: Duration = Duration::from_secs(1);
/// How often the monitor checks that running runs' orchestrators are alive.
const MONITOR_POLL: Duration = Duration::from_millis(500);

/// The watchers of one run, and how far they've been sent.
struct Watchers {
    senders: Vec<Sender<Value>>,
    /// Highest step or worker history id already sent (they share a
    /// sequence).
    last_event: i64,
    status: RunStatus,
}

const METHODS: &[&str] = &["run.start", "run.finish", "run.cancel", "step.report"];

pub struct Engine {
    /// `None` only once shutdown has closed the database.
    store: Mutex<Option<Store>>,
    watchers: Mutex<HashMap<i64, Watchers>>,
    /// Runs whose orchestrator is being started; the monitor leaves them be.
    launching: Mutex<HashSet<i64>>,
}

impl Engine {
    pub fn new(store: Store) -> Engine {
        Engine {
            store: Mutex::new(Some(store)),
            watchers: Mutex::new(HashMap::new()),
            launching: Mutex::new(HashSet::new()),
        }
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
        METHODS.contains(&method) || workers::METHODS.contains(&method) || triggers::METHODS.contains(&method)
    }

    pub fn dispatch(&self, method: &str, p: &Value) -> CliResult<Value> {
        match method {
            "run.start" => self.start(p).map(|run| json!(run)),
            "run.finish" => self.finish(p).map(|run| json!(run)),
            "run.cancel" => self.cancel(req_id(p)?, opt_str(p, "reason").unwrap_or(reason::USER_CANCELLED)).map(|run| json!(run)),
            "step.report" => self.step(p),
            m if workers::METHODS.contains(&m) => self.dispatch_primitive(m, p),
            m if triggers::METHODS.contains(&m) => self.dispatch_triggers(m, p),
            _ => Err(CliError::invalid(format!("unknown method `{method}`"))),
        }
    }

    /// `run.start {workflow_path, source?, project_path?, params?}`: validate
    /// the workflow, record the run with its resolved snapshot, launch its
    /// orchestrator and return the run. At the workflow's concurrency limit
    /// the run is queued instead, or refused (`on_conflict: reject`).
    pub(crate) fn start(&self, p: &Value) -> CliResult<Run> {
        let wf = api::load_workflow(p)?;
        let fm = &wf.frontmatter;
        // An unknown harness or backend is a bad request: refuse before
        // recording a run.
        orchestrator::harness_for(fm)?;
        session::Kind::choose(fm.defaults.backend.as_deref())?;
        let run = self.with_store(|store| {
            let status = match fm.concurrency {
                None => RunStatus::Running,
                Some(limit) => {
                    let running = store.count_runs(&wf.name(), RunStatus::Running).map_err(internal)?;
                    let queued = store.count_runs(&wf.name(), RunStatus::Queued).map_err(internal)?;
                    match fm.on_conflict.unwrap_or(OnConflict::Queue) {
                        OnConflict::Reject if running >= limit as usize => {
                            return Err(CliError::conflict(format!(
                                "workflow `{}` is at its concurrency limit ({running} of {limit} running)",
                                wf.name()
                            ))
                            .with_hint("wait for a run to finish, cancel one with `tome run cancel <id>`, or set `on_conflict: queue`"));
                        }
                        // Behind any runs already waiting.
                        OnConflict::Queue if running >= limit as usize || queued > 0 => RunStatus::Queued,
                        _ => RunStatus::Running,
                    }
                }
            };
            let (run, _body) = api::create_run(store, p, &wf, status)?;
            Ok(run)
        })?;
        if run.status == RunStatus::Queued {
            eprintln!("tome daemon: run {} ({}) queued", run.id, run.workflow_name);
            return Ok(run);
        }
        let name = run.workflow_name.clone();
        self.launch(run).inspect_err(|_| self.promote(&name))
    }

    /// Start queued runs of `workflow`, oldest first, while there are free
    /// slots. Each queued run is held to the limit in its own snapshot. Call
    /// whenever a run of the workflow ends.
    fn promote(&self, workflow: &str) {
        loop {
            let next = self.with_store(|store| {
                let Some(run) = store.next_queued(workflow).map_err(internal)? else { return Ok(None) };
                if let Some(limit) = snapshot_limit(&run) {
                    if store.count_runs(workflow, RunStatus::Running).map_err(internal)? >= limit {
                        return Ok(None);
                    }
                }
                let run = store.dequeue_run(run.id)?;
                self.sync(store, run.id);
                Ok(Some(run))
            });
            let Ok(Some(run)) = next else { return };
            eprintln!("tome daemon: run {} ({}) dequeued", run.id, run.workflow_name);
            // A failed launch frees its slot again; the loop moves on.
            let _ = self.launch(run);
        }
    }

    /// Start a running run's orchestrator. If that fails the run is marked
    /// failed (`launch_failed`) and the error returned.
    fn launch(&self, run: Run) -> CliResult<Run> {
        self.launching.lock().unwrap_or_else(|p| p.into_inner()).insert(run.id);
        let result = orchestrator::plan(&run).and_then(|plan| {
            let session = orchestrator::launch(&run, &plan)?;
            // Recorded before the monitor may look (it skips launching runs).
            self.with_store(|store| store.add_session(&session).map_err(internal)).inspect_err(|_| {
                session::kill(&session);
            })
        });
        self.launching.lock().unwrap_or_else(|p| p.into_inner()).remove(&run.id);
        match result {
            Ok(()) => Ok(run),
            Err(e) => {
                let _ = self.with_store(|store| {
                    let failed = store.abort_run(run.id, RunStatus::Failed, orchestrator::LAUNCH_FAILED, Some(&e.message));
                    self.sync(store, run.id);
                    failed
                });
                self.kill_sessions(run.id);
                Err(e)
            }
        }
    }

    /// `run.start {..., attach: true}`: start the run and stream it on this
    /// connection until it finishes (see [`watch`](Self::watch)). If the
    /// caller goes away the run is cancelled.
    pub fn start_attached(&self, p: &Value, sink: &mut dyn Sink) -> Option<CliResult<Run>> {
        let run = match self.start(p) {
            Ok(run) => run,
            Err(e) => return Some(Err(e)),
        };
        self.watch(run.id, true, sink)
    }

    /// `run.finish {id, status, reason?, summary?}`
    fn finish(&self, p: &Value) -> CliResult<Run> {
        let id = req_id(p)?;
        let status = req_str(p, "status")?;
        let status = RunStatus::parse(status)
            .filter(|s| s.is_finished())
            .ok_or_else(|| CliError::invalid(format!("invalid status `{status}` (use succeeded, failed or cancelled)")))?;
        if status == RunStatus::Cancelled {
            return self.cancel(id, opt_str(p, "reason").unwrap_or(reason::USER_CANCELLED));
        }
        let run = self.with_store(|store| {
            let active: Vec<String> =
                store.workers(id)?.into_iter().filter(|w| !w.status.is_final()).map(|w| w.name).collect();
            if !active.is_empty() && store.require_run(id)?.status == RunStatus::Running {
                return Err(CliError::conflict(format!(
                    "run {id} still has running workers: {}",
                    active.join(", ")
                ))
                .with_hint("wait for them (`tome worker wait <name>`) or stop them (`tome worker kill <name>`) first"));
            }
            let run = store.finish_run(id, status, opt_str(p, "reason"), opt_str(p, "summary"))?;
            self.sync(store, id);
            Ok(run)
        })?;
        self.promote(&run.workflow_name);
        Ok(run)
    }

    /// Cancel an unfinished run (`run.cancel {id, reason?}`): mark it
    /// cancelled, then kill its sessions (a queued run has none) and start
    /// the next queued run of its workflow. Its worktrees are kept, and no
    /// notification is sent.
    pub fn cancel(&self, id: i64, reason: &str) -> CliResult<Run> {
        let run = self.with_store(|store| {
            let run = store.cancel_run(id, reason)?;
            self.sync(store, id);
            Ok(run)
        })?;
        self.kill_sessions(id);
        self.promote(&run.workflow_name);
        Ok(run)
    }

    pub(crate) fn recorded_sessions(&self, run_id: i64) -> Vec<store::Session> {
        self.with_store(|store| store.sessions(run_id).map_err(internal)).unwrap_or_default()
    }

    pub(crate) fn kill_sessions(&self, run_id: i64) {
        orchestrator::kill_sessions(run_id, &self.recorded_sessions(run_id));
    }

    /// Fail running runs whose orchestrator has exited without finishing the
    /// run (`orchestrator_exited`), and end workers whose session is over.
    /// Runs until the daemon closes the store.
    pub fn monitor(self: Arc<Self>) {
        loop {
            std::thread::sleep(MONITOR_POLL);
            self.check_workers();
            let Ok(sessions) = self.with_store(|store| store.running_orchestrators().map_err(internal)) else {
                return;
            };
            for s in sessions {
                if self.launching.lock().unwrap_or_else(|p| p.into_inner()).contains(&s.run_id) {
                    continue;
                }
                // Alive, or can't tell right now: look again next time.
                if session::is_alive(&s) != Some(false) {
                    continue;
                }
                let ended = self.with_store(|store| {
                    // It may have finished while we looked.
                    if store.require_run(s.run_id)?.status != RunStatus::Running {
                        return Ok(None);
                    }
                    let cut = store.end_active_workers(s.run_id, WorkerStatus::Cancelled, orchestrator::EXITED)?;
                    let run = store.abort_run(s.run_id, RunStatus::Failed, orchestrator::EXITED, None)?;
                    self.sync(store, s.run_id);
                    Ok(Some((run, cut)))
                });
                if let Ok(Some((run, cut))) = ended {
                    eprintln!("tome daemon: run {} ({}) failed: {}", run.id, run.workflow_name, orchestrator::EXITED);
                    // Its workers have no one to report to.
                    let recorded = self.recorded_sessions(run.id);
                    orchestrator::kill_sessions(run.id, &recorded);
                    orchestrator::notify(&run, &recorded, &cut);
                    self.promote(&run.workflow_name);
                }
            }
        }
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
            let step = store.report_step(run_id, &step, event, opt_str(p, "message"))?;
            self.sync(store, run_id);
            Ok(json!(step))
        })
    }

    // --- event stream ------------------------------------------------------

    /// Stream a run's events to `sink` until it finishes: first its current
    /// status and step history, then each transition as it happens. Returns
    /// the finished run, or `None` if the caller went away first (which
    /// cancels the run when `cancel_on_disconnect`).
    pub fn watch(&self, id: i64, cancel_on_disconnect: bool, sink: &mut dyn Sink) -> Option<CliResult<Run>> {
        let (tx, rx) = mpsc::channel();
        let replay = self.with_store(|store| {
            let run = store.require_run(id)?;
            let history = history(store, id, 0)?;
            if !run.status.is_finished() {
                let mut watchers = self.watchers.lock().unwrap_or_else(|p| p.into_inner());
                let w = watchers.entry(id).or_insert_with(|| Watchers {
                    senders: Vec::new(),
                    last_event: history.last().map_or(0, |(hid, _)| *hid),
                    status: run.status,
                });
                w.senders.push(tx);
            }
            Ok((run, history))
        });
        let (run, history) = match replay {
            Ok(r) => r,
            Err(e) => return Some(Err(e)),
        };

        let mut replay = Vec::new();
        if !run.status.is_finished() {
            replay.push(run_event(&run));
        }
        replay.extend(history.into_iter().map(|(_, e)| e));
        if run.status.is_finished() {
            replay.push(run_event(&run));
        }
        for event in &replay {
            if sink.send(event).is_err() {
                return self.caller_gone(id, cancel_on_disconnect);
            }
        }
        if run.status.is_finished() {
            return Some(Ok(run));
        }
        self.pump(id, &rx, cancel_on_disconnect, sink)
    }

    fn pump(&self, id: i64, rx: &Receiver<Value>, cancel_on_disconnect: bool, sink: &mut dyn Sink) -> Option<CliResult<Run>> {
        let mut last_sync = Instant::now();
        loop {
            match rx.recv_timeout(WATCH_POLL) {
                Ok(event) => {
                    if sink.send(&event).is_err() {
                        return self.caller_gone(id, cancel_on_disconnect);
                    }
                    let finished = event["type"] == "run"
                        && event["status"].as_str().and_then(RunStatus::parse).is_some_and(RunStatus::is_finished);
                    if finished {
                        return Some(self.with_store(|store| store.require_run(id)));
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    if sink.gone() {
                        return self.caller_gone(id, cancel_on_disconnect);
                    }
                    if last_sync.elapsed() >= WATCH_RESYNC {
                        last_sync = Instant::now();
                        if let Err(e) = self.with_store(|store| {
                            self.sync(store, id);
                            Ok(())
                        }) {
                            return Some(Err(e));
                        }
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Some(Err(CliError::internal("the daemon stopped watching the run")));
                }
            }
        }
    }

    fn caller_gone(&self, id: i64, cancel: bool) -> Option<CliResult<Run>> {
        if cancel {
            // Already finished is fine: nothing left to cancel.
            if let Ok(run) = self.cancel(id, reason::CALLER_EXITED) {
                eprintln!("tome daemon: run {} ({}) cancelled: {}", run.id, run.workflow_name, reason::CALLER_EXITED);
            }
        }
        None
    }

    /// Send watchers of `run_id` whatever changed since they were last
    /// synced. Call with the store locked, after changing the run.
    pub(crate) fn sync(&self, store: &Store, run_id: i64) {
        let mut watchers = self.watchers.lock().unwrap_or_else(|p| p.into_inner());
        let Some(w) = watchers.get_mut(&run_id) else { return };
        let (Ok(history), Ok(Some(run))) = (history(store, run_id, w.last_event), store.get_run(run_id, false)) else { return };
        w.last_event = history.last().map_or(w.last_event, |(id, _)| (*id).max(w.last_event));
        let mut events: Vec<Value> = history.into_iter().map(|(_, e)| e).collect();
        if run.status != w.status {
            w.status = run.status;
            events.push(run_event(&run));
        }
        for event in events {
            w.senders.retain(|tx| tx.send(event.clone()).is_ok());
        }
        // Nothing more will happen to a finished run.
        if w.senders.is_empty() || run.status.is_finished() {
            watchers.remove(&run_id);
        }
    }
}

/// A run's step and worker events after `after`, in order, with their ids.
fn history(store: &Store, run_id: i64, after: i64) -> CliResult<Vec<(i64, Value)>> {
    let mut out: Vec<(i64, Value)> = store
        .step_history(run_id)
        .map_err(internal)?
        .iter()
        .filter(|h| h.id > after)
        .map(|h| (h.id, step_event(run_id, h)))
        .collect();
    out.extend(store.worker_history(run_id)?.iter().filter(|h| h.id > after).map(|h| (h.id, worker_event(run_id, h))));
    out.sort_by_key(|(id, _)| *id);
    Ok(out)
}

/// The concurrency limit in a run's workflow snapshot.
fn snapshot_limit(run: &Run) -> Option<usize> {
    let snapshot = run.workflow_snapshot.as_deref()?;
    let path = PathBuf::from(run.workflow_path.clone().unwrap_or_default());
    let wf = workflow::parse_snapshot(&path, snapshot).ok()?;
    wf.frontmatter.concurrency.map(|n| n as usize)
}

/// Where a watch writes its events.
pub trait Sink {
    fn send(&mut self, event: &Value) -> std::io::Result<()>;
    /// Whether the caller has gone away.
    fn gone(&mut self) -> bool;
}

/// `{"type": "run", "run_id", "workflow", "status", "reason", "summary", "time"}`
pub fn run_event(run: &Run) -> Value {
    json!({
        "type": "run",
        "run_id": run.id,
        "workflow": run.workflow_name,
        "status": run.status,
        "reason": run.reason,
        "summary": run.summary,
        "time": run.finished_at.clone().unwrap_or_else(|| store::fmt_ts(store::now())),
    })
}

/// `{"type": "step", "run_id", "step", "event": start|done|fail, "message", "time"}`
pub fn step_event(run_id: i64, h: &StepHistory) -> Value {
    json!({
        "type": "step",
        "run_id": run_id,
        "step": h.step,
        "event": h.event,
        "message": h.message,
        "time": h.occurred_at,
    })
}

/// `{"type": "worker", "run_id", "worker", "group", "event", "message", "time"}`
/// (events: spawned, started, done, failed, cancelled), for a group
/// `{"type": "group", "run_id", "group", "event": "finished", "message", "time"}`,
/// or for a trigger signal `{"type": "trigger", "run_id", "event": "trigger", "message", "time"}`.
pub fn worker_event(run_id: i64, h: &WorkerHistory) -> Value {
    match &h.worker {
        None if h.group.is_none() => json!({
            "type": "trigger",
            "run_id": run_id,
            "event": h.event,
            "message": h.message,
            "time": h.occurred_at,
        }),
        Some(worker) => json!({
            "type": "worker",
            "run_id": run_id,
            "worker": worker,
            "group": h.group,
            "event": h.event,
            "message": h.message,
            "time": h.occurred_at,
        }),
        None => json!({
            "type": "group",
            "run_id": run_id,
            "group": h.group,
            "event": h.event,
            "message": h.message,
            "time": h.occurred_at,
        }),
    }
}
