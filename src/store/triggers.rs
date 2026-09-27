//! Trigger bookkeeping: every fire's outcome, and the registry of projects
//! whose triggers the daemon arms.

use super::{fmt_ts, internal, now, Store};
use crate::output::CliResult;
use duckdb::{params, OptionalExt, Row};
use serde::Serialize;
use serde_json::Value;

/// What a fired trigger did.
#[derive(Debug, Clone, Serialize)]
pub struct Fire {
    pub id: i64,
    pub workflow_path: String,
    pub workflow_name: String,
    pub project_path: Option<String>,
    pub trigger_index: i64,
    pub trigger: String,
    /// `started`, `signalled`, `no_target`, `muted`, `rejected` or `error`.
    pub outcome: String,
    pub message: Option<String>,
    pub run_ids: Vec<i64>,
    pub fired_at: String,
}

pub struct NewFire<'a> {
    pub workflow_path: &'a str,
    pub workflow_name: &'a str,
    pub project_path: Option<&'a str>,
    pub trigger_index: usize,
    pub trigger: &'a str,
    pub outcome: &'a str,
    pub message: Option<&'a str>,
    pub run_ids: &'a [i64],
}

#[derive(Debug, Clone, Serialize)]
pub struct Project {
    pub path: String,
    pub enabled: bool,
    pub registered_at: String,
}

const FIRE_COLUMNS: &str =
    "id, workflow_path, workflow_name, project_path, trigger_index, trigger_desc, outcome, message, run_ids, fired_at";

fn fire_from_row(r: &Row<'_>) -> duckdb::Result<Fire> {
    let ids: String = r.get(8)?;
    Ok(Fire {
        id: r.get(0)?,
        workflow_path: r.get(1)?,
        workflow_name: r.get(2)?,
        project_path: r.get(3)?,
        trigger_index: r.get(4)?,
        trigger: r.get(5)?,
        outcome: r.get(6)?,
        message: r.get(7)?,
        run_ids: serde_json::from_str(&ids).unwrap_or_default(),
        fired_at: fmt_ts(r.get(9)?),
    })
}

impl Store {
    pub fn record_fire(&mut self, f: &NewFire<'_>) -> CliResult<Fire> {
        let id: i64 = self
            .conn
            .query_row(
                "INSERT INTO trigger_fires (workflow_path, workflow_name, project_path, trigger_index, trigger_desc, outcome, message, run_ids, fired_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
                params![
                    f.workflow_path,
                    f.workflow_name,
                    f.project_path,
                    f.trigger_index as i64,
                    f.trigger,
                    f.outcome,
                    f.message,
                    Value::from(f.run_ids.to_vec()).to_string(),
                    now()
                ],
                |r| r.get(0),
            )
            .map_err(internal)?;
        let sql = format!("SELECT {FIRE_COLUMNS} FROM trigger_fires WHERE id = ?");
        self.conn.query_row(&sql, params![id], fire_from_row).map_err(internal)
    }

