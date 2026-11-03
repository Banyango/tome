//! Which hubs (and whether global) each project is attached to. Recorded per
//! machine, never in the repo.

use super::{internal, now, Store};
use crate::output::CliResult;
use crate::scope::Shared;
use duckdb::params;
use std::collections::BTreeMap;

impl Store {
    /// Attach a project to a hub or global. Returns whether it was new.
    pub fn attach(&mut self, project: &str, to: &Shared) -> CliResult<bool> {
        let n = self
            .conn
            .execute(
                "INSERT INTO attachments (project_path, scope, attached_at) VALUES (?, ?, ?) ON CONFLICT DO NOTHING",
                params![project, to.key(), now()],
            )
            .map_err(internal)?;
        Ok(n > 0)
    }

    /// Returns whether it was attached.
    pub fn detach(&mut self, project: &str, from: &Shared) -> CliResult<bool> {
        let n = self
            .conn
            .execute(
                "DELETE FROM attachments WHERE project_path = ? AND scope = ?",
                params![project, from.key()],
            )
            .map_err(internal)?;
        Ok(n > 0)
    }

    /// After `project` detaches from `from`: drop its pending deliveries of
    /// that bus's events and return the messages it claimed from that
    /// bus's queues. Returns (dropped, released).
    pub fn detach_cleanup(&mut self, project: &str, from: &Shared) -> CliResult<(usize, usize)> {
        let key = from.key();
        let dropped = self
            .conn
            .execute(
                "DELETE FROM deliveries WHERE project_path = ? AND state = 'pending' \
                 AND event_id IN (SELECT id FROM bus_events WHERE project_path = ?)",
                params![project, key],
            )
            .map_err(internal)?;
        let released = self
            .conn
            .execute(
                "UPDATE queue_messages SET claimed_by = NULL, claimed_at = NULL, claimer_project = NULL \
                 WHERE project_path = ? AND claimer_project = ?",
                params![key, project],
            )
            .map_err(internal)?;
        Ok((dropped, released))
    }

    /// Work still waiting on a hub or global: pending deliveries (as
    /// `project` lines) and queued messages (as `queue <name>` lines),
    /// each with its count.
    pub fn shared_pending(&self, of: &Shared) -> CliResult<Vec<(String, i64)>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT d.project_path, count(*) FROM deliveries d JOIN bus_events e ON e.id = d.event_id \
                 WHERE e.project_path = ? AND d.state = 'pending' GROUP BY 1 \
                 UNION ALL SELECT 'queue ' || queue, count(*) FROM queue_messages WHERE project_path = ? GROUP BY 1 \
                 ORDER BY 1",
            )
            .map_err(internal)?;
        let key = of.key();
        let rows = stmt
            .query_map(params![key, key], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(internal)?;
        rows.collect::<Result<_, _>>().map_err(internal)
    }

    /// Delete everything stored for a hub or global: attachments,
    /// deliveries of its events, its events, queues and messages.
    pub fn purge_shared(&mut self, of: &Shared) -> CliResult<()> {
        let key = of.key();
        for sql in [
            "DELETE FROM attachments WHERE scope = ?",
            "DELETE FROM deliveries WHERE event_id IN (SELECT id FROM bus_events WHERE project_path = ?)",
            "DELETE FROM bus_events WHERE project_path = ?",
            "DELETE FROM queue_messages WHERE project_path = ?",
            "DELETE FROM queues WHERE project_path = ?",
        ] {
            self.conn.execute(sql, params![key]).map_err(internal)?;
        }
        Ok(())
    }

    pub fn is_attached(&self, project: &str, to: &Shared) -> CliResult<bool> {
        let n: i64 = self
            .conn
            .query_row(
                "SELECT count(*) FROM attachments WHERE project_path = ? AND scope = ?",
                params![project, to.key()],
                |r| r.get(0),
            )
            .map_err(internal)?;
        Ok(n > 0)
    }

    /// What a project is attached to, in the order it attached.
    pub fn attachments_of(&self, project: &str) -> CliResult<Vec<Shared>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT scope FROM attachments WHERE project_path = ? ORDER BY attached_at, scope",
            )
            .map_err(internal)?;
        let rows = stmt
            .query_map(params![project], |r| r.get::<_, String>(0))
            .map_err(internal)?;
        let keys: Vec<String> = rows.collect::<Result<_, _>>().map_err(internal)?;
        Ok(keys.iter().filter_map(|k| Shared::from_key(k)).collect())
    }

    /// Rewrite `~/.tome/attachments.json` from the table, for
    /// [`crate::layers`].
    pub fn mirror_attachments(&self) -> CliResult<()> {
        let mut stmt = self
            .conn
            .prepare("SELECT project_path, scope FROM attachments ORDER BY attached_at, scope")
            .map_err(internal)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(internal)?;
        let mut all: BTreeMap<String, Vec<Shared>> = BTreeMap::new();
        for row in rows {
            let (project, key) = row.map_err(internal)?;
            if let Some(s) = Shared::from_key(&key) {
                all.entry(project).or_default().push(s);
            }
        }
        crate::layers::write_mirror(&all);
        Ok(())
    }

    /// The projects attached to a hub or global.
    pub fn attached_projects(&self, to: &Shared) -> CliResult<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT project_path FROM attachments WHERE scope = ? ORDER BY project_path")
            .map_err(internal)?;
        let rows = stmt
            .query_map(params![to.key()], |r| r.get(0))
            .map_err(internal)?;
        rows.collect::<Result<_, _>>().map_err(internal)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::store;
    use crate::scope::{HubName, Shared};

    #[test]
    fn attach_detach() {
        let (_d, mut store) = store();
        let hub = Shared::Hub(HubName::new("release").unwrap());
        assert!(store.attach("/p", &hub).unwrap());
        assert!(!store.attach("/p", &hub).unwrap());
        store.attach("/p", &Shared::Global).unwrap();
        store.attach("/q", &hub).unwrap();
        assert_eq!(
            store.attachments_of("/p").unwrap(),
            [hub.clone(), Shared::Global]
        );
        assert_eq!(store.attached_projects(&hub).unwrap(), ["/p", "/q"]);
        assert!(store.is_attached("/q", &hub).unwrap());
        assert!(store.detach("/q", &hub).unwrap());
        assert!(!store.detach("/q", &hub).unwrap());
        assert!(!store.is_attached("/q", &hub).unwrap());
    }

    #[test]
    fn detach_returns_claims_and_purge_clears() {
        let (_d, mut store) = store();
        let hub = Shared::Hub(HubName::new("release").unwrap());
        let key = hub.key();
        store.attach("/p", &hub).unwrap();
        store.push_message(&key, "q", "a", "s").unwrap();
        store.push_message(&key, "q", "b", "s").unwrap();
        store.pull_message(&key, "q", "user", Some("/p")).unwrap();
        store
            .pull_message(&key, "q", "user", Some("/other"))
            .unwrap();
        assert_eq!(store.detach_cleanup("/p", &hub).unwrap(), (0, 1));
        assert_eq!(
            store.shared_pending(&hub).unwrap(),
            [("queue q".to_string(), 2)]
        );
        store.purge_shared(&hub).unwrap();
        assert!(store.shared_pending(&hub).unwrap().is_empty());
        assert!(store.attached_projects(&hub).unwrap().is_empty());
    }
}
