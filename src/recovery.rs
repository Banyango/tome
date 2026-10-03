//! Crash recovery, run once at daemon startup.
//!
//! A run is only driven while the daemon that started it is alive, so any
//! run still running when a daemon starts was orphaned by a crash, a kill or
//! a reboot. Those runs are marked failed with reason `daemon_restart`, as
//! are their running workers. Their worktrees are left in place for
//! inspection (`tome gc` removes them later), and the hooks give later
//! features a place to kill leftover agent sessions and tell the user.
//!
//! Queued runs haven't started anything yet, so they survive: they keep
//! their workflow snapshot, and the daemon starts them once their workflow
//! is idle.

use crate::store::{Run, RunStatus, Session, StepEvent, Store, Worker, WorkerStatus};

pub const REASON: &str = "daemon_restart";

/// Called for each recovered run (the daemon's are `orchestrator::Hooks`).
pub trait RecoveryHooks {
    /// Kill the agent sessions the run left behind (`sessions` are the
    /// recorded ones).
    fn kill_sessions(&self, _run: &Run, _sessions: &[Session]) {}
    /// Tell the user the run was interrupted (`cut` are the workers it
    /// failed).
    fn notify(&self, _run: &Run, _sessions: &[Session], _cut: &[Worker]) {}
}

/// Fail every running run; queued runs stay queued. Returns the recovered
/// runs (in their final state).
pub fn recover(store: &mut Store, hooks: &dyn RecoveryHooks) -> anyhow::Result<Vec<Run>> {
    let mut recovered = Vec::new();
    for run in store
        .in_progress_runs()?
        .into_iter()
        .filter(|r| r.status == RunStatus::Running)
    {
        let sessions = store.sessions(run.id)?;
        hooks.kill_sessions(&run, &sessions);
        let cut = store.end_active_workers(run.id, WorkerStatus::Failed, REASON)?;
        for step in store.steps(run.id)? {
            if step.status == "running" {
                store.report_step(run.id, &step.name, StepEvent::Fail, Some(REASON))?;
            }
        }
        let run = store.finish_run(run.id, RunStatus::Failed, Some(REASON), None)?;
        hooks.notify(&run, &sessions, &cut);
        recovered.push(run);
    }
    Ok(recovered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::NewRun;
    use serde_json::Map;
    use std::cell::RefCell;
    use std::path::Path;

    #[derive(Default)]
    struct Recorder {
        calls: RefCell<Vec<String>>,
    }

    impl RecoveryHooks for Recorder {
        fn kill_sessions(&self, run: &Run, _: &[Session]) {
            self.calls
                .borrow_mut()
                .push(format!("kill {} {}", run.id, run.status.as_str()));
        }
        fn notify(&self, run: &Run, _: &[Session], cut: &[Worker]) {
            let cut: Vec<&str> = cut.iter().map(|w| w.name.as_str()).collect();
            self.calls.borrow_mut().push(format!(
                "notify {} {} {cut:?}",
                run.id,
                run.status.as_str()
            ));
        }
    }

    #[test]
    fn fails_running_runs_and_calls_hooks() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open_in_memory(dir.path()).unwrap();
        let params = Map::new();
        let mut new = |status| {
            store
                .create_run(
                    NewRun {
                        workflow_name: "w",
                        workflow_path: None,
                        project_path: None,
                        params: &params,
                        status,
                        trigger: None,
                        placement: None,
                        mode: crate::workflow::Mode::Orchestrated,
                    },
                    |id| format!("snapshot {id}"),
                )
                .unwrap()
                .id
        };
        let (done, live, queued) = (
            new(RunStatus::Running),
            new(RunStatus::Running),
            new(RunStatus::Queued),
        );
        store
            .finish_run(done, RunStatus::Succeeded, None, None)
            .unwrap();
        store
            .report_step(live, "Build", StepEvent::Start, None)
            .unwrap();
        store
            .add_worktree(
                live,
                &crate::store::NewWorktree {
                    path: Path::new("/tmp/wt"),
                    repo_path: None,
                    branch: None,
                    base: None,
                    worker: None,
                },
            )
            .unwrap();

        let spawn = |store: &mut Store, name| {
            let new = crate::store::NewWorker {
                name: Some(name),
                kind: "command",
                group: None,
                harness: None,
                command: None,
                keep_open: false,
            };
            store.reserve_worker(live, &new).unwrap();
        };
        spawn(&mut store, "a");
        store.start_worker(live, "a", "tome-2-w-a").unwrap();
        spawn(&mut store, "b");
        store
            .finish_worker(live, "b", WorkerStatus::Done, None, None, None, true)
            .unwrap();

        let hooks = Recorder::default();
        let recovered = recover(&mut store, &hooks).unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].id, live);
        assert_eq!(recovered[0].reason.as_deref(), Some(REASON));
        assert_eq!(
            *hooks.calls.borrow(),
            [
                format!("kill {live} running"),
                format!("notify {live} failed [\"a\"]")
            ]
        );
        let a = store.require_worker(live, "a").unwrap();
        assert_eq!(
            (a.status, a.reason.as_deref()),
            (WorkerStatus::Failed, Some(REASON))
        );
        assert_eq!(
            store.require_worker(live, "b").unwrap().status,
            WorkerStatus::Done
        );

        assert_eq!(store.steps(live).unwrap()[0].status, "failed");
        assert_eq!(
            store.worktrees(live).unwrap().len(),
            1,
            "worktrees are kept"
        );
        assert_eq!(
            store.get_run(done, false).unwrap().unwrap().status,
            RunStatus::Succeeded
        );
        let queued = store.get_run(queued, true).unwrap().unwrap();
        assert_eq!(queued.status, RunStatus::Queued, "queued runs survive");
        assert_eq!(
            queued.workflow_snapshot,
            Some(format!("snapshot {}", queued.id))
        );
        // Nothing left to recover.
        assert!(recover(&mut store, &hooks).unwrap().is_empty());
    }
}
