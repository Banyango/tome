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
use crate::handshake::{self, Agent, Pending};
use crate::orchestrator;
use crate::output::{CliError, CliResult};
use crate::placement;
use crate::session;
use crate::store::{
    self, Run, RunStatus, StepEvent, StepHistory, Store, WorkerHistory, WorkerStatus,
};
use crate::triggers;
use crate::workers;
use crate::workflow::{self, OnConflict};
use serde_json::{json, Value};
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
/// How often herdr agents' statuses are read.
const STATUS_POLL: Duration = Duration::from_secs(2);

/// How often the monitor checks that running runs' orchestrators are alive.
const MONITOR_POLL: Duration = Duration::from_millis(500);

fn status_subject(engine: &Engine, session: &store::Session) -> String {
    if session.role == "orchestrator" || session.role == "agent" {
        return "agent".into();
    }
    engine
        .with_store(|store| {
            Ok(store
                .workers(session.run_id)?
                .into_iter()
                .find(|w| w.session.as_deref() == Some(session.name.as_str()))
                .map(|w| w.name)
                .unwrap_or_else(|| session.name.clone()))
        })
        .unwrap_or_else(|_| session.name.clone())
}

/// The watchers of one run, and how far they've been sent.
struct Watchers {
    senders: Vec<Sender<Value>>,
    /// Highest step or worker history id already sent (they share a
    /// sequence).
    last_event: i64,
    status: RunStatus,
}

const METHODS: &[&str] = &[
    "run.start",
    "run.finish",
    "run.cancel",
    "step.report",
    "session.move",
    "session.attach_command",
];

pub struct Engine {
    /// `None` only once shutdown has closed the database.
    store: Mutex<Option<Store>>,
    watchers: Mutex<HashMap<i64, Watchers>>,
    /// Runs whose orchestrator is being started; the monitor leaves them be.
    launching: Mutex<HashSet<i64>>,
    /// File triggers polling rather than on OS events, by armed key, with
    /// why. Kept by the trigger loop.
    pub(crate) file_polling: Mutex<HashMap<String, String>>,
    /// Launched agents that haven't made a tome call yet.
    pub(crate) starts: Mutex<HashMap<Agent, Pending>>,
    /// Held while topic triggers hand out their pending deliveries.
    pub(crate) draining: Mutex<()>,
    /// Topic triggers whose runs can't be started, by armed key, with why:
    /// left alone until they're re-armed.
    pub(crate) stalled: Mutex<HashMap<String, String>>,
    /// The forwarder's attempts and the nodes it's sending to.
    pub(crate) forwarding: Mutex<crate::bus::forward::State>,
    blocked_since: Mutex<HashMap<(i64, String), Instant>>,
    blocked_notified: Mutex<HashMap<(i64, String), Instant>>,
}

