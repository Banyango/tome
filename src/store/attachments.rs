//! Which hubs (and whether global) each project is attached to. Recorded per
//! machine, never in the repo.

use super::{internal, now, Store};
use crate::output::CliResult;
use crate::scope::Shared;
use duckdb::params;

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
}