    /// The latest fire of each (workflow file, trigger index, project).
    pub fn last_fires(&self) -> CliResult<Vec<Fire>> {
        let sql = format!(
            "SELECT {FIRE_COLUMNS} FROM trigger_fires WHERE id IN (
                 SELECT max(id) FROM trigger_fires GROUP BY workflow_path, trigger_index, project_path
             ) ORDER BY id"
        );
        let mut stmt = self.conn.prepare(&sql).map_err(internal)?;
        let rows = stmt.query_map([], fire_from_row).map_err(internal)?;
        rows.collect::<Result<_, _>>().map_err(internal)
    }

    pub fn fires(&self, limit: usize) -> CliResult<Vec<Fire>> {
        let sql = format!("SELECT {FIRE_COLUMNS} FROM trigger_fires ORDER BY id DESC LIMIT {limit}");
        let mut stmt = self.conn.prepare(&sql).map_err(internal)?;
        let rows = stmt.query_map([], fire_from_row).map_err(internal)?;
        rows.collect::<Result<_, _>>().map_err(internal)
    }

    // --- projects ------------------------------------------------------------

    /// Register a project (enabled). Returns whether it was new.
    pub fn register_project(&mut self, path: &str) -> CliResult<bool> {
        let n = self
            .conn
            .execute(
                "INSERT INTO projects (path, enabled, registered_at) VALUES (?, true, ?) ON CONFLICT DO NOTHING",
                params![path, now()],
            )
            .map_err(internal)?;
        Ok(n > 0)
    }

    pub fn project(&self, path: &str) -> CliResult<Option<Project>> {
        self.conn
            .query_row("SELECT path, enabled, registered_at FROM projects WHERE path = ?", params![path], |r| {
                Ok(Project { path: r.get(0)?, enabled: r.get(1)?, registered_at: fmt_ts(r.get(2)?) })
            })
            .optional()
            .map_err(internal)
    }

    pub fn projects(&self) -> CliResult<Vec<Project>> {
        let mut stmt = self.conn.prepare("SELECT path, enabled, registered_at FROM projects ORDER BY path").map_err(internal)?;
        let rows = stmt
            .query_map([], |r| Ok(Project { path: r.get(0)?, enabled: r.get(1)?, registered_at: fmt_ts(r.get(2)?) }))
            .map_err(internal)?;
        rows.collect::<Result<_, _>>().map_err(internal)
    }

    /// Enable or disable a project's triggers, registering it if needed.
    pub fn set_project_enabled(&mut self, path: &str, enabled: bool) -> CliResult<Project> {
        self.register_project(path)?;
        self.conn.execute("UPDATE projects SET enabled = ? WHERE path = ?", params![enabled, path]).map_err(internal)?;
        Ok(self.project(path)?.expect("project just registered"))
    }

    pub fn drop_project(&mut self, path: &str) -> CliResult<()> {
        self.conn.execute("DELETE FROM projects WHERE path = ?", params![path]).map_err(internal)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::store;
    use super::*;

    fn fire<'a>(outcome: &'a str, index: usize, runs: &'a [i64]) -> NewFire<'a> {
        NewFire {
            workflow_path: "/p/.tome/workflows/a.md",
            workflow_name: "a",
            project_path: Some("/p"),
            trigger_index: index,
            trigger: "cron * * * * *",
            outcome,
            message: None,
            run_ids: runs,
        }
    }

    #[test]
    fn fires_keep_the_latest_per_trigger() {
        let (_d, mut store) = store();
        store.record_fire(&fire("started", 1, &[1])).unwrap();
        store.record_fire(&fire("rejected", 1, &[])).unwrap();
        let f = store.record_fire(&fire("started", 2, &[3, 4])).unwrap();
        assert_eq!(f.run_ids, vec![3, 4]);
        let last = store.last_fires().unwrap();
        let summary: Vec<(i64, &str)> = last.iter().map(|f| (f.trigger_index, f.outcome.as_str())).collect();
        assert_eq!(summary, vec![(1, "rejected"), (2, "started")]);
        assert_eq!(store.fires(10).unwrap().len(), 3);
    }

    #[test]
    fn project_registry() {
        let (_d, mut store) = store();
        assert!(store.register_project("/p").unwrap());
        assert!(!store.register_project("/p").unwrap());
        assert!(!store.set_project_enabled("/p", false).unwrap().enabled);
        assert!(!store.register_project("/p").unwrap(), "registering again keeps it disabled");
        assert!(!store.project("/p").unwrap().unwrap().enabled);
        assert!(store.set_project_enabled("/q", true).unwrap().enabled);
        assert_eq!(store.projects().unwrap().len(), 2);
        store.drop_project("/p").unwrap();
        assert_eq!(store.projects().unwrap().len(), 1);
    }
}
