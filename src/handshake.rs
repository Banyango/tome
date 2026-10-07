//! The start handshake: the daemon checks that every agent it launches
//! actually starts. An agent has started once it makes any tome call
//! carrying its identity (`TOME_RUN_ID`, plus `TOME_WORKER_ID` for a
//! worker); its bootstrap prompt asks it to run `tome ready` first.
//!
//! The timer starts when the agent's session is launched. If it runs out
//! with no call, the daemon types one line into the agent's pane and waits
//! another timeout; if there's still no call, the agent is failed and its
//! session killed. Each state (waiting, nudged, ready, no_start) goes in the
//! run's event stream.
//!
//! Waits are kept in memory only: a daemon restart fails running runs
//! (`daemon_restart`) anyway.

use crate::api::{opt_str, req_id_at};
use crate::engine::Engine;
use crate::ids::RunId;
use crate::orchestrator;
use crate::output::{CliError, CliResult};
use crate::session;
use crate::store::{RunStatus, Store, WorkerStatus};
use crate::workflow::{Frontmatter, Mode, StartTimeout};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub const METHODS: &[&str] = &["agent.ready"];

/// The handshake states, recorded as events in the run's history.
pub mod state {
    pub const WAITING: &str = "waiting";
    pub const NUDGED: &str = "nudged";
    pub const READY: &str = "ready";
    pub const NO_START: &str = "no_start";
    pub const ALL: &[&str] = &[WAITING, NUDGED, READY, NO_START];
}

/// Why a run failed when its orchestrator never made a tome call.
pub const ORCHESTRATOR_NO_START: &str = "orchestrator_no_start";
/// Why a single run failed when its agent never made a tome call.
pub const AGENT_NO_START: &str = "agent_no_start";
/// Why a worker failed when it never made a tome call.
pub const WORKER_NO_START: &str = "worker_no_start";

/// How long an agent has to make its first tome call, by default.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// Who a tome call came from, as the CLI sends it (from `TOME_RUN_ID` and
/// `TOME_WORKER_ID`).
pub fn caller_of(req_caller: &Value) -> Option<Agent> {
    let run_id = req_id_at(req_caller, "run_id").ok()?;
    let worker = opt_str(req_caller, "worker")
        .filter(|w| !w.is_empty())
        .map(str::to_string);
    Some((run_id, worker))
}

/// `TOME_START_TIMEOUT` (for tests): `off`, a duration like `30s`, or
/// milliseconds like `500ms`. `None` when unset or unreadable.
fn env_timeout() -> Option<Option<Duration>> {
    let v = std::env::var("TOME_START_TIMEOUT").ok()?;
    let v = v.trim();
    if v == "off" {
        return Some(None);
    }
    if let Some(ms) = v.strip_suffix("ms") {
        return ms.parse().ok().map(|ms| Some(Duration::from_millis(ms)));
    }
    crate::duration::parse(v).ok().map(Some)
}

/// How long an agent of a workflow has to start; `None` when the check is
/// off. `TOME_START_TIMEOUT` wins over `defaults.start_timeout`, which wins
/// over the default.
pub fn timeout(fm: &Frontmatter) -> Option<Duration> {
    env_timeout().unwrap_or_else(|| {
        fm.defaults
            .start_timeout
            .map_or(Some(DEFAULT_TIMEOUT), StartTimeout::duration)
    })
}

/// An agent the daemon launched that hasn't made a tome call yet.
#[derive(Debug, Clone)]
pub struct Pending {
    pub timeout: Duration,
    /// When the current wait ends.
    pub deadline: Instant,
    pub nudged: bool,
    /// What the nudge tells the agent to read.
    pub prompt_file: PathBuf,
}

/// An agent: a run's main session, its orchestrator or single agent
/// (`None`), or one of its workers.
pub type Agent = (RunId, Option<String>);

fn describe(agent: &Agent) -> String {
    match &agent.1 {
        None => format!("run {} main session", agent.0),
        Some(w) => format!("run {} worker {w}", agent.0),
    }
}

fn secs(d: Duration) -> String {
    if d.subsec_millis() == 0 {
        format!("{}s", d.as_secs())
    } else {
        format!("{}ms", d.as_millis())
    }
}

