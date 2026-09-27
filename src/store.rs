//! Run state in DuckDB (`~/.tome/tome.duckdb`). Only the daemon opens it:
//! DuckDB allows a single read-write process, so every access goes through
//! the daemon.
//!
//! The store keeps runs (with project path and the resolved workflow snapshot
//! taken at start), the step status/history the orchestrator reports, the
//! index of raw log files under `~/.tome/runs/<run-id>/<step>.log`, and the
//! worktrees a run created (so gc can remove them).

use crate::output::{CliError, CliResult};
use anyhow::Context;
use chrono::{NaiveDateTime, Utc};
use duckdb::{params, Config, Connection, OptionalExt, Row};
use serde::Serialize;
use serde_json::{Map, Value};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Schema migrations, applied in order. Never edit a released entry; append
/// a new one instead. The daemon applies pending ones on startup.
const MIGRATIONS: &[&str] = &[
    // 1: initial schema
    "
    CREATE SEQUENCE run_id_seq START 1;
    CREATE TABLE runs (
        id                BIGINT PRIMARY KEY,
        workflow_name     VARCHAR NOT NULL,
        workflow_path     VARCHAR,
        project_path      VARCHAR,
        params            VARCHAR NOT NULL,
        workflow_snapshot VARCHAR NOT NULL,
        status            VARCHAR NOT NULL,
        reason            VARCHAR,
        summary           VARCHAR,
        created_at        TIMESTAMP NOT NULL,
        finished_at       TIMESTAMP
    );
    CREATE TABLE steps (
        run_id      BIGINT NOT NULL,
        name        VARCHAR NOT NULL,
        status      VARCHAR NOT NULL,
        attempts    INTEGER NOT NULL,
        message     VARCHAR,
        started_at  TIMESTAMP,
        finished_at TIMESTAMP,
        PRIMARY KEY (run_id, name)
    );
    CREATE SEQUENCE step_event_seq START 1;
    CREATE TABLE step_events (
        id      BIGINT PRIMARY KEY DEFAULT nextval('step_event_seq'),
        run_id  BIGINT NOT NULL,
        step    VARCHAR NOT NULL,
        event   VARCHAR NOT NULL,
        message VARCHAR,
        occurred_at TIMESTAMP NOT NULL
    );
    CREATE TABLE logs (
        run_id     BIGINT NOT NULL,
        step       VARCHAR NOT NULL,
        path       VARCHAR NOT NULL,
        size       BIGINT NOT NULL,
        tail       VARCHAR NOT NULL,
        updated_at TIMESTAMP NOT NULL,
        PRIMARY KEY (run_id, step)
    );
    CREATE TABLE worktrees (
        run_id     BIGINT NOT NULL,
        path       VARCHAR NOT NULL,
        repo_path  VARCHAR,
        branch     VARCHAR,
        created_at TIMESTAMP NOT NULL,
        PRIMARY KEY (run_id, path)
    );
    ",
];

