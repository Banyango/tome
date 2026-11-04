//! Workers this daemon hosts for another machine's run (`tome worker spawn
//! --node`): the run's daemon owns the worker; this keeps only what the
//! node needs to run and close its session.

use super::{fmt_ts, internal, now, Store};
use crate::output::CliResult;
use chrono::NaiveDateTime;
use duckdb::{params, OptionalExt, Row};
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HostedStatus {
    /// Its session is going.
    Running,
    /// Its session ended; the run's daemon hasn't heard yet.
    Ended,
    /// The run's daemon has heard it ended.
    Reported,
    /// The run's daemon closed it.
    Closed,
    /// The link to the run's daemon was lost while it ran.
    Orphaned,
}

impl HostedStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            HostedStatus::Running => "running",
            HostedStatus::Ended => "ended",
            HostedStatus::Reported => "reported",
            HostedStatus::Closed => "closed",
            HostedStatus::Orphaned => "orphaned",
        }
    }

    fn parse(s: &str) -> HostedStatus {
        match s {
            "running" => HostedStatus::Running,
            "ended" => HostedStatus::Ended,
            "reported" => HostedStatus::Reported,
            "closed" => HostedStatus::Closed,
            _ => HostedStatus::Orphaned,
        }
    }

    /// Whether its session may still be open.
    pub fn is_live(self) -> bool {
        matches!(self, HostedStatus::Running | HostedStatus::Ended)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct HostedWorker {
    /// The daemon whose run it belongs to.
    pub origin: String,
    pub run_id: i64,
    pub name: String,
    /// `agent` or `command`.
    pub kind: String,
    pub status: HostedStatus,
    /// The session, as `store::Session` serializes.
    pub session: Option<Value>,
    pub keep_open: bool,
    /// Its prompt, launcher, log and exit code.
    pub dir: String,
    /// The node's copy of the project, and the worktree in it.
    pub repo: Option<String>,
    pub worktree: Option<String>,
    pub branch: Option<String>,
    pub exit_code: Option<i32>,
    pub summary: Option<String>,
    pub created_at: String,
    pub ended_at: Option<String>,
}

pub struct NewHosted<'a> {
    pub origin: &'a str,
    pub run_id: i64,
    pub name: &'a str,
    pub kind: &'a str,
    pub keep_open: bool,
    pub dir: &'a str,
    pub repo: Option<&'a str>,
    pub worktree: Option<&'a str>,
    pub branch: Option<&'a str>,
}

const COLUMNS: &str = "origin, run_id, name, kind, status, session, keep_open, dir, repo, \
     worktree, branch, exit_code, summary, created_at, ended_at";

fn from_row(r: &Row<'_>) -> duckdb::Result<HostedWorker> {
    let status: String = r.get(4)?;
    let session: Option<String> = r.get(5)?;
    Ok(HostedWorker {
        origin: r.get(0)?,
        run_id: r.get(1)?,
        name: r.get(2)?,
        kind: r.get(3)?,
        status: HostedStatus::parse(&status),
        session: session.and_then(|s| serde_json::from_str(&s).ok()),
        keep_open: r.get(6)?,
        dir: r.get(7)?,
        repo: r.get(8)?,
        worktree: r.get(9)?,
        branch: r.get(10)?,
        exit_code: r.get(11)?,
        summary: r.get(12)?,
        created_at: fmt_ts(r.get(13)?),
        ended_at: r.get::<_, Option<NaiveDateTime>>(14)?.map(fmt_ts),
    })
}

impl Store {
    /// Record a hosted worker as running, replacing an earlier one of the
    /// same key (a respawn under the same name).
    pub fn add_hosted(&mut self, new: &NewHosted<'_>) -> CliResult<()> {
        self.conn
            .execute(
                "DELETE FROM hosted_workers WHERE origin = ? AND run_id = ? AND name = ?",
                params![new.origin, new.run_id, new.name],
            )
            .map_err(internal)?;
        self.conn
            .execute(
                "INSERT INTO hosted_workers (origin, run_id, name, kind, status, keep_open, dir, repo, worktree, branch, created_at)
                 VALUES (?, ?, ?, ?, 'running', ?, ?, ?, ?, ?, ?)",
                params![
                    new.origin, new.run_id, new.name, new.kind, new.keep_open, new.dir,
                    new.repo, new.worktree, new.branch, now()
                ],
            )
            .map_err(internal)?;
        Ok(())
    }

