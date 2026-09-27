//! Crash recovery, run once at daemon startup.
//!
//! A run is only driven while the daemon that started it is alive, so any
//! run still queued or running when a daemon starts was orphaned by a crash,
//! a kill or a reboot. Those runs are marked failed with reason
//! `daemon_restart`. Their worktrees are left in place for inspection
//! (`tome gc` removes them later), and the hooks give later features a place
//! to kill leftover agent sessions and tell the user.

use crate::store::{Run, RunStatus, Session, StepEvent, Store};

pub const REASON: &str = "daemon_restart";

/// Called for each recovered run (the daemon's are `orchestrator::Hooks`).
pub trait RecoveryHooks {
    /// Kill the agent sessions the run left behind (`sessions` are the
    /// recorded ones).
    fn kill_sessions(&self, _run: &Run, _sessions: &[Session]) {}
    /// Tell the user the run was interrupted.
    fn notify(&self, _run: &Run, _sessions: &[Session]) {}
}

/// Fail every in-progress run. Returns the recovered runs (in their final
/// state).
pub fn recover(store: &mut Store, hooks: &dyn RecoveryHooks) -> anyhow::Result<Vec<Run>> {
    let mut recovered = Vec::new();
    for run in store.in_progress_runs()? {
        let sessions = store.sessions(run.id)?;
        hooks.kill_sessions(&run, &sessions);
        for step in store.steps(run.id)? {
            if step.status == "running" {
                store.report_step(run.id, &step.name, StepEvent::Fail, Some(REASON))?;
            }
        }
        let run = store.finish_run(run.id, RunStatus::Failed, Some(REASON), None)?;
        hooks.notify(&run, &sessions);
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
            self.calls.borrow_mut().push(format!("kill {} {}", run.id, run.status.as_str()));
        }
        fn notify(&self, run: &Run, _: &[Session]) {
            self.calls.borrow_mut().push(format!("notify {} {}", run.id, run.status.as_str()));
        }
    }

    #[test]
    fn fails_in_progress_runs_and_calls_hooks() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open_in_memory(dir.path()).unwrap();
        let params = Map::new();
        let mut new = || {
            store
                .create_run(
                    NewRun {
                        workflow_name: "w",
                        workflow_path: None,
                        project_path: None,
                        params: &params,
                        status: RunStatus::Running,
                    },
                    |_| String::new(),
                )
                .unwrap()
                .id
        };
        let (done, live) = (new(), new());
        store.finish_run(done, RunStatus::Succeeded, None, None).unwrap();
        store.report_step(live, "Build", StepEvent::Start, None).unwrap();
        store.add_worktree(live, Path::new("/tmp/wt"), None, None).unwrap();

        let hooks = Recorder::default();
        let recovered = recover(&mut store, &hooks).unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].id, live);
        assert_eq!(recovered[0].reason.as_deref(), Some(REASON));
        assert_eq!(*hooks.calls.borrow(), [format!("kill {live} running"), format!("notify {live} failed")]);

        assert_eq!(store.steps(live).unwrap()[0].status, "failed");
        assert_eq!(store.worktrees(live).unwrap().len(), 1, "worktrees are kept");
        assert_eq!(store.get_run(done, false).unwrap().unwrap().status, RunStatus::Succeeded);
        // Nothing left to recover.
        assert!(recover(&mut store, &hooks).unwrap().is_empty());
    }
}