/// How much of a log file is kept in the index as its tail excerpt.
const TAIL_LINES: usize = 20;
const TAIL_BYTES: u64 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl RunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            RunStatus::Queued => "queued",
            RunStatus::Running => "running",
            RunStatus::Succeeded => "succeeded",
            RunStatus::Failed => "failed",
            RunStatus::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<RunStatus> {
        Some(match s {
            "queued" => RunStatus::Queued,
            "running" => RunStatus::Running,
            "succeeded" => RunStatus::Succeeded,
            "failed" => RunStatus::Failed,
            "cancelled" => RunStatus::Cancelled,
            _ => return None,
        })
    }

    pub fn is_finished(self) -> bool {
        matches!(self, RunStatus::Succeeded | RunStatus::Failed | RunStatus::Cancelled)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepEvent {
    Start,
    Done,
    Fail,
}

impl StepEvent {
    pub fn parse(s: &str) -> Option<StepEvent> {
        Some(match s {
            "start" => StepEvent::Start,
            "done" => StepEvent::Done,
            "fail" => StepEvent::Fail,
            _ => return None,
        })
    }

    fn as_str(self) -> &'static str {
        match self {
            StepEvent::Start => "start",
            StepEvent::Done => "done",
            StepEvent::Fail => "fail",
        }
    }

    fn resulting_status(self) -> &'static str {
        match self {
            StepEvent::Start => "running",
            StepEvent::Done => "done",
            StepEvent::Fail => "failed",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Run {
    pub id: i64,
    pub workflow_name: String,
    pub workflow_path: Option<String>,
    pub project_path: Option<String>,
    pub params: Value,
    pub status: RunStatus,
    pub reason: Option<String>,
    pub summary: Option<String>,
    pub created_at: String,
    pub finished_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflow_snapshot: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Step {
    pub name: String,
    pub status: String,
    pub attempts: i32,
    pub message: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StepHistory {
    pub id: i64,
    pub step: String,
    pub event: String,
    pub message: Option<String>,
    pub occurred_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    pub step: String,
    pub path: String,
    pub size: i64,
    pub tail: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Worktree {
    pub path: String,
    pub repo_path: Option<String>,
    pub branch: Option<String>,
    pub created_at: String,
}

pub struct NewRun<'a> {
    pub workflow_name: &'a str,
    pub workflow_path: Option<&'a Path>,
    pub project_path: Option<&'a Path>,
    pub params: &'a Map<String, Value>,
    /// `Running`, or `Queued` for a run waiting on a concurrency slot.
    pub status: RunStatus,
}

#[derive(Debug, Default, Clone)]
pub struct RunFilter {
    pub status: Option<RunStatus>,
    pub workflow: Option<String>,
    pub limit: Option<usize>,
}

pub struct Store {
    conn: Connection,
    runs_dir: PathBuf,
}

pub fn now() -> NaiveDateTime {
    Utc::now().naive_utc()
}

pub fn fmt_ts(ts: NaiveDateTime) -> String {
    ts.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// A step name as a safe log file stem.
pub fn log_file_stem(step: &str) -> String {
    let stem: String = step
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ') { c } else { '_' })
        .collect();
    let stem = stem.trim().trim_start_matches('.').to_string();
    if stem.is_empty() { "step".into() } else { stem }
}

impl Store {
    /// Open (creating if needed) the database and apply pending migrations.
    pub fn open(db_path: &Path, runs_dir: &Path) -> anyhow::Result<Store> {
        // No file/network access from SQL: `tome query` is passed straight
        // through, and the daemon doesn't need DuckDB to touch other files.
        let config = Config::default().enable_external_access(false)?;
        let conn = Connection::open_with_flags(db_path, config)
            .with_context(|| format!("opening {}", db_path.display()))?;
        let mut store = Store { conn, runs_dir: runs_dir.to_path_buf() };
        store.migrate()?;
        Ok(store)
    }

    #[cfg(test)]
    pub fn open_in_memory(runs_dir: &Path) -> anyhow::Result<Store> {
        let config = Config::default().enable_external_access(false)?;
        let conn = Connection::open_in_memory_with_flags(config)?;
        let mut store = Store { conn, runs_dir: runs_dir.to_path_buf() };
        store.migrate()?;
        Ok(store)
    }

    pub fn transaction(&mut self) -> duckdb::Result<duckdb::Transaction<'_>> {
        self.conn.transaction()
    }

    pub fn runs_dir(&self) -> &Path {
        &self.runs_dir
    }

    pub fn run_dir(&self, run_id: i64) -> PathBuf {
        self.runs_dir.join(run_id.to_string())
    }

    pub fn log_path(&self, run_id: i64, step: &str) -> PathBuf {
        self.run_dir(run_id).join(format!("{}.log", log_file_stem(step)))
    }

    fn migrate(&mut self) -> anyhow::Result<()> {
        self.conn.execute_batch("CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL)")?;
        let current = self.schema_version()?;
        if current > MIGRATIONS.len() {
            anyhow::bail!(
                "database schema version {current} is newer than this tome supports ({}); upgrade tome",
                MIGRATIONS.len()
            );
        }
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(current) {
            let version = i + 1;
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql).with_context(|| format!("applying schema migration {version}"))?;
            tx.execute("DELETE FROM schema_version", [])?;
            tx.execute("INSERT INTO schema_version VALUES (?)", params![version as i64])?;
            tx.commit()?;
            eprintln!("tome daemon: migrated database schema to version {version}");
        }
        Ok(())
    }

    pub fn schema_version(&self) -> anyhow::Result<usize> {
        let v: Option<i64> = self
            .conn
            .query_row("SELECT max(version) FROM schema_version", [], |r| r.get(0))
            .optional()?
            .flatten();
        Ok(v.unwrap_or(0) as usize)
    }

    // --- runs --------------------------------------------------------------

    /// Record a new run. `render` receives the allocated run id and returns
    /// the resolved workflow snapshot (so `{{run.id}}` can be substituted).
    pub fn create_run(&mut self, new: NewRun<'_>, render: impl FnOnce(i64) -> String) -> anyhow::Result<Run> {
        let tx = self.conn.transaction()?;
        let id: i64 = tx.query_row("SELECT nextval('run_id_seq')", [], |r| r.get(0))?;
        let snapshot = render(id);
        tx.execute(
            "INSERT INTO runs (id, workflow_name, workflow_path, project_path, params, workflow_snapshot, status, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                id,
                new.workflow_name,
                new.workflow_path.map(|p| p.display().to_string()),
                new.project_path.map(|p| p.display().to_string()),
                Value::Object(new.params.clone()).to_string(),
                snapshot,
                new.status.as_str(),
                now(),
            ],
        )?;
        tx.commit()?;
        std::fs::create_dir_all(self.run_dir(id))?;
        Ok(self.get_run(id, true)?.expect("run just inserted"))
    }

    pub fn get_run(&self, id: i64, with_snapshot: bool) -> anyhow::Result<Option<Run>> {
        let sql = format!("{} WHERE id = ?", run_select(with_snapshot));
        Ok(self.conn.query_row(&sql, params![id], |r| run_from_row(r, with_snapshot)).optional()?)
    }

    pub fn require_run(&self, id: i64) -> CliResult<Run> {
        self.get_run(id, false)
            .map_err(|e| CliError::internal(format!("{e:#}")))?
            .ok_or_else(|| CliError::not_found(format!("no run with id {id}")))
    }

    pub fn list_runs(&self, filter: &RunFilter) -> anyhow::Result<Vec<Run>> {
        let mut sql = run_select(false);
        let mut clauses = Vec::new();
        let mut args: Vec<String> = Vec::new();
        if let Some(status) = filter.status {
            clauses.push("status = ?");
            args.push(status.as_str().into());
        }
        if let Some(wf) = &filter.workflow {
            clauses.push("workflow_name = ?");
            args.push(wf.clone());
        }
        if !clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&clauses.join(" AND "));
        }
        sql.push_str(" ORDER BY id DESC");
        if let Some(limit) = filter.limit {
            sql.push_str(&format!(" LIMIT {limit}"));
        }
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(duckdb::params_from_iter(args.iter()), |r| run_from_row(r, false))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Runs that haven't finished (queued or running).
    pub fn in_progress_runs(&self) -> anyhow::Result<Vec<Run>> {
        let sql = format!("{} WHERE status IN ('queued', 'running') ORDER BY id", run_select(false));
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], |r| run_from_row(r, false))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Finished runs whose `finished_at` is before `cutoff`, oldest first.
    pub fn finished_runs_before(&self, cutoff: NaiveDateTime) -> anyhow::Result<Vec<Run>> {
        let sql = format!(
            "{} WHERE status IN ('succeeded', 'failed', 'cancelled') AND finished_at < ? ORDER BY id",
            run_select(false)
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![cutoff], |r| run_from_row(r, false))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Delete a run's rows from every table. Files on disk are the caller's
    /// job.
    pub fn delete_run(&mut self, id: i64) -> anyhow::Result<()> {
        let tx = self.conn.transaction()?;
        for table in ["step_events", "steps", "logs", "worktrees"] {
            tx.execute(&format!("DELETE FROM {table} WHERE run_id = ?"), params![id])?;
        }
        tx.execute("DELETE FROM runs WHERE id = ?", params![id])?;
        tx.commit()?;
        Ok(())
    }

    /// Move a run to a finished status. Errors if it's already finished.
    pub fn finish_run(&mut self, id: i64, status: RunStatus, reason: Option<&str>, summary: Option<&str>) -> CliResult<Run> {
        if !status.is_finished() {
            return Err(CliError::invalid(format!("`{}` is not a final run status", status.as_str())));
        }
        let run = self.require_run(id)?;
        if run.status.is_finished() {
            return Err(CliError::invalid(format!("run {id} has already finished ({})", run.status.as_str())));
        }
        self.conn
            .execute(
                "UPDATE runs SET status = ?, reason = ?, summary = coalesce(?, summary), finished_at = ? WHERE id = ?",
                params![status.as_str(), reason, summary, now(), id],
            )
            .map_err(internal)?;
        self.require_run(id)
    }

    /// Cancel an unfinished run: any step still running is failed with
    /// `reason`, then the run is marked `cancelled`.
    pub fn cancel_run(&mut self, id: i64, reason: &str) -> CliResult<Run> {
        let run = self.require_run(id)?;
        if run.status.is_finished() {
            return Err(CliError::invalid(format!("run {id} has already finished ({})", run.status.as_str())));
        }
        for step in self.steps(id).map_err(internal_any)? {
            if step.status == "running" {
                self.report_step(id, &step.name, StepEvent::Fail, Some(reason))?;
            }
        }
        self.finish_run(id, RunStatus::Cancelled, Some(reason), None)
    }

    // --- steps -------------------------------------------------------------

    /// The step a bare `tome step done|fail` refers to: the most recently
    /// started step that's still running.
    pub fn current_step(&self, run_id: i64) -> anyhow::Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT name FROM steps WHERE run_id = ? AND status = 'running' ORDER BY started_at DESC, name LIMIT 1",
                params![run_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Record a step transition reported by the orchestrator.
    pub fn report_step(&mut self, run_id: i64, step: &str, event: StepEvent, message: Option<&str>) -> CliResult<Step> {
        let step = step.trim();
        if step.is_empty() {
            return Err(CliError::invalid("step name must not be empty"));
        }
        let run = self.require_run(run_id)?;
        if run.status.is_finished() {
            return Err(CliError::invalid(format!("run {run_id} has already finished ({})", run.status.as_str())));
        }
        let ts = now();
        let tx = self.conn.transaction().map_err(internal)?;
        tx.execute(
            "INSERT INTO step_events (run_id, step, event, message, occurred_at) VALUES (?, ?, ?, ?, ?)",
            params![run_id, step, event.as_str(), message, ts],
        )
        .map_err(internal)?;
        let exists: bool = tx
            .query_row("SELECT count(*) > 0 FROM steps WHERE run_id = ? AND name = ?", params![run_id, step], |r| r.get(0))
            .map_err(internal)?;
        let status = event.resulting_status();
        match (exists, event) {
            (false, _) => {
                let finished = (event != StepEvent::Start).then_some(ts);
                tx.execute(
                    "INSERT INTO steps (run_id, name, status, attempts, message, started_at, finished_at) VALUES (?, ?, ?, 1, ?, ?, ?)",
                    params![run_id, step, status, message, ts, finished],
                )
                .map_err(internal)?;
            }
            (true, StepEvent::Start) => {
                tx.execute(
                    "UPDATE steps SET status = ?, attempts = attempts + 1, message = ?, started_at = ?, finished_at = NULL
                     WHERE run_id = ? AND name = ?",
                    params![status, message, ts, run_id, step],
                )
                .map_err(internal)?;
            }
            (true, _) => {
                tx.execute(
                    "UPDATE steps SET status = ?, message = ?, finished_at = ? WHERE run_id = ? AND name = ?",
                    params![status, message, ts, run_id, step],
                )
                .map_err(internal)?;
            }
        }
        tx.commit().map_err(internal)?;
        let steps = self.steps(run_id).map_err(internal_any)?;
        Ok(steps.into_iter().find(|s| s.name == step).expect("step just written"))
    }

    pub fn steps(&self, run_id: i64) -> anyhow::Result<Vec<Step>> {
        let mut stmt = self.conn.prepare(
            "SELECT name, status, attempts, message, started_at, finished_at FROM steps WHERE run_id = ?
             ORDER BY started_at NULLS LAST, name",
        )?;
        let rows = stmt.query_map(params![run_id], |r| {
            Ok(Step {
                name: r.get(0)?,
                status: r.get(1)?,
                attempts: r.get(2)?,
                message: r.get(3)?,
                started_at: r.get::<_, Option<NaiveDateTime>>(4)?.map(fmt_ts),
                finished_at: r.get::<_, Option<NaiveDateTime>>(5)?.map(fmt_ts),
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn step_history(&self, run_id: i64) -> anyhow::Result<Vec<StepHistory>> {
        let mut stmt =
            self.conn.prepare("SELECT id, step, event, message, occurred_at FROM step_events WHERE run_id = ? ORDER BY id")?;
        let rows = stmt.query_map(params![run_id], |r| {
            Ok(StepHistory {
                id: r.get(0)?,
                step: r.get(1)?,
                event: r.get(2)?,
                message: r.get(3)?,
                occurred_at: fmt_ts(r.get(4)?),
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    // --- logs --------------------------------------------------------------

    /// Re-scan `~/.tome/runs/<run-id>/*.log`, refresh the index (path, size,
    /// tail excerpt) and return it.
    pub fn index_logs(&mut self, run_id: i64) -> anyhow::Result<Vec<LogEntry>> {
        let dir = self.run_dir(run_id);
        let mut found = Vec::new();
        if let Ok(read) = std::fs::read_dir(&dir) {
            for entry in read.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "log") && path.is_file() {
                    let step = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
                    let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                    found.push((step, path, size));
                }
            }
        }
        let ts = now();
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM logs WHERE run_id = ?", params![run_id])?;
        for (step, path, size) in &found {
            tx.execute(
                "INSERT INTO logs (run_id, step, path, size, tail, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
                params![run_id, step, path.display().to_string(), *size as i64, read_tail(path, TAIL_LINES), ts],
            )?;
        }
        tx.commit()?;
        self.logs(run_id)
    }

    pub fn logs(&self, run_id: i64) -> anyhow::Result<Vec<LogEntry>> {
        let mut stmt =
            self.conn.prepare("SELECT step, path, size, tail, updated_at FROM logs WHERE run_id = ? ORDER BY step")?;
        let rows = stmt.query_map(params![run_id], |r| {
            Ok(LogEntry { step: r.get(0)?, path: r.get(1)?, size: r.get(2)?, tail: r.get(3)?, updated_at: fmt_ts(r.get(4)?) })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    // --- worktrees ---------------------------------------------------------

    pub fn add_worktree(&mut self, run_id: i64, path: &Path, repo_path: Option<&Path>, branch: Option<&str>) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO worktrees (run_id, path, repo_path, branch, created_at) VALUES (?, ?, ?, ?, ?)",
            params![run_id, path.display().to_string(), repo_path.map(|p| p.display().to_string()), branch, now()],
        )?;
        Ok(())
    }

    pub fn worktrees(&self, run_id: i64) -> anyhow::Result<Vec<Worktree>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path, repo_path, branch, created_at FROM worktrees WHERE run_id = ? ORDER BY created_at, path")?;
        let rows = stmt.query_map(params![run_id], |r| {
            Ok(Worktree { path: r.get(0)?, repo_path: r.get(1)?, branch: r.get(2)?, created_at: fmt_ts(r.get(3)?) })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

fn internal(e: duckdb::Error) -> CliError {
    CliError::internal(format!("database error: {e}"))
}

fn internal_any(e: anyhow::Error) -> CliError {
    CliError::internal(format!("{e:#}"))
}

fn run_select(with_snapshot: bool) -> String {
    format!(
        "SELECT id, workflow_name, workflow_path, project_path, params, status, reason, summary, created_at, finished_at{} FROM runs",
        if with_snapshot { ", workflow_snapshot" } else { "" }
    )
}

fn run_from_row(r: &Row<'_>, with_snapshot: bool) -> duckdb::Result<Run> {
    let params: String = r.get(4)?;
    let status: String = r.get(5)?;
    Ok(Run {
        id: r.get(0)?,
        workflow_name: r.get(1)?,
        workflow_path: r.get(2)?,
        project_path: r.get(3)?,
        params: serde_json::from_str(&params).unwrap_or(Value::Null),
        status: RunStatus::parse(&status).unwrap_or(RunStatus::Failed),
        reason: r.get(6)?,
        summary: r.get(7)?,
        created_at: fmt_ts(r.get(8)?),
        finished_at: r.get::<_, Option<NaiveDateTime>>(9)?.map(fmt_ts),
        workflow_snapshot: if with_snapshot { r.get(10)? } else { None },
    })
}

/// The last `lines` lines of a file, reading at most `TAIL_BYTES` from its end.
pub fn read_tail(path: &Path, lines: usize) -> String {
    read_tail_bytes(path, lines, TAIL_BYTES)
}

pub fn read_tail_bytes(path: &Path, lines: usize, max_bytes: u64) -> String {
    let Ok(mut file) = std::fs::File::open(path) else { return String::new() };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(max_bytes);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut buf = Vec::new();
    let _ = file.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    // If we started mid-file, drop the (probably partial) first line.
    let text = if start > 0 { text.split_once('\n').map(|(_, rest)| rest).unwrap_or("") } else { &text };
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(&dir.path().join("runs")).unwrap();
        (dir, store)
    }

    fn new_run(store: &mut Store, name: &str) -> Run {
        let params = json!({"base": "main"}).as_object().unwrap().clone();
        store
            .create_run(
                NewRun {
                    workflow_name: name,
                    workflow_path: None,
                    project_path: Some(Path::new("/proj")),
                    params: &params,
                    status: RunStatus::Running,
                },
                |id| format!("snapshot for run {id}"),
            )
            .unwrap()
    }

    #[test]
    fn migrations_apply_once() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("t.duckdb");
        let runs = dir.path().join("runs");
        {
            let store = Store::open(&db, &runs).unwrap();
            assert_eq!(store.schema_version().unwrap(), MIGRATIONS.len());
        }
        // Reopening doesn't re-apply (which would fail on CREATE TABLE).
        let store = Store::open(&db, &runs).unwrap();
        assert_eq!(store.schema_version().unwrap(), MIGRATIONS.len());
    }

    #[test]
    fn refuses_newer_schema() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("t.duckdb");
        let runs = dir.path().join("runs");
        {
            let store = Store::open(&db, &runs).unwrap();
            store.conn.execute("UPDATE schema_version SET version = 999", []).unwrap();
        }
        let err = Store::open(&db, &runs).err().unwrap();
        assert!(err.to_string().contains("newer"), "{err}");
    }

    #[test]
    fn run_snapshot_uses_allocated_id() {
        let (_d, mut store) = store();
        let a = new_run(&mut store, "wf");
        let b = new_run(&mut store, "wf");
        assert_eq!(b.id, a.id + 1);
        assert_eq!(b.workflow_snapshot.as_deref(), Some(format!("snapshot for run {}", b.id).as_str()));
        assert_eq!(b.status, RunStatus::Running);
        assert_eq!(b.params["base"], "main");
        assert_eq!(b.project_path.as_deref(), Some("/proj"));
        assert!(store.run_dir(b.id).is_dir());
    }

    #[test]
    fn step_status_and_history() {
        let (_d, mut store) = store();
        let run = new_run(&mut store, "wf");
        store.report_step(run.id, "Implement", StepEvent::Start, None).unwrap();
        store.report_step(run.id, "Implement", StepEvent::Done, Some("ok")).unwrap();
        store.report_step(run.id, "Review", StepEvent::Start, None).unwrap();
        store.report_step(run.id, "Review", StepEvent::Fail, Some("changes requested")).unwrap();
        let again = store.report_step(run.id, "Implement", StepEvent::Start, None).unwrap();
        assert_eq!(again.attempts, 2);
        assert_eq!(again.status, "running");
        assert!(again.finished_at.is_none());

        let steps = store.steps(run.id).unwrap();
        let review = steps.iter().find(|s| s.name == "Review").unwrap();
        assert_eq!(review.status, "failed");
        assert_eq!(review.message.as_deref(), Some("changes requested"));

        let history = store.step_history(run.id).unwrap();
        let events: Vec<_> = history.iter().map(|h| format!("{}:{}", h.step, h.event)).collect();
        assert_eq!(events, ["Implement:start", "Implement:done", "Review:start", "Review:fail", "Implement:start"]);
    }

    #[test]
    fn finishing_is_final() {
        let (_d, mut store) = store();
        let run = new_run(&mut store, "wf");
        let done = store.finish_run(run.id, RunStatus::Succeeded, None, Some("all good")).unwrap();
        assert_eq!(done.status, RunStatus::Succeeded);
        assert!(done.finished_at.is_some());
        assert!(store.finish_run(run.id, RunStatus::Failed, None, None).is_err());
        assert!(store.report_step(run.id, "x", StepEvent::Start, None).is_err());
        assert_eq!(store.finish_run(999, RunStatus::Failed, None, None).unwrap_err().kind, crate::output::ErrorKind::NotFound);
    }

    #[test]
    fn cancel_fails_running_steps() {
        let (_d, mut store) = store();
        let run = new_run(&mut store, "wf");
        store.report_step(run.id, "Build", StepEvent::Start, None).unwrap();
        store.report_step(run.id, "Build", StepEvent::Done, None).unwrap();
        store.report_step(run.id, "Test", StepEvent::Start, None).unwrap();
        assert_eq!(store.current_step(run.id).unwrap().as_deref(), Some("Test"));
        let done = store.cancel_run(run.id, "user_cancelled").unwrap();
        assert_eq!(done.status, RunStatus::Cancelled);
        assert_eq!(done.reason.as_deref(), Some("user_cancelled"));
        let steps = store.steps(run.id).unwrap();
        assert_eq!(steps.iter().map(|s| s.status.as_str()).collect::<Vec<_>>(), ["done", "failed"]);
        assert!(store.current_step(run.id).unwrap().is_none());
        assert!(store.cancel_run(run.id, "again").is_err());
    }

    #[test]
    fn list_filters() {
        let (_d, mut store) = store();
        let a = new_run(&mut store, "alpha");
        new_run(&mut store, "beta");
        store.finish_run(a.id, RunStatus::Failed, Some("boom"), None).unwrap();
        assert_eq!(store.list_runs(&RunFilter::default()).unwrap().len(), 2);
        let failed = store.list_runs(&RunFilter { status: Some(RunStatus::Failed), ..Default::default() }).unwrap();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].reason.as_deref(), Some("boom"));
        let beta = store.list_runs(&RunFilter { workflow: Some("beta".into()), ..Default::default() }).unwrap();
        assert_eq!(beta[0].workflow_name, "beta");
        assert_eq!(store.in_progress_runs().unwrap().len(), 1);
    }

    #[test]
    fn indexes_log_files() {
        let (_d, mut store) = store();
        let run = new_run(&mut store, "wf");
        let path = store.log_path(run.id, "Implement / build");
        assert!(path.ends_with("Implement _ build.log"));
        let content: String = (1..=50).map(|i| format!("line {i}\n")).collect();
        std::fs::write(&path, &content).unwrap();
        let logs = store.index_logs(run.id).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].size, content.len() as i64);
        assert!(logs[0].tail.starts_with("line 31\n"));
        assert!(logs[0].tail.ends_with("line 50"));

        std::fs::remove_file(&path).unwrap();
        assert!(store.index_logs(run.id).unwrap().is_empty());
    }

    #[test]
    fn tail_drops_partial_first_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.log");
        std::fs::write(&path, "aaaaaaaaaa\nbb\ncc\n").unwrap();
        assert_eq!(read_tail_bytes(&path, 10, 8), "bb\ncc");
        assert_eq!(read_tail_bytes(&path, 1, 1000), "cc");
    }
}