    pub fn set_hosted_session(
        &mut self,
        origin: &str,
        run_id: i64,
        name: &str,
        session: &Value,
    ) -> CliResult<()> {
        self.conn
            .execute(
                "UPDATE hosted_workers SET session = ? WHERE origin = ? AND run_id = ? AND name = ?",
                params![session.to_string(), origin, run_id, name],
            )
            .map_err(internal)?;
        Ok(())
    }

    /// Move a hosted worker to `status`; the first end records its exit
    /// code and summary.
    pub fn set_hosted_status(
        &mut self,
        origin: &str,
        run_id: i64,
        name: &str,
        status: HostedStatus,
        exit_code: Option<i32>,
        summary: Option<&str>,
    ) -> CliResult<()> {
        self.conn
            .execute(
                "UPDATE hosted_workers SET status = ?, exit_code = coalesce(exit_code, ?),
                 summary = coalesce(summary, ?), ended_at = coalesce(ended_at, ?)
                 WHERE origin = ? AND run_id = ? AND name = ?",
                params![
                    status.as_str(),
                    exit_code,
                    summary,
                    now(),
                    origin,
                    run_id,
                    name
                ],
            )
            .map_err(internal)?;
        Ok(())
    }

    pub fn hosted(&self, origin: &str, run_id: i64, name: &str) -> CliResult<Option<HostedWorker>> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM hosted_workers WHERE origin = ? AND run_id = ? AND name = ?"
                ),
                params![origin, run_id, name],
                from_row,
            )
            .optional()
            .map_err(internal)
    }

    /// Hosted workers, oldest first; only `origin`'s if given.
    pub fn hosted_workers(&self, origin: Option<&str>) -> CliResult<Vec<HostedWorker>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {COLUMNS} FROM hosted_workers WHERE ? IS NULL OR origin = ? ORDER BY created_at, name"
            ))
            .map_err(internal)?;
        let rows = stmt
            .query_map(params![origin, origin], from_row)
            .map_err(internal)?;
        rows.collect::<Result<_, _>>().map_err(internal)
    }

    /// Hosted workers that ended before `cutoff`, for gc.
    pub fn hosted_ended_before(&self, cutoff: NaiveDateTime) -> CliResult<Vec<HostedWorker>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {COLUMNS} FROM hosted_workers WHERE ended_at IS NOT NULL AND ended_at < ? ORDER BY ended_at"
            ))
            .map_err(internal)?;
        let rows = stmt
            .query_map(params![cutoff], from_row)
            .map_err(internal)?;
        rows.collect::<Result<_, _>>().map_err(internal)
    }

    pub fn delete_hosted(&mut self, origin: &str, run_id: i64, name: &str) -> CliResult<()> {
        self.conn
            .execute(
                "DELETE FROM hosted_workers WHERE origin = ? AND run_id = ? AND name = ?",
                params![origin, run_id, name],
            )
            .map_err(internal)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn new<'a>(name: &'a str) -> NewHosted<'a> {
        NewHosted {
            origin: "laptop-1",
            run_id: 7,
            name,
            kind: "command",
            keep_open: false,
            dir: "/tmp/x",
            repo: None,
            worktree: None,
            branch: None,
        }
    }

    #[test]
    fn hosted_workers_end_once() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("db"), &dir.path().join("runs")).unwrap();
        store.add_hosted(&new("build")).unwrap();
        store
            .set_hosted_session("laptop-1", 7, "build", &json!({ "name": "s" }))
            .unwrap();
        store
            .set_hosted_status(
                "laptop-1",
                7,
                "build",
                HostedStatus::Ended,
                Some(3),
                Some("boom"),
            )
            .unwrap();
        store
            .set_hosted_status(
                "laptop-1",
                7,
                "build",
                HostedStatus::Reported,
                Some(0),
                None,
            )
            .unwrap();
        let h = store.hosted("laptop-1", 7, "build").unwrap().unwrap();
        assert_eq!(h.status, HostedStatus::Reported);
        assert_eq!(h.exit_code, Some(3));
        assert_eq!(h.summary.as_deref(), Some("boom"));
        assert_eq!(h.session.unwrap()["name"], "s");
        assert_eq!(store.hosted_workers(Some("other")).unwrap().len(), 0);
        assert_eq!(store.hosted_workers(None).unwrap().len(), 1);
        // A respawn under the same name starts afresh.
        store.add_hosted(&new("build")).unwrap();
        let h = store.hosted("laptop-1", 7, "build").unwrap().unwrap();
        assert_eq!((h.status, h.exit_code), (HostedStatus::Running, None));
    }
}