impl Engine {
    pub fn new(store: Store) -> Engine {
        Engine {
            store: Mutex::new(Some(store)),
            watchers: Mutex::new(HashMap::new()),
            launching: Mutex::new(HashSet::new()),
            file_polling: Mutex::new(HashMap::new()),
            starts: Mutex::new(HashMap::new()),
            draining: Mutex::new(()),
            stalled: Mutex::new(HashMap::new()),
            forwarding: Mutex::new(Default::default()),
            blocked_since: Mutex::new(HashMap::new()),
            blocked_notified: Mutex::new(HashMap::new()),
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
        METHODS.contains(&method)
            || workers::METHODS.contains(&method)
            || triggers::METHODS.contains(&method)
            || handshake::METHODS.contains(&method)
            || crate::bus::METHODS.contains(&method)
    }

    pub fn dispatch(&self, method: &str, p: &Value) -> CliResult<Value> {
        match method {
            "run.start" => self.start(p).map(|run| json!(run)),
            "run.finish" => self.finish(p).map(|run| json!(run)),
            "run.cancel" => self
                .cancel(
                    req_id(p)?,
                    opt_str(p, "reason").unwrap_or(reason::USER_CANCELLED),
                )
                .map(|run| json!(run)),
            "step.report" => self.step(p),
            "session.move" => self.move_session(p),
            "session.attach_command" => self.attach_session(p),
            m if workers::METHODS.contains(&m) => self.dispatch_primitive(m, p),
            m if triggers::METHODS.contains(&m) => self.dispatch_triggers(m, p),
            m if crate::bus::METHODS.contains(&m) => self.dispatch_bus(m, p),
            "agent.ready" => self.ready(p),
            _ => Err(CliError::invalid(format!("unknown method `{method}`"))),
        }
    }

    /// `run.start {workflow_path, source?, project_path?, params?}`: validate
    /// the workflow, record the run with its resolved snapshot, launch its
    /// orchestrator and return the run. At the workflow's concurrency limit
    /// the run is queued instead, or refused (`on_conflict: reject`). A run
    /// for a `while_running: queue` trigger is always queued; it waits for
    /// the workflow to be idle.
    pub(crate) fn start(&self, p: &Value) -> CliResult<Run> {
        let mut wf = api::load_workflow(p)?;
        if p.get("harness").and_then(Value::as_str).is_some()
            || p.get("model").and_then(Value::as_str).is_some()
        {
            let mut fm: serde_yaml::Value = serde_yaml::from_str(&wf.frontmatter_text)
                .map_err(|e| CliError::invalid(format!("invalid workflow frontmatter: {e}")))?;
            let root = fm
                .as_mapping_mut()
                .ok_or_else(|| CliError::invalid("workflow frontmatter must be a mapping"))?;
            let defaults_key = serde_yaml::Value::String("defaults".into());
            if !root.contains_key(&defaults_key) {
                root.insert(
                    defaults_key.clone(),
                    serde_yaml::Value::Mapping(Default::default()),
                );
            }
            let defaults = root
                .get_mut(&defaults_key)
                .and_then(serde_yaml::Value::as_mapping_mut)
                .ok_or_else(|| CliError::invalid("workflow defaults must be a mapping"))?;
            for (arg, worker_key, orch_key) in [
                ("harness", "harness", "orchestrator_harness"),
                ("model", "model", "orchestrator_model"),
            ] {
                if let Some(value) = p.get(arg).and_then(Value::as_str) {
                    defaults.insert(
                        serde_yaml::Value::String(worker_key.into()),
                        serde_yaml::Value::String(value.into()),
                    );
                    defaults.insert(
                        serde_yaml::Value::String(orch_key.into()),
                        serde_yaml::Value::String(value.into()),
                    );
                }
            }
            let body = wf
                .source
                .split_once("---")
                .and_then(|(_, s)| s.split_once("---").map(|(_, b)| b))
                .unwrap_or("");
            let source = format!(
                "---\n{}\n---{}",
                serde_yaml::to_string(&fm).map_err(|e| CliError::internal(e.to_string()))?,
                body
            );
            wf = crate::workflow::parse(&wf.path, &source)
                .map_err(crate::workflow::Invalid::into_cli_error)?;
        }
        let fm = &wf.frontmatter;
        // An unknown harness, backend, placement value or preset is a bad
        // request: refuse before recording a run.
        let project = opt_str(p, "project_path").map(std::path::Path::new);
        orchestrator::harness_for(fm, fm.mode, project)?
            .check_model(orchestrator::model_for(fm, fm.mode))?;
        // Workers use `defaults.harness`; `--harness` at spawn time is
        // checked when it happens.
        if let Some(model) = fm.defaults.model.as_deref() {
            let name = fm
                .defaults
                .harness
                .as_deref()
                .unwrap_or(crate::harness::DEFAULT);
            crate::harness::resolve(name, project)?.check_model(Some(model))?;
        }
        let caller_value = p.get("cmux_caller");
        let kind = session::Kind::choose_with_caller(
            fm.defaults.backend.as_deref(),
            project,
            caller_value.is_some_and(|c| c["herdr_pane"].is_string()),
        )?;
        let flags = p
            .get("placement")
            .filter(|v| !v.is_null())
            .map(placement::Settings::from_json)
            .transpose()?;
        if let Some(flags) = &flags {
            placement::check_flag_preset(flags, "`tome run` flags", project)?;
        }
        orchestrator::placement(fm, fm.mode, flags.as_ref(), project)?;
        placement::check_presets(fm.defaults.layout.as_ref(), project)?;
        // What's focused now, for `workspace: focused`.
        let by_trigger = p.get("cause").is_some_and(Value::is_object);
        let focused = match by_trigger {
            true => Err("the run was started by a trigger".to_string()),
            false => session::focused(kind),
        };
        // The cmux pane that ran `tome run`, for `from: caller`.
        let caller = match (
            by_trigger,
            p.get("cmux_caller")
                .filter(|c| c["surface"].is_string() || c["herdr_pane"].is_string()),
        ) {
            (true, _) => json!({ "unknown": "the run was started by a trigger" }),
            (false, None) => {
                json!({ "unknown": "`tome run` wasn't run from a cmux or herdr pane" })
            }
            (false, Some(c)) => {
                let mut c = c.clone();
                let found = c["surface"]
                    .as_str()
                    .map(session::caller_anchor)
                    .or_else(|| c["herdr_pane"].as_str().map(session::herdr_caller_anchor));
                if let Some(Ok((anchor, pane))) = found {
                    c["workspace"] = json!(anchor.handle);
                    c["pane"] = json!(pane);
                    if c["herdr_pane"].is_string() {
                        c["herdr_workspace"] = json!(anchor.handle);
                    }
                }
                c
            }
        };
        let mut p = p.clone();
        p["focused"] = match focused {
            Ok(id) => json!({ "id": id }),
            Err(why) => json!({ "unknown": why }),
        };
        p["caller"] = caller;
        let p = &p;
        let run = self.with_store(|store| {
            let status = match fm.concurrency {
                _ if triggers::waits_for_idle(p.get("cause")) => RunStatus::Queued,
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
            // A topic trigger's run holds its delivery from the start.
            if let Some(delivery) = p["delivery_id"].as_i64() {
                store.set_delivery_runs(delivery, &[run.id])?;
            }
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
    /// slots. Each queued run is held to the limit in its own snapshot, or
    /// to an idle workflow if a `while_running: queue` trigger queued it.
    /// Call whenever a run of the workflow ends.
    pub(crate) fn promote(&self, workflow: &str) {
        loop {
            let next = self.with_store(|store| {
                let Some(run) = store.next_queued(workflow).map_err(internal)? else {
                    return Ok(None);
                };
                let idle = triggers::waits_for_idle(run.trigger.as_ref());
                if let Some(limit) = if idle { Some(1) } else { snapshot_limit(&run) } {
                    if store
                        .count_runs(workflow, RunStatus::Running)
                        .map_err(internal)?
                        >= limit
                    {
                        return Ok(None);
                    }
                }
                let mut run = store.dequeue_run(run.id)?;
                if idle {
                    render_deferred(store, &mut run);
                }
                self.sync(store, run.id);
                Ok(Some(run))
            });
            let Ok(Some(run)) = next else { return };
            eprintln!(
                "tome daemon: run {} ({}) dequeued",
                run.id, run.workflow_name
            );
            // A failed launch frees its slot again; the loop moves on.
            let _ = self.launch(run);
        }
    }

    /// Start the queued runs that survived a daemon restart, as far as each
    /// workflow has room. Call once at startup, after recovery.
    pub fn resume_queued(&self) {
        let Ok(runs) = self.with_store(|store| store.in_progress_runs().map_err(internal)) else {
            return;
        };
        let mut workflows: Vec<String> = runs
            .into_iter()
            .filter(|r| r.status == RunStatus::Queued)
            .map(|r| r.workflow_name)
            .collect();
        workflows.sort();
        workflows.dedup();
        for workflow in workflows {
            self.promote(&workflow);
        }
    }

    /// Start a running run's orchestrator, and the clock on its start
    /// handshake. If that fails the run is marked failed (`launch_failed`)
    /// and the error returned.
    fn launch(&self, run: Run) -> CliResult<Run> {
        self.launching
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(run.id);
        let agent: Agent = (run.id, None);
        let mut waited = false;
        let mut backend = None;
        let result = orchestrator::plan(&run).and_then(|plan| {
            backend = Some(plan.backend);
            waited = plan.start_timeout.is_some();
            self.expect_start(
                agent.clone(),
                plan.start_timeout,
                orchestrator::prompt_file(run.id, plan.role),
            );
            let (session, note) =
                orchestrator::launch(&run, &plan, &self.recorded_sessions(run.id))?;
            // Recorded before the monitor may look (it skips launching runs).
            self.with_store(|store| {
                store.add_session(&session).map_err(internal)?;
                match &note {
                    Some(note) => store.add_run_note(run.id, note).map_err(internal),
                    None => Ok(()),
                }
            })
            .inspect_err(|_| {
                session::kill(&session);
            })
        });
        self.launching
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&run.id);
        match result {
            Ok(()) => {
                // Without a handshake to wait for, it has started now.
                if !waited {
                    self.announce_started(run.id);
                }
                Ok(run)
            }
            Err(e) => {
                self.forget_start(&agent);
                let _ = self.with_store(|store| {
                    let failed = store.abort_run(
                        run.id,
                        RunStatus::Failed,
                        if e.message.contains("backend_unavailable") {
                            orchestrator::BACKEND_UNAVAILABLE
                        } else {
                            orchestrator::LAUNCH_FAILED
                        },
                        Some(&e.message),
                    );
                    self.sync(store, run.id);
                    failed
                });
                if let Some(kind) = backend.filter(|k| *k == session::Kind::Herdr) {
                    if std::env::var("TOME_NOTIFY").as_deref() != Ok("off") {
                        session::notify(
                            kind,
                            &format!("tome: {} #{} failed", run.workflow_name, run.id),
                            &e.message,
                        );
                    }
                }
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
        // A run started from another machine outlives its connection.
        self.watch(run.id, p["cancel_on_disconnect"] != false, sink)
    }

    /// `run.finish {id, status, reason?, summary?}`
    fn finish(&self, p: &Value) -> CliResult<Run> {
        let id = req_id(p)?;
        let status = req_str(p, "status")?;
        let status = RunStatus::parse(status)
            .filter(|s| s.is_finished())
            .ok_or_else(|| {
                CliError::invalid(format!(
                    "invalid status `{status}` (use succeeded, failed or cancelled)"
                ))
            })?;
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
        self.with_store(|store| store.sessions(run_id).map_err(internal))
            .unwrap_or_default()
    }

    /// A run's session by name (its main role, or a worker's name), with
    /// the run and all its recorded sessions.
    fn find_session(
        &self,
        run_id: i64,
        name: &str,
    ) -> CliResult<(store::Run, Vec<store::Session>, store::Session)> {
        let run = self.with_store(|store| {
            store
                .get_run(run_id, false)
                .map_err(internal)?
                .ok_or_else(|| CliError::not_found(format!("run {run_id} not found")))
        })?;
        let recorded = self.recorded_sessions(run_id);
        let orch = session::run_session_name(run.id, &run.workflow_name, orchestrator::ROLE);
        let name_of = |s: &store::Session| -> String {
            match s.role.as_str() {
                role if orchestrator::is_main(role) => role.to_string(),
                _ => s
                    .name
                    .strip_prefix(&format!("{orch}-"))
                    .unwrap_or(&s.name)
                    .to_string(),
            }
        };
        let s = recorded
            .iter()
            .find(|s| name_of(s) == name)
            .cloned()
            .ok_or_else(|| {
                let names: Vec<String> = recorded.iter().map(name_of).collect();
                let main = orchestrator::role(run.mode);
                let hint = match names.is_empty() {
                    _ if orchestrator::is_main(name) => format!(
                        "run {run_id} is a{} run; its main session is `{main}`",
                        match run.mode {
                            crate::workflow::Mode::Single => " single-agent",
                            crate::workflow::Mode::Orchestrated => "n orchestrated",
                        }
                    ),
                    true => "it has no sessions".to_string(),
                    false => format!("its sessions: {}", names.join(", ")),
                };
                CliError::not_found(format!("run {run_id} has no session `{name}`")).with_hint(hint)
            })?;
        Ok((run, recorded, s))
    }

    /// `session.attach_command {run_id, worker?}`: how to attach to the run's
    /// main session (or a worker's) from a terminal on this machine.
    fn attach_session(&self, p: &Value) -> CliResult<Value> {
        let run_id = req_id_at(p, "run_id")?;
        let mode = self.with_store(|store| {
            store
                .get_run(run_id, false)
                .map_err(internal)?
                .map(|r| r.mode)
                .ok_or_else(|| CliError::not_found(format!("run {run_id} not found")))
        })?;
        let name = opt_str(p, "worker").unwrap_or(orchestrator::role(mode));
        let (_, _, s) = self.find_session(run_id, name)?;
        if session::is_alive(&s) == Some(false) {
            return Err(CliError::invalid(format!(
                "session `{name}` of run {run_id} isn't running"
            ))
            .with_hint(format!("its logs: `tome runs logs {run_id}`")));
        }
        Ok(json!({
            "run_id": run_id,
            "session": name,
            "name": s.name,
            "backend": s.backend,
            "command": session::attach_command(&s),
        }))
    }

    /// `session.move {run_id, name, placement, cmux_caller?}`: move a live
    /// session of the run (`orchestrator` or a worker's name) as the
    /// placement flags say, without restarting it. Settings the flags don't
    /// give stay as they were; `from: caller` is the cmux pane that ran the
    /// move (`cmux_caller`). Returns the session, with `attach_command`.
    fn move_session(&self, p: &Value) -> CliResult<Value> {
        let run_id = req_id_at(p, "run_id")?;
        let name = req_str(p, "name")?;
        let flags = p
            .get("placement")
            .filter(|v| !v.is_null())
            .map(placement::Settings::from_json)
            .transpose()?
            .filter(|f| !f.is_empty())
            .ok_or_else(|| {
                CliError::invalid("say where to move it")
                    .with_hint("give placement flags, e.g. --layout split or --preset <name>")
            })?;
        let (run, recorded, s) = self.find_session(run_id, name)?;
        if session::is_alive(&s) != Some(true) {
            return Err(CliError::invalid(format!(
                "session `{name}` of run {run_id} isn't running"
            )));
        }
        let project = orchestrator::run_project(&run);
        placement::check_flag_preset(&flags, "`tome session move` flags", project.as_deref())?;
        let current = s
            .placement
            .as_ref()
            .and_then(placement::Placement::from_json)
            .unwrap_or_else(|| placement::Placement::of_layout(session::Layout::of(&s)));
        let mut placement = placement::moved(&current, &flags, project.as_deref())?;
        let others: Vec<store::Session> = recorded
            .iter()
            .filter(|o| o.name != s.name)
            .cloned()
            .collect();
        let mut split =
            session::Split::new(placement.direction, placement.size, placement.from, &others);
        // `focused` is the workspace focused now, not when the run started.
        let mut warnings = Vec::new();
        let target = match &placement.workspace {
            placement::Workspace::Focused => match session::Kind::parse(&s.backend)
                .ok_or("its backend is unknown".to_string())
                .and_then(session::focused)
            {
                Ok(id) => session::Target::Focused(id),
                Err(why) => {
                    warnings.push(format!(
                        "workspace: focused: {why}; used the project workspace"
                    ));
                    session::Target::Project
                }
            },
            _ => orchestrator::target(&run, &placement).0,
        };
        // Next to the pane that runs the move; if that can't be, the move fails.
        let mut target = target;
        let mut layout = placement.layout;
        if placement.from == Some(placement::From::Caller) {
            let found = match (
                s.role.as_str(),
                p.get("cmux_caller")
                    .filter(|c| c["surface"].is_string() || c["herdr_pane"].is_string()),
            ) {
                (role, None) if orchestrator::is_main(role) => {
                    Err("`tome session move` wasn't run from a cmux or herdr pane".to_string())
                }
                (role, Some(c)) if orchestrator::is_main(role) => {
                    let kind = session::Kind::parse(&s.backend).unwrap_or(session::Kind::Tmux);
                    orchestrator::caller_anchor(kind, Some(c)).and_then(|found| {
                        match s.pane.as_deref() == Some(found.0.pane.as_str()) {
                            true => Err("that's the session's own pane".to_string()),
                            false => Ok(found),
                        }
                    })
                }
                _ => Err(placement::CALLER_IS_FOR_THE_ORCHESTRATOR.to_string()),
            };
            if let Err(why) = &found {
                return Err(CliError::invalid(format!(
                    "can't move `{name}` next to the caller: {why}; it was left where it was"
                )));
            }
            layout = orchestrator::use_caller(
                &mut placement,
                found,
                &mut split,
                &mut target,
                &mut warnings,
            );
        }
        let title = match s.role.as_str() {
            role if orchestrator::is_main(role) => {
                format!("tome: {} #{}", run.workflow_name, run.id)
            }
            _ => format!("tome: {} #{} / {name}", run.workflow_name, run.id),
        };
        let (moved, more) = session::move_to(
            &s,
            &session::Move {
                title: &title,
                layout,
                split: &split,
                target: &target,
                project: project.as_deref(),
            },
        )?;
        warnings.extend(more);
        placement.warnings = warnings;
        let moved = store::Session {
            placement: Some(placement.to_json()),
            ..moved
        };
        self.with_store(|store| store.add_session(&moved).map_err(internal))?;
        let mut out = json!(moved);
        out["attach_command"] = json!(session::attach_command(&moved));
        Ok(out)
    }

    pub(crate) fn kill_sessions(&self, run_id: i64) {
        orchestrator::kill_sessions(run_id, &self.recorded_sessions(run_id));
    }

    /// Fail running runs whose orchestrator or agent has exited without
    /// finishing the run (`orchestrator_exited`, `agent_exited`), end workers whose session is over, and
    /// nudge or fail agents that haven't started. Runs until the daemon
    /// closes the store.
    pub fn monitor(self: Arc<Self>) {
        loop {
            std::thread::sleep(MONITOR_POLL);
            self.check_workers();
            let Ok(sessions) = self.with_store(|store| store.running_sessions().map_err(internal))
            else {
                return;
            };
            for s in sessions {
                if self
                    .launching
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .contains(&s.run_id)
                {
                    continue;
                }
                if s.role != "orchestrator" && s.role != "agent" {
                    continue;
                }
                // Alive, or can't tell right now: look again next time.
                if session::is_alive(&s) != Some(false) {
                    continue;
                }
                let exited = match s.role.as_str() {
                    orchestrator::AGENT_ROLE => orchestrator::AGENT_EXITED,
                    _ => orchestrator::EXITED,
                };
                let ended = self.with_store(|store| {
                    // It may have finished while we looked.
                    if store.require_run(s.run_id)?.status != RunStatus::Running {
                        return Ok(None);
                    }
                    let cut =
                        store.end_active_workers(s.run_id, WorkerStatus::Cancelled, exited)?;
                    let run = store.abort_run(s.run_id, RunStatus::Failed, exited, None)?;
                    self.sync(store, s.run_id);
                    Ok(Some((run, cut)))
                });
                if let Ok(Some((run, cut))) = ended {
                    eprintln!(
                        "tome daemon: run {} ({}) failed: {}",
                        run.id, run.workflow_name, exited
                    );
                    // Its workers have no one to report to.
                    let recorded = self.recorded_sessions(run.id);
                    orchestrator::kill_sessions(run.id, &recorded);
                    orchestrator::notify(&run, &recorded, &cut);
                    self.promote(&run.workflow_name);
                }
            }
            self.check_starts();
        }
    }

    /// Herdr's agent statuses, polled apart from `monitor` so a slow herdr
    /// can't hold up liveness checks.
    pub fn status_monitor(self: Arc<Self>) {
        loop {
            std::thread::sleep(STATUS_POLL);
            let Ok(sessions) = self.with_store(|store| store.running_sessions().map_err(internal))
            else {
                return;
            };
            // Forget blocked-tracking for runs that are no longer running.
            let running: HashSet<i64> = sessions.iter().map(|s| s.run_id).collect();
            self.blocked_since
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .retain(|(run, _), _| running.contains(run));
            self.blocked_notified
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .retain(|(run, _), _| running.contains(run));
            for s in sessions {
                if self
                    .launching
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .contains(&s.run_id)
                {
                    continue;
                }
                self.track_agent_status(&s);
            }
        }
    }

    fn track_agent_status(&self, s: &store::Session) {
        if session::Kind::parse(&s.backend) != Some(session::Kind::Herdr) {
            return;
        }
        let Some(pane) = s.pane.as_deref() else {
            return;
        };
        // Herdr isn't answering: keep what we know.
        let Some(status) = session::agent_status(s) else {
            return;
        };
        let now = Instant::now();
        let key = (s.run_id, s.name.clone());
        let blocked = status == "blocked";
        let was_blocked = s.agent_status.as_deref() == Some("blocked");
        if blocked && !was_blocked {
            let at = chrono::Utc::now().to_rfc3339();
            let _ = self.with_store(|store| {
                store
                    .set_session_agent_status(s.run_id, &s.name, Some("blocked"), Some(&at))
                    .map_err(internal)
            });
            self.announce_blocked(s.run_id, &status_subject(self, s), pane, &at);
        } else if !blocked && s.agent_status.as_deref() != Some(status.as_str()) {
            let _ = self.with_store(|store| {
                store
                    .set_session_agent_status(s.run_id, &s.name, Some(&status), None)
                    .map_err(internal)
            });
        }
        if !blocked {
            self.blocked_since
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&key);
            self.blocked_notified
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&key);
            return;
        }
        // Also covers a daemon that restarted while the agent was blocked.
        let since = *self
            .blocked_since
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(key.clone())
            .or_insert(now);
        if now.duration_since(since) < Duration::from_secs(5) {
            return;
        }
        let mut notified = self
            .blocked_notified
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if notified
            .get(&key)
            .is_some_and(|t| now.duration_since(*t) < Duration::from_secs(60))
        {
            return;
        }
        notified.insert(key, now);
        drop(notified);
        if let Ok(Some(run)) =
            self.with_store(|store| store.get_run(s.run_id, false).map_err(internal))
        {
            let who = if s.role == "orchestrator" || s.role == "agent" {
                "agent".to_string()
            } else {
                format!("worker {}", status_subject(self, s))
            };
            orchestrator::notify_blocked(&run, &who);
        }
    }

    /// `step.report {run_id, step?, event: start|done|fail, message?}`.
    /// `done`/`fail` without a step name apply to the step that's running.
    fn step(&self, p: &Value) -> CliResult<Value> {
        let run_id = req_id_at(p, "run_id")?;
        let event = req_str(p, "event")?;
        let event = StepEvent::parse(event).ok_or_else(|| {
            CliError::invalid(format!("invalid event `{event}` (use start, done or fail)"))
        })?;
        self.with_store(|store| {
            let step = match (
                opt_str(p, "step").map(str::trim).filter(|s| !s.is_empty()),
                event,
            ) {
                (Some(step), _) => step.to_string(),
                (None, StepEvent::Start) => {
                    return Err(CliError::invalid("`tome step start` needs a step name"))
                }
                (None, _) => {
                    store.require_run(run_id)?;
                    store
                        .current_step(run_id)
                        .map_err(internal)?
                        .ok_or_else(|| {
                            CliError::invalid(format!(
                                "run {run_id} has no running step; name the step to report"
                            ))
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
    pub fn watch(
        &self,
        id: i64,
        cancel_on_disconnect: bool,
        sink: &mut dyn Sink,
    ) -> Option<CliResult<Run>> {
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

    fn pump(
        &self,
        id: i64,
        rx: &Receiver<Value>,
        cancel_on_disconnect: bool,
        sink: &mut dyn Sink,
    ) -> Option<CliResult<Run>> {
        let mut last_sync = Instant::now();
        loop {
            match rx.recv_timeout(WATCH_POLL) {
                Ok(event) => {
                    if sink.send(&event).is_err() {
                        return self.caller_gone(id, cancel_on_disconnect);
                    }
                    let finished = event["type"] == "run"
                        && event["status"]
                            .as_str()
                            .and_then(RunStatus::parse)
                            .is_some_and(RunStatus::is_finished);
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
                    return Some(Err(CliError::internal(
                        "the daemon stopped watching the run",
                    )));
                }
            }
        }
    }

    fn caller_gone(&self, id: i64, cancel: bool) -> Option<CliResult<Run>> {
        if cancel {
            // Already finished is fine: nothing left to cancel.
            if let Ok(run) = self.cancel(id, reason::CALLER_EXITED) {
                eprintln!(
                    "tome daemon: run {} ({}) cancelled: {}",
                    run.id,
                    run.workflow_name,
                    reason::CALLER_EXITED
                );
            }
        }
        None
    }

    /// Send watchers of `run_id` whatever changed since they were last
    /// synced. Call with the store locked, after changing the run.
    pub(crate) fn sync(&self, store: &Store, run_id: i64) {
        let mut watchers = self.watchers.lock().unwrap_or_else(|p| p.into_inner());
        let Some(w) = watchers.get_mut(&run_id) else {
            return;
        };
        let (Ok(history), Ok(Some(run))) = (
            history(store, run_id, w.last_event),
            store.get_run(run_id, false),
        ) else {
            return;
        };
        w.last_event = history
            .last()
            .map_or(w.last_event, |(id, _)| (*id).max(w.last_event));
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
    let main = store
        .get_run(run_id, false)
        .map_err(internal)?
        .map_or(orchestrator::ROLE, |r| orchestrator::role(r.mode));
    let mut out: Vec<(i64, Value)> = store
        .step_history(run_id)
        .map_err(internal)?
        .iter()
        .filter(|h| h.id > after)
        .map(|h| (h.id, step_event(run_id, h)))
        .collect();
    out.extend(
        store
            .worker_history(run_id)?
            .iter()
            .filter(|h| h.id > after)
            .map(|h| (h.id, worker_event(run_id, main, h))),
    );
    out.sort_by_key(|(id, _)| *id);
    Ok(out)
}

/// The concurrency limit in a run's workflow snapshot.
/// Fill in the placeholders of a run whose snapshot was saved unrendered
/// (see [`Workflow::template_snapshot`](crate::workflow::Workflow::template_snapshot)),
/// from its params and the trigger event recorded in its cause.
fn render_deferred(store: &mut Store, run: &mut Run) {
    let Some(template) = &run.workflow_snapshot else {
        return;
    };
    let path = PathBuf::from(run.workflow_path.as_deref().unwrap_or_default());
    let wf = match workflow::parse(&path, template) {
        Ok(wf) => wf,
        Err(inv) => {
            eprintln!(
                "tome daemon: run {}: can't fill in its placeholders: {}",
                run.id,
                triggers::first_errors(&inv)
            );
            return;
        }
    };
    let params = run.params.as_object().cloned().unwrap_or_default();
    let event = run
        .trigger
        .as_ref()
        .and_then(|c| c["event"].as_object())
        .cloned()
        .unwrap_or_default();
    let snapshot = wf.render_snapshot(&params, &run.id.to_string(), &event);
    match store.set_snapshot(run.id, &snapshot) {
        Ok(()) => run.workflow_snapshot = Some(snapshot),
        Err(e) => eprintln!(
            "tome daemon: run {}: saving its snapshot failed: {e:#}",
            run.id
        ),
    }
}

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
        "mode": run.mode,
        "role": orchestrator::role(run.mode),
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
/// (events: spawned, started, done, failed, cancelled), for an agent's start
/// handshake `{"type": "handshake", "run_id", "role", "worker", "event", "message", "time"}`
/// (events: waiting, nudged, ready, no_start),
/// for a group
/// `{"type": "group", "run_id", "group", "event": "finished", "message", "time"}`,
/// for a trigger signal `{"type": "trigger", "run_id", "event": "trigger", "message", "time"}`,
/// or for a publish `{"type": "publish", "run_id", "event": "published", "message", "time"}`.
///
/// `main` is the role of the run's main session (`orchestrator` or `agent`).
pub fn worker_event(run_id: i64, main: &str, h: &WorkerHistory) -> Value {
    if h.group.is_none() && handshake::state::ALL.contains(&h.event.as_str()) {
        return json!({
            "type": "handshake",
            "run_id": run_id,
            "role": if h.worker.is_some() { workers::ROLE } else { main },
            "worker": h.worker,
            "event": h.event,
            "message": h.message,
            "time": h.occurred_at,
        });
    }
    if h.worker.is_none() && h.group.is_none() && h.event == crate::bus::PUBLISHED {
        return json!({
            "type": "publish",
            "run_id": run_id,
            "event": h.event,
            "message": h.message,
            "time": h.occurred_at,
        });
    }
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