/// Record a handshake state for an agent in its run's history.
fn record(store: &mut Store, agent: &Agent, state: &str, message: &str) -> CliResult<()> {
    store.worker_event(agent.0, agent.1.as_deref(), None, state, Some(message))
}

impl Engine {
    /// Start the clock on an agent that's about to be launched. Call before
    /// its session starts, so a quick first call isn't missed.
    pub(crate) fn expect_start(
        &self,
        agent: Agent,
        timeout: Option<Duration>,
        prompt_file: PathBuf,
    ) {
        let Some(timeout) = timeout else { return };
        let _ = self.with_store(|store| {
            record(
                store,
                &agent,
                state::WAITING,
                &format!("waiting {} for a first tome call", secs(timeout)),
            )?;
            self.sync(store, agent.0);
            Ok(())
        });
        let pending = Pending {
            timeout,
            deadline: Instant::now() + timeout,
            nudged: false,
            prompt_file,
        };
        self.starts
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(agent, pending);
    }

    /// Stop waiting for an agent that won't start after all (its launch
    /// failed).
    pub(crate) fn forget_start(&self, agent: &Agent) {
        self.starts
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(agent);
    }

    /// A tome call came from `agent`: if it was being waited for, it has
    /// started.
    pub(crate) fn seen(&self, agent: Agent) {
        let Some(pending) = self
            .starts
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&agent)
        else {
            return;
        };
        let message = if pending.nudged {
            "first tome call, after the nudge"
        } else {
            "first tome call"
        };
        let _ = self.with_store(|store| {
            record(store, &agent, state::READY, message)?;
            self.sync(store, agent.0);
            Ok(())
        });
        if agent.1.is_none() {
            self.announce_started(agent.0);
        }
    }

    /// `agent.ready {run_id, worker?}`: `tome ready`. The call itself is the
    /// proof of start (see [`seen`](Self::seen)); this only refuses callers
    /// that have already ended.
    pub(crate) fn ready(&self, p: &Value) -> CliResult<Value> {
        let run_id = req_id_at(p, "run_id")?;
        let worker = opt_str(p, "worker").filter(|w| !w.is_empty());
        self.with_store(|store| {
            let run = store.require_run(run_id)?;
            if run.status != RunStatus::Running {
                return Err(CliError::invalid(format!(
                    "run {run_id} isn't running ({})",
                    run.status.as_str()
                )));
            }
            if let Some(name) = worker {
                let w = store.require_worker(run_id, name)?;
                if w.status.is_final() {
                    return Err(CliError::invalid(format!(
                        "worker `{name}` has already finished ({})",
                        w.status.as_str()
                    )));
                }
            }
            let role = if worker.is_some() {
                "worker"
            } else {
                orchestrator::role(run.mode)
            };
            Ok(json!({ "run_id": run_id, "role": role, "worker": worker, "ready": true }))
        })
    }

    /// Nudge or fail agents whose wait has run out. Called by the monitor,
    /// after it has dealt with sessions that exited.
    pub(crate) fn check_starts(&self) {
        let now = Instant::now();
        let due: Vec<(Agent, Pending)> = self
            .starts
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .filter(|(_, p)| p.deadline <= now)
            .map(|(a, p)| (a.clone(), p.clone()))
            .collect();
        for (agent, pending) in due {
            // Ended some other way meanwhile: nothing to wait for.
            if !self.still_waiting(&agent) {
                self.forget_start(&agent);
                continue;
            }
            // Not recorded yet (still launching): look again next time.
            let Some(s) = self.agent_session(&agent) else {
                continue;
            };
            // Its session is over: the exit handling takes it.
            if session::is_alive(&s) == Some(false) {
                continue;
            }
            if pending.nudged {
                // A call may have removed it since we looked.
                if self
                    .starts
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .remove(&agent)
                    .is_none()
                {
                    continue;
                }
                eprintln!("tome daemon: {} didn't start", describe(&agent));
                self.fail_no_start(&agent, &pending);
                continue;
            }
            let line = format!("tome: read {} and follow it", pending.prompt_file.display());
            let message = if session::send_line(&s, &line) {
                format!("no tome call in {}; typed: {line}", secs(pending.timeout))
            } else {
                format!(
                    "no tome call in {}; its pane is gone, so the nudge was skipped",
                    secs(pending.timeout)
                )
            };
            let mut starts = self.starts.lock().unwrap_or_else(|p| p.into_inner());
            // Started while we were typing: the ready event stands.
            let Some(p) = starts.get_mut(&agent) else {
                continue;
            };
            p.nudged = true;
            p.deadline = Instant::now() + p.timeout;
            drop(starts);
            let _ = self.with_store(|store| {
                record(store, &agent, state::NUDGED, &message)?;
                self.sync(store, agent.0);
                Ok(())
            });
        }
    }

    /// Whether the agent's run (and worker) is still going.
    fn still_waiting(&self, agent: &Agent) -> bool {
        self.with_store(|store| {
            let running = store.require_run(agent.0)?.status == RunStatus::Running;
            Ok(match &agent.1 {
                None => running,
                Some(name) => running && !store.require_worker(agent.0, name)?.status.is_final(),
            })
        })
        .unwrap_or(false)
    }

    fn agent_session(&self, agent: &Agent) -> Option<crate::store::Session> {
        let sessions = self.recorded_sessions(agent.0);
        match &agent.1 {
            None => sessions
                .into_iter()
                .find(|s| orchestrator::is_main(&s.role)),
            Some(name) => {
                let w = self
                    .with_store(|store| store.require_worker(agent.0, name))
                    .ok()?;
                let session = w.session?;
                sessions.into_iter().find(|s| s.name == session)
            }
        }
    }

    fn fail_no_start(&self, agent: &Agent, pending: &Pending) {
        let message = format!("no tome call in {} after the nudge", secs(pending.timeout));
        match &agent.1 {
            None => self.fail_orchestrator_start(agent, &message),
            Some(name) => self.fail_worker_start(agent, name, &message),
        }
    }

    /// The run fails (`orchestrator_no_start`, or `agent_no_start` for a
    /// single run), as when its orchestrator exits: its sessions are killed (the logs stay in the run directory),
    /// the user is notified, and its workflow's next queued run may start.
    fn fail_orchestrator_start(&self, agent: &Agent, message: &str) {
        let run_id = agent.0;
        let ended = self.with_store(|store| {
            let run = store.require_run(run_id)?;
            if run.status != RunStatus::Running {
                return Ok(None);
            }
            let reason = match run.mode {
                Mode::Single => AGENT_NO_START,
                Mode::Orchestrated => ORCHESTRATOR_NO_START,
            };
            record(store, agent, state::NO_START, message)?;
            let cut = store.end_active_workers(run_id, WorkerStatus::Cancelled, reason)?;
            let run = store.abort_run(run_id, RunStatus::Failed, reason, Some(message))?;
            self.sync(store, run_id);
            Ok(Some((run, cut, reason)))
        });
        let Ok(Some((run, cut, reason))) = ended else {
            return;
        };
        eprintln!(
            "tome daemon: run {} ({}) failed: {reason}",
            run.id, run.workflow_name
        );
        let recorded = self.recorded_sessions(run_id);
        orchestrator::kill_sessions(run_id, &recorded);
        orchestrator::notify(&run, &recorded, &cut);
        self.promote(&run.workflow_name);
    }

    /// The worker fails (`worker_no_start`) and its session is killed (its
    /// log stays in the run directory). The orchestrator gets the usual
    /// worker-finished nudge and decides what to do; the user isn't told.
    fn fail_worker_start(&self, agent: &Agent, name: &str, message: &str) {
        let run_id = agent.0;
        let end = self.with_store(|store| {
            if store.require_worker(run_id, name)?.status.is_final() {
                return Ok(None);
            }
            record(store, agent, state::NO_START, message)?;
            let end = store.finish_worker(
                run_id,
                name,
                WorkerStatus::Failed,
                Some(WORKER_NO_START),
                Some(message),
                None,
                true,
            )?;
            self.sync(store, run_id);
            Ok(Some(end))
        });
        let Ok(Some(end)) = end else { return };
        eprintln!("tome daemon: run {run_id} worker {name} failed: {WORKER_NO_START}");
        if let Some(s) = self.worker_session(run_id, &end.worker) {
            session::kill(&s);
        }
        self.after_end(run_id, &end, None);
    }
}
