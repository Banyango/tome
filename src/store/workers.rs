//! Workers a run's orchestrator spawned, the groups they belong to, and the
//! history of both (streamed alongside step events).

use super::{fmt_ts, internal, now, RunStatus, Store};
use crate::output::{CliError, CliResult};
use chrono::NaiveDateTime;
use duckdb::{params, OptionalExt, Row};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkerStatus {
    /// Recorded, its session not started yet.
    Pending,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl WorkerStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            WorkerStatus::Pending => "pending",
            WorkerStatus::Running => "running",
            WorkerStatus::Done => "done",
            WorkerStatus::Failed => "failed",
            WorkerStatus::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<WorkerStatus> {
        Some(match s {
            "pending" => WorkerStatus::Pending,
            "running" => WorkerStatus::Running,
            "done" => WorkerStatus::Done,
            "failed" => WorkerStatus::Failed,
            "cancelled" => WorkerStatus::Cancelled,
            _ => return None,
        })
    }

    pub fn is_final(self) -> bool {
        matches!(
            self,
            WorkerStatus::Done | WorkerStatus::Failed | WorkerStatus::Cancelled
        )
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Worker {
    #[serde(skip)]
    pub run_id: i64,
    pub name: String,
    /// `agent` or `command`.
    pub kind: String,
    pub status: WorkerStatus,
    pub reason: Option<String>,
    pub summary: Option<String>,
    pub group: Option<String>,
    pub harness: Option<String>,
    pub command: Option<Vec<String>>,
    pub session: Option<String>,
    pub worktree: Option<String>,
    pub branch: Option<String>,
    pub base: Option<String>,
    pub keep_open: bool,
    pub exit_code: Option<i32>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

pub struct NewWorker<'a> {
    /// `None` picks the next free `w<n>`.
    pub name: Option<&'a str>,
    pub kind: &'a str,
    pub group: Option<&'a str>,
    pub harness: Option<&'a str>,
    pub command: Option<&'a [String]>,
    pub keep_open: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Group {
    pub name: String,
    pub fail_fast: bool,
    /// Taking no new members.
    pub closed: bool,
    /// `open`, `closed` or `finished`.
    pub status: &'static str,
    pub created_at: String,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkerHistory {
    pub id: i64,
    pub worker: Option<String>,
    pub group: Option<String>,
    pub event: String,
    pub message: Option<String>,
    pub occurred_at: String,
}

/// What ending a worker led to.
#[derive(Debug, Clone)]
pub struct WorkerEnd {
    pub worker: Worker,
    /// Other members a fail-fast group cancelled; their sessions need killing.
    pub cut: Vec<Worker>,
    /// The group, if this finished it.
    pub group_finished: Option<Group>,
}

/// Why fail-fast cancelled a group's remaining members.
pub const FAIL_FAST: &str = "fail_fast";

const WORKER_COLUMNS: &str =
    "run_id, name, kind, status, reason, summary, group_name, harness, command, session, \
     worktree, branch, base, keep_open, exit_code, created_at, started_at, finished_at";

/// Worker, group and queue names: letters, digits, `_` and `-`.
pub fn check_name(what: &str, name: &str) -> CliResult<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if ok {
        Ok(())
    } else {
        Err(CliError::invalid(format!("invalid {what} name `{name}`"))
            .with_hint("use letters, digits, `_` and `-` (at most 64)"))
    }
}

fn ts(r: &Row<'_>, i: usize) -> duckdb::Result<Option<String>> {
    Ok(r.get::<_, Option<NaiveDateTime>>(i)?.map(fmt_ts))
}

fn worker_from_row(r: &Row<'_>) -> duckdb::Result<Worker> {
    let status: String = r.get(3)?;
    let command: Option<String> = r.get(8)?;
    Ok(Worker {
        run_id: r.get(0)?,
        name: r.get(1)?,
        kind: r.get(2)?,
        status: WorkerStatus::parse(&status).unwrap_or(WorkerStatus::Failed),
        reason: r.get(4)?,
        summary: r.get(5)?,
        group: r.get(6)?,
        harness: r.get(7)?,
        command: command.and_then(|c| serde_json::from_str(&c).ok()),
        session: r.get(9)?,
        worktree: r.get(10)?,
        branch: r.get(11)?,
        base: r.get(12)?,
        keep_open: r.get(13)?,
        exit_code: r.get(14)?,
        created_at: fmt_ts(r.get(15)?),
        started_at: ts(r, 16)?,
        finished_at: ts(r, 17)?,
    })
}

fn group_from_row(r: &Row<'_>) -> duckdb::Result<Group> {
    let closed: bool = r.get(2)?;
    let finished_at = ts(r, 4)?;
    Ok(Group {
        name: r.get(0)?,
        fail_fast: r.get(1)?,
        closed,
        status: match (&finished_at, closed) {
            (Some(_), _) => "finished",
            (None, true) => "closed",
            (None, false) => "open",
        },
        created_at: fmt_ts(r.get(3)?),
        finished_at,
    })
}

impl Store {
    // --- workers -----------------------------------------------------------

    /// Record a new worker as `pending`, creating its group on first use.
    /// Refused if the run isn't running, the name is taken or the group is
    /// closed.
    pub fn reserve_worker(&mut self, run_id: i64, new: &NewWorker<'_>) -> CliResult<Worker> {
        let run = self.require_run(run_id)?;
        if run.status != RunStatus::Running {
            return Err(CliError::invalid(format!(
                "run {run_id} isn't running ({})",
                run.status.as_str()
            )));
        }
        let name = match new.name {
            Some(name) => {
                check_name("worker", name)?;
                if name == "orchestrator" {
                    return Err(CliError::invalid(
                        "`orchestrator` is reserved; pick another worker name",
                    ));
                }
                if self.worker(run_id, name)?.is_some() {
                    return Err(CliError::invalid(format!(
                        "run {run_id} already has a worker named `{name}`"
                    ))
                    .with_hint(
                        "worker names are unique within a run; pick another or leave --name out",
                    ));
                }
                name.to_string()
            }
            None => self.next_worker_name(run_id)?,
        };
        if let Some(group) = new.group {
            check_name("group", group)?;
            match self.group(run_id, group)? {
                Some(g) if g.closed => {
                    return Err(CliError::invalid(format!(
                        "group `{group}` is closed ({})",
                        g.status
                    ))
                    .with_hint("closed groups take no new members; use another group"));
                }
                Some(_) => {}
                None => self.insert_group(run_id, group, false)?,
            }
        }
        let command = new
            .command
            .map(|c| serde_json::to_string(c).unwrap_or_default());
        self.conn
            .execute(
                "INSERT INTO workers (run_id, name, kind, status, group_name, harness, command, keep_open, created_at)
                 VALUES (?, ?, ?, 'pending', ?, ?, ?, ?, ?)",
                params![run_id, name, new.kind, new.group, new.harness, command, new.keep_open, now()],
            )
            .map_err(internal)?;
        self.require_worker(run_id, &name)
    }

    fn next_worker_name(&self, run_id: i64) -> CliResult<String> {
        let taken: Vec<String> = self.workers(run_id)?.into_iter().map(|w| w.name).collect();
        let mut n = taken.len() + 1;
        loop {
            let name = format!("w{n}");
            if !taken.contains(&name) {
                return Ok(name);
            }
            n += 1;
        }
    }

    /// Forget a worker that never got going (its spawn failed early).
    pub fn delete_worker(&mut self, run_id: i64, name: &str) -> CliResult<()> {
        self.conn
            .execute(
                "DELETE FROM workers WHERE run_id = ? AND name = ?",
                params![run_id, name],
            )
            .map_err(internal)?;
        Ok(())
    }

    pub fn set_worker_worktree(
        &mut self,
        run_id: i64,
        name: &str,
        path: &str,
        branch: &str,
        base: &str,
    ) -> CliResult<()> {
        self.conn
            .execute(
                "UPDATE workers SET worktree = ?, branch = ?, base = ? WHERE run_id = ? AND name = ?",
                params![path, branch, base, run_id, name],
            )
            .map_err(internal)?;
        Ok(())
    }

    /// A pending worker's session started. Returns false if it isn't pending
    /// any more (it was ended meanwhile).
    pub fn start_worker(&mut self, run_id: i64, name: &str, session: &str) -> CliResult<bool> {
        let n = self
            .conn
            .execute(
                "UPDATE workers SET status = 'running', session = ?, started_at = ? WHERE run_id = ? AND name = ? AND status = 'pending'",
                params![session, now(), run_id, name],
            )
            .map_err(internal)?;
        if n > 0 {
            self.worker_event(run_id, Some(name), None, "started", Some(session))?;
        }
        Ok(n > 0)
    }

    /// End a worker that's still pending or running. A worker that already
    /// ended is refused: its first result stands. With `cascade`, a failure
    /// in a fail-fast group cancels the group's other members.
    pub fn finish_worker(
        &mut self,
        run_id: i64,
        name: &str,
        status: WorkerStatus,
        reason: Option<&str>,
        summary: Option<&str>,
        exit_code: Option<i32>,
        cascade: bool,
    ) -> CliResult<WorkerEnd> {
        let worker = self.require_worker(run_id, name)?;
        if worker.status.is_final() {
            return Err(CliError::conflict(format!(
                "worker `{name}` has already finished ({}); its first result stands",
                worker.status.as_str()
            )));
        }
        self.end_worker(&worker, status, reason, summary, exit_code)?;
        let worker = self.require_worker(run_id, name)?;
        let mut cut = Vec::new();
        let mut group_finished = None;
        if let Some(group) = worker.group.as_deref() {
            let g = self.require_group(run_id, group)?;
            if cascade && status == WorkerStatus::Failed && g.fail_fast && g.finished_at.is_none() {
                for other in self.group_members(run_id, group)? {
                    if !other.status.is_final() {
                        self.end_worker(
                            &other,
                            WorkerStatus::Cancelled,
                            Some(FAIL_FAST),
                            None,
                            None,
                        )?;
                        cut.push(self.require_worker(run_id, &other.name)?);
                    }
                }
                let message = format!("fail-fast: `{name}` failed, {} cancelled", cut.len());
                group_finished = Some(self.finish_group(run_id, group, &message)?);
            } else {
                group_finished = self.maybe_finish_group(run_id, group)?;
            }
        }
        Ok(WorkerEnd {
            worker,
            cut,
            group_finished,
        })
    }

    fn end_worker(
        &mut self,
        w: &Worker,
        status: WorkerStatus,
        reason: Option<&str>,
        summary: Option<&str>,
        exit_code: Option<i32>,
    ) -> CliResult<()> {
        self.conn
            .execute(
                "UPDATE workers SET status = ?, reason = ?, summary = ?, exit_code = ?, finished_at = ? WHERE run_id = ? AND name = ?",
                params![status.as_str(), reason, summary, exit_code, now(), w.run_id, w.name],
            )
            .map_err(internal)?;
        let message = summary.or(reason);
        self.worker_event(
            w.run_id,
            Some(&w.name),
            w.group.as_deref(),
            status.as_str(),
            message,
        )?;
        self.release_claims(w.run_id, &w.name)?;
        Ok(())
    }

    /// End every worker of a run that's still pending or running (the run
    /// is ending). Groups don't cascade. Returns the workers it ended.
    pub fn end_active_workers(
        &mut self,
        run_id: i64,
        status: WorkerStatus,
        reason: &str,
    ) -> CliResult<Vec<Worker>> {
        let mut ended = Vec::new();
        for w in self.workers(run_id)? {
            if !w.status.is_final() {
                self.end_worker(&w, status, Some(reason), None, None)?;
                ended.push(self.require_worker(run_id, &w.name)?);
            }
        }
        for g in self.groups(run_id)? {
            self.maybe_finish_group(run_id, &g.name)?;
        }
        Ok(ended)
    }

    pub fn worker(&self, run_id: i64, name: &str) -> CliResult<Option<Worker>> {
        let sql = format!("SELECT {WORKER_COLUMNS} FROM workers WHERE run_id = ? AND name = ?");
        self.conn
            .query_row(&sql, params![run_id, name], worker_from_row)
            .optional()
            .map_err(internal)
    }

    pub fn require_worker(&self, run_id: i64, name: &str) -> CliResult<Worker> {
        self.worker(run_id, name)?.ok_or_else(|| {
            CliError::not_found(format!("run {run_id} has no worker named `{name}`"))
        })
    }

    pub fn workers(&self, run_id: i64) -> CliResult<Vec<Worker>> {
        self.query_workers(
            "WHERE run_id = ? ORDER BY created_at, name",
            params![run_id],
        )
    }

    fn group_members(&self, run_id: i64, group: &str) -> CliResult<Vec<Worker>> {
        self.query_workers(
            "WHERE run_id = ? AND group_name = ? ORDER BY created_at, name",
            params![run_id, group],
        )
    }

    /// Running workers of running runs, for the monitor.
    pub fn running_workers(&self) -> CliResult<Vec<Worker>> {
        self.query_workers(
            "WHERE status = 'running' AND run_id IN (SELECT id FROM runs WHERE status = 'running') ORDER BY run_id, name",
            params![],
        )
    }

    fn query_workers(&self, rest: &str, args: &[&dyn duckdb::ToSql]) -> CliResult<Vec<Worker>> {
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT {WORKER_COLUMNS} FROM workers {rest}"))
            .map_err(internal)?;
        let rows = stmt.query_map(args, worker_from_row).map_err(internal)?;
        rows.collect::<Result<_, _>>().map_err(internal)
    }

    // --- groups ------------------------------------------------------------

    /// `tome group create`: refused if the group exists.
    pub fn create_group(&mut self, run_id: i64, name: &str, fail_fast: bool) -> CliResult<Group> {
        check_name("group", name)?;
        let run = self.require_run(run_id)?;
        if run.status != RunStatus::Running {
            return Err(CliError::invalid(format!(
                "run {run_id} isn't running ({})",
                run.status.as_str()
            )));
        }
        if self.group(run_id, name)?.is_some() {
            return Err(CliError::invalid(format!(
                "run {run_id} already has a group named `{name}`"
            )));
        }
        self.insert_group(run_id, name, fail_fast)?;
        self.require_group(run_id, name)
    }

    fn insert_group(&mut self, run_id: i64, name: &str, fail_fast: bool) -> CliResult<()> {
        self.conn
            .execute(
                "INSERT INTO worker_groups (run_id, name, fail_fast, closed, created_at) VALUES (?, ?, ?, false, ?)",
                params![run_id, name, fail_fast, now()],
            )
            .map_err(internal)?;
        Ok(())
    }

    pub fn group(&self, run_id: i64, name: &str) -> CliResult<Option<Group>> {
        self.conn
            .query_row(
                "SELECT name, fail_fast, closed, created_at, finished_at FROM worker_groups WHERE run_id = ? AND name = ?",
                params![run_id, name],
                group_from_row,
            )
            .optional()
            .map_err(internal)
    }

    pub fn require_group(&self, run_id: i64, name: &str) -> CliResult<Group> {
        self.group(run_id, name)?
            .ok_or_else(|| CliError::not_found(format!("run {run_id} has no group named `{name}`")))
    }

    pub fn groups(&self, run_id: i64) -> CliResult<Vec<Group>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name, fail_fast, closed, created_at, finished_at FROM worker_groups WHERE run_id = ? ORDER BY created_at, name")
            .map_err(internal)?;
        let rows = stmt
            .query_map(params![run_id], group_from_row)
            .map_err(internal)?;
        rows.collect::<Result<_, _>>().map_err(internal)
    }

    /// A group's members, in spawn order.
    pub fn members(&self, run_id: i64, group: &str) -> CliResult<Vec<Worker>> {
        self.group_members(run_id, group)
    }

    /// Stop a group taking new members. Returns the group, and whether that
    /// finished it (all its members had already ended).
    pub fn close_group(&mut self, run_id: i64, name: &str) -> CliResult<(Group, bool)> {
        let g = self.require_group(run_id, name)?;
        if !g.closed {
            self.conn
                .execute(
                    "UPDATE worker_groups SET closed = true WHERE run_id = ? AND name = ?",
                    params![run_id, name],
                )
                .map_err(internal)?;
        }
        let finished = self.maybe_finish_group(run_id, name)?.is_some();
        Ok((self.require_group(run_id, name)?, finished))
    }

    /// Finish a closed group once all its members have ended.
    fn maybe_finish_group(&mut self, run_id: i64, name: &str) -> CliResult<Option<Group>> {
        let g = self.require_group(run_id, name)?;
        if !g.closed || g.finished_at.is_some() {
            return Ok(None);
        }
        let members = self.group_members(run_id, name)?;
        if members.iter().any(|w| !w.status.is_final()) {
            return Ok(None);
        }
        let count = |s: WorkerStatus| members.iter().filter(|w| w.status == s).count();
        let message = format!(
            "{} done, {} failed, {} cancelled",
            count(WorkerStatus::Done),
            count(WorkerStatus::Failed),
            count(WorkerStatus::Cancelled)
        );
        Ok(Some(self.finish_group(run_id, name, &message)?))
    }

    fn finish_group(&mut self, run_id: i64, name: &str, message: &str) -> CliResult<Group> {
        self.conn
            .execute(
                "UPDATE worker_groups SET closed = true, finished_at = ? WHERE run_id = ? AND name = ?",
                params![now(), run_id, name],
            )
            .map_err(internal)?;
        self.worker_event(run_id, None, Some(name), "finished", Some(message))?;
        self.require_group(run_id, name)
    }

    // --- history -----------------------------------------------------------

    /// Record a worker (or, with no worker, a group) event.
    pub fn worker_event(
        &mut self,
        run_id: i64,
        worker: Option<&str>,
        group: Option<&str>,
        event: &str,
        message: Option<&str>,
    ) -> CliResult<()> {
        self.conn
            .execute(
                "INSERT INTO worker_events (run_id, worker, group_name, event, message, occurred_at) VALUES (?, ?, ?, ?, ?, ?)",
                params![run_id, worker, group, event, message, now()],
            )
            .map_err(internal)?;
        Ok(())
    }

    pub fn worker_history(&self, run_id: i64) -> CliResult<Vec<WorkerHistory>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, worker, group_name, event, message, occurred_at FROM worker_events WHERE run_id = ? ORDER BY id")
            .map_err(internal)?;
        let rows = stmt
            .query_map(params![run_id], |r| {
                Ok(WorkerHistory {
                    id: r.get(0)?,
                    worker: r.get(1)?,
                    group: r.get(2)?,
                    event: r.get(3)?,
                    message: r.get(4)?,
                    occurred_at: fmt_ts(r.get(5)?),
                })
            })
            .map_err(internal)?;
        rows.collect::<Result<_, _>>().map_err(internal)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{new_run, store};
    use super::*;

    fn spawn(store: &mut Store, run: i64, name: Option<&str>, group: Option<&str>) -> Worker {
        let w = store
            .reserve_worker(
                run,
                &NewWorker {
                    name,
                    kind: "agent",
                    group,
                    harness: Some("claude"),
                    command: None,
                    keep_open: false,
                },
            )
            .unwrap();
        store.start_worker(run, &w.name, "sess").unwrap();
        store.require_worker(run, &w.name).unwrap()
    }

    #[test]
    fn names_are_unique_and_generated() {
        let (_d, mut store) = store();
        let run = new_run(&mut store, "wf").id;
        assert_eq!(spawn(&mut store, run, None, None).name, "w1");
        assert_eq!(spawn(&mut store, run, Some("w3"), None).name, "w3");
        assert_eq!(spawn(&mut store, run, None, None).name, "w4");
        let new = NewWorker {
            name: Some("w3"),
            kind: "agent",
            group: None,
            harness: None,
            command: None,
            keep_open: false,
        };
        assert_eq!(
            store.reserve_worker(run, &new).unwrap_err().kind,
            crate::output::ErrorKind::Invalid
        );
        let bad = NewWorker {
            name: Some("a b"),
            ..new
        };
        assert!(store.reserve_worker(run, &bad).is_err());
    }

    #[test]
    fn first_result_stands() {
        let (_d, mut store) = store();
        let run = new_run(&mut store, "wf").id;
        spawn(&mut store, run, Some("a"), None);
        let end = store
            .finish_worker(run, "a", WorkerStatus::Done, None, Some("ok"), None, true)
            .unwrap();
        assert_eq!(end.worker.status, WorkerStatus::Done);
        assert_eq!(end.worker.summary.as_deref(), Some("ok"));
        let err = store
            .finish_worker(run, "a", WorkerStatus::Failed, None, None, None, true)
            .unwrap_err();
        assert_eq!(err.kind, crate::output::ErrorKind::Conflict);
        let events: Vec<_> = store
            .worker_history(run)
            .unwrap()
            .into_iter()
            .map(|h| h.event)
            .collect();
        assert_eq!(events, ["started", "done"]);
    }

    #[test]
    fn groups_finish_when_closed_and_all_members_ended() {
        let (_d, mut store) = store();
        let run = new_run(&mut store, "wf").id;
        spawn(&mut store, run, Some("a"), Some("g"));
        spawn(&mut store, run, Some("b"), Some("g"));
        let end = store
            .finish_worker(run, "a", WorkerStatus::Failed, None, None, None, true)
            .unwrap();
        assert!(
            end.group_finished.is_none() && end.cut.is_empty(),
            "not fail-fast, not closed"
        );
        let (g, finished) = store.close_group(run, "g").unwrap();
        assert!(!finished);
        assert_eq!(g.status, "closed");
        // Closed: no new members.
        let new = NewWorker {
            name: Some("c"),
            kind: "agent",
            group: Some("g"),
            harness: None,
            command: None,
            keep_open: false,
        };
        assert!(store
            .reserve_worker(run, &new)
            .unwrap_err()
            .message
            .contains("closed"));
        let end = store
            .finish_worker(run, "b", WorkerStatus::Done, None, None, None, true)
            .unwrap();
        let g = end.group_finished.unwrap();
        assert_eq!(g.status, "finished");
        let last = store.worker_history(run).unwrap().pop().unwrap();
        assert_eq!(
            (last.group.as_deref(), last.event.as_str()),
            (Some("g"), "finished")
        );
        assert_eq!(
            last.message.as_deref(),
            Some("1 done, 1 failed, 0 cancelled")
        );
    }

    #[test]
    fn fail_fast_cancels_the_rest() {
        let (_d, mut store) = store();
        let run = new_run(&mut store, "wf").id;
        store.create_group(run, "g", true).unwrap();
        assert!(store.create_group(run, "g", true).is_err());
        spawn(&mut store, run, Some("a"), Some("g"));
        spawn(&mut store, run, Some("b"), Some("g"));
        spawn(&mut store, run, Some("c"), Some("g"));
        store
            .finish_worker(run, "c", WorkerStatus::Done, None, None, None, true)
            .unwrap();
        let end = store
            .finish_worker(run, "a", WorkerStatus::Failed, None, None, None, true)
            .unwrap();
        assert_eq!(
            end.cut.iter().map(|w| w.name.as_str()).collect::<Vec<_>>(),
            ["b"]
        );
        assert_eq!(end.cut[0].status, WorkerStatus::Cancelled);
        assert_eq!(end.cut[0].reason.as_deref(), Some(FAIL_FAST));
        assert_eq!(end.group_finished.unwrap().status, "finished");
    }

    #[test]
    fn ending_a_run_ends_its_workers() {
        let (_d, mut store) = store();
        let run = new_run(&mut store, "wf").id;
        spawn(&mut store, run, Some("a"), None);
        spawn(&mut store, run, Some("b"), None);
        store
            .finish_worker(run, "a", WorkerStatus::Done, None, None, None, true)
            .unwrap();
        let ended = store
            .end_active_workers(run, WorkerStatus::Cancelled, "user_cancelled")
            .unwrap();
        assert_eq!(ended.len(), 1);
        assert_eq!(ended[0].name, "b");
        assert!(store.running_workers().unwrap().is_empty());
    }
}
