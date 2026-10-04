//! The project message bus: published events, and the delivery each
//! subscribing workflow gets of one.

use super::queues::MAX_MESSAGE_BYTES;
use super::{fmt_ts, internal, now, Store};
use crate::output::{CliError, CliResult};
use duckdb::{params, OptionalExt, Row};
use serde::Serialize;
use serde_json::Value;

/// Delivery states.
pub mod state {
    /// Waiting for its workflow to take it.
    pub const PENDING: &str = "pending";
    /// Taken by a run (started for it, or signalled with it).
    pub const CLAIMED: &str = "claimed";
    pub const DONE: &str = "done";
    /// The run that claimed it didn't succeed; parked until retried.
    pub const FAILED: &str = "failed";
    /// Its subscription went away, or it was removed by hand.
    pub const DROPPED: &str = "dropped";
}

#[derive(Debug, Clone, Serialize)]
pub struct BusEvent {
    pub id: i64,
    pub project_path: String,
    pub topic: String,
    pub payload: String,
    /// `user`, `run N`, `run N worker W`, `tome` or `test`.
    pub sender: String,
    pub sender_run_id: Option<i64>,
    pub depth: i64,
    /// Why it wasn't delivered (a chain too deep), if it wasn't.
    pub refused: Option<String>,
    pub published_at: String,
}

pub struct NewEvent<'a> {
    pub project_path: &'a str,
    pub topic: &'a str,
    pub payload: &'a str,
    pub sender: &'a str,
    pub sender_run_id: Option<i64>,
    pub depth: i64,
    pub refused: Option<&'a str>,
}

/// A workflow subscribed to an event's topic, by its first matching `on:`
/// trigger.
#[derive(Debug, Clone)]
pub struct Subscriber {
    pub workflow_name: String,
    pub workflow_path: String,
    pub pattern: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Delivery {
    pub id: i64,
    pub event_id: i64,
    pub project_path: String,
    pub workflow: String,
    pub workflow_path: String,
    pub pattern: String,
    pub state: String,
    /// The runs that claimed it.
    pub run_ids: Vec<i64>,
    pub updated_at: String,
}

const EVENT_COLUMNS: &str =
    "id, project_path, topic, payload, sender, sender_run_id, depth, refused, published_at";
const DELIVERY_COLUMNS: &str =
    "id, event_id, project_path, workflow_name, workflow_path, pattern, state, run_ids, updated_at";

fn event_from_row(r: &Row<'_>) -> duckdb::Result<BusEvent> {
    Ok(BusEvent {
        id: r.get(0)?,
        project_path: r.get(1)?,
        topic: r.get(2)?,
        payload: r.get(3)?,
        sender: r.get(4)?,
        sender_run_id: r.get(5)?,
        depth: r.get(6)?,
        refused: r.get(7)?,
        published_at: fmt_ts(r.get(8)?),
    })
}

fn delivery_from_row(r: &Row<'_>) -> duckdb::Result<Delivery> {
    let ids: String = r.get(7)?;
    Ok(Delivery {
        id: r.get(0)?,
        event_id: r.get(1)?,
        project_path: r.get(2)?,
        workflow: r.get(3)?,
        workflow_path: r.get(4)?,
        pattern: r.get(5)?,
        state: r.get(6)?,
        run_ids: serde_json::from_str(&ids).unwrap_or_default(),
        updated_at: fmt_ts(r.get(8)?),
    })
}

fn ids_json(ids: &[i64]) -> String {
    Value::from(ids.to_vec()).to_string()
}

/// Refuse a payload over the queue message limit.
pub fn check_payload(payload: &str) -> CliResult<()> {
    if payload.len() > MAX_MESSAGE_BYTES {
        return Err(CliError::invalid(format!(
            "payload is {} bytes; the limit is 1 MiB",
            payload.len()
        ))
        .with_hint("write the data to a file and publish the file path instead"));
    }
    Ok(())
}

impl Store {
    /// Record an event and a `pending` delivery for each subscriber (none
    /// if it was refused).
    pub fn publish_event(
        &mut self,
        e: &NewEvent<'_>,
        subscribers: &[Subscriber],
    ) -> CliResult<(BusEvent, Vec<Delivery>)> {
        check_payload(e.payload)?;
        let tx = self.conn.transaction().map_err(internal)?;
        let at = now();
        let id: i64 = tx
            .query_row(
                "INSERT INTO bus_events (project_path, topic, payload, sender, sender_run_id, depth, refused, published_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
                params![e.project_path, e.topic, e.payload, e.sender, e.sender_run_id, e.depth, e.refused, at],
                |r| r.get(0),
            )
            .map_err(internal)?;
        if e.refused.is_none() {
            for s in subscribers {
                tx.execute(
                    "INSERT INTO deliveries (event_id, project_path, workflow_name, workflow_path, pattern, state, run_ids, updated_at)
                     VALUES (?, ?, ?, ?, ?, ?, '[]', ?)",
                    params![id, e.project_path, s.workflow_name, s.workflow_path, s.pattern, state::PENDING, at],
                )
                .map_err(internal)?;
            }
        }
        tx.commit().map_err(internal)?;
        let event = self.bus_event(id)?.expect("event just recorded");
        Ok((event, self.deliveries_of(id)?))
    }

    pub fn bus_event(&self, id: i64) -> CliResult<Option<BusEvent>> {
        let sql = format!("SELECT {EVENT_COLUMNS} FROM bus_events WHERE id = ?");
        self.conn
            .query_row(&sql, params![id], event_from_row)
            .optional()
            .map_err(internal)
    }

    /// Delete an event outright. Only for one with no deliveries.
    pub fn delete_bus_event(&mut self, id: i64) -> CliResult<()> {
        self.conn
            .execute("DELETE FROM bus_events WHERE id = ?", params![id])
            .map_err(internal)?;
        Ok(())
    }

    /// A project's events on `topic`, oldest first.
    pub fn bus_events(&self, project: &str, topic: &str) -> CliResult<Vec<BusEvent>> {
        let sql = format!("SELECT {EVENT_COLUMNS} FROM bus_events WHERE project_path = ? AND topic = ? ORDER BY id");
        let mut stmt = self.conn.prepare(&sql).map_err(internal)?;
        let rows = stmt
            .query_map(params![project, topic], event_from_row)
            .map_err(internal)?;
        rows.collect::<Result<_, _>>().map_err(internal)
    }

    /// Each topic of a project: `(topic, events, last published)`.
    pub fn bus_topics(&self, project: &str) -> CliResult<Vec<(String, i64, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT topic, count(*), max(published_at) FROM bus_events WHERE project_path = ? GROUP BY topic ORDER BY topic")
            .map_err(internal)?;
        let rows = stmt
            .query_map(params![project], |r| {
                Ok((r.get(0)?, r.get(1)?, fmt_ts(r.get(2)?)))
            })
            .map_err(internal)?;
        rows.collect::<Result<_, _>>().map_err(internal)
    }

    fn deliveries_where(
        &self,
        clause: &str,
        args: &[&dyn duckdb::ToSql],
    ) -> CliResult<Vec<Delivery>> {
        let sql = format!("SELECT {DELIVERY_COLUMNS} FROM deliveries WHERE {clause} ORDER BY id");
        let mut stmt = self.conn.prepare(&sql).map_err(internal)?;
        let rows = stmt.query_map(args, delivery_from_row).map_err(internal)?;
        rows.collect::<Result<_, _>>().map_err(internal)
    }

    pub fn deliveries_of(&self, event_id: i64) -> CliResult<Vec<Delivery>> {
        self.deliveries_where("event_id = ?", &[&event_id])
    }

    pub fn delivery(&self, id: i64) -> CliResult<Option<Delivery>> {
        Ok(self.deliveries_where("id = ?", &[&id])?.pop())
    }

    /// A project's deliveries for one subscription in `state`, in publish
    /// order.
    pub fn subscription_deliveries(
        &self,
        project: &str,
        workflow: &str,
        pattern: &str,
        state: &str,
    ) -> CliResult<Vec<Delivery>> {
        self.deliveries_where(
            "project_path = ? AND workflow_name = ? AND pattern = ? AND state = ?",
            &[&project, &workflow, &pattern, &state],
        )
    }

    /// Every delivery of a project.
    pub fn project_deliveries(&self, project: &str) -> CliResult<Vec<Delivery>> {
        self.deliveries_where("project_path = ?", &[&project])
    }

    pub fn deliveries_in(&self, state: &str) -> CliResult<Vec<Delivery>> {
        self.deliveries_where("state = ?", &[&state])
    }

    /// Move a delivery from `from` to `to` (recording `runs` if given).
    /// False if it wasn't in `from` (any more).
    pub fn move_delivery(
        &mut self,
        id: i64,
        from: &str,
        to: &str,
        runs: Option<&[i64]>,
    ) -> CliResult<bool> {
        let n = match runs {
            Some(runs) => self.conn.execute(
                "UPDATE deliveries SET state = ?, run_ids = ?, updated_at = ? WHERE id = ? AND state = ?",
                params![to, ids_json(runs), now(), id, from],
            ),
            None => self
                .conn
                .execute("UPDATE deliveries SET state = ?, updated_at = ? WHERE id = ? AND state = ?", params![to, now(), id, from]),
        }
        .map_err(internal)?;
        Ok(n > 0)
    }

    /// Record the runs that claimed a delivery.
    pub fn set_delivery_runs(&mut self, id: i64, runs: &[i64]) -> CliResult<()> {
        self.conn
            .execute(
                "UPDATE deliveries SET run_ids = ?, updated_at = ? WHERE id = ?",
                params![ids_json(runs), now(), id],
            )
            .map_err(internal)?;
        Ok(())
    }

    /// Drop the pending deliveries of a project whose subscription is gone:
    /// neither in `live` (workflow name, pattern) nor of a workflow in
    /// `keep` (ones that are invalid right now). Returns them.
    pub fn drop_unsubscribed(
        &mut self,
        project: &str,
        live: &[(String, String)],
        keep: &[String],
    ) -> CliResult<Vec<Delivery>> {
        let gone: Vec<Delivery> = self
            .deliveries_where(
                "project_path = ? AND state = ?",
                &[&project, &state::PENDING],
            )?
            .into_iter()
            // Forward deliveries aren't subscriptions; they stay.
            .filter(|d| !d.workflow_path.starts_with("node:"))
            .filter(|d| {
                !keep.contains(&d.workflow)
                    && !live
                        .iter()
                        .any(|(w, p)| *w == d.workflow && *p == d.pattern)
            })
            .collect();
        for d in &gone {
            self.move_delivery(d.id, state::PENDING, state::DROPPED, None)?;
        }
        Ok(gone)
    }

    /// Remove `done` deliveries, then events with no deliveries left.
    /// Returns how many of each.
    pub fn gc_bus(&mut self, dry_run: bool) -> CliResult<(usize, usize)> {
        let count = |sql: &str| -> CliResult<usize> {
            let n: i64 = self
                .conn
                .query_row(sql, [], |r| r.get(0))
                .map_err(internal)?;
            Ok(n as usize)
        };
        let done = count("SELECT count(*) FROM deliveries WHERE state = 'done'")?;
        let empty = "id NOT IN (SELECT event_id FROM deliveries WHERE state <> 'done')";
        let events = count(&format!("SELECT count(*) FROM bus_events WHERE {empty}"))?;
        if !dry_run {
            self.conn
                .execute("DELETE FROM deliveries WHERE state = 'done'", [])
                .map_err(internal)?;
            self.conn
                .execute(&format!("DELETE FROM bus_events WHERE {empty}"), [])
                .map_err(internal)?;
        }
        Ok((done, events))
    }

    // --- run lifecycle ---------------------------------------------------

    /// Which lifecycle events have been published for a run: `None`,
    /// `started` or `ended`.
    pub fn announced(&self, run_id: i64) -> CliResult<Option<String>> {
        self.conn
            .query_row(
                "SELECT announced FROM runs WHERE id = ?",
                params![run_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(internal)
            .map(Option::flatten)
    }

    pub fn set_announced(&mut self, run_id: i64, what: &str) -> CliResult<()> {
        self.conn
            .execute(
                "UPDATE runs SET announced = ? WHERE id = ?",
                params![what, run_id],
            )
            .map_err(internal)?;
        Ok(())
    }

    /// Finished runs whose end hasn't been handled yet (lifecycle event,
    /// deliveries settled).
    pub fn unannounced_ends(&self) -> CliResult<Vec<i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM runs WHERE finished_at IS NOT NULL AND coalesce(announced, '') <> 'ended' ORDER BY finished_at, id")
            .map_err(internal)?;
        let rows = stmt.query_map([], |r| r.get(0)).map_err(internal)?;
        rows.collect::<Result<_, _>>().map_err(internal)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::store;
    use super::*;

    fn sub(wf: &str, pattern: &str) -> Subscriber {
        Subscriber {
            workflow_name: wf.into(),
            workflow_path: format!("/p/.tome/workflows/{wf}.md"),
            pattern: pattern.into(),
        }
    }

    fn event<'a>(topic: &'a str, payload: &'a str) -> NewEvent<'a> {
        NewEvent {
            project_path: "/p",
            topic,
            payload,
            sender: "user",
            sender_run_id: None,
            depth: 0,
            refused: None,
        }
    }

    #[test]
    fn events_fan_out_to_pending_deliveries() {
        let (_d, mut store) = store();
        let (e, ds) = store
            .publish_event(
                &event("review.requested", "a"),
                &[sub("review", "review.*"), sub("audit", "**")],
            )
            .unwrap();
        assert_eq!(
            (e.topic.as_str(), e.sender.as_str(), e.depth),
            ("review.requested", "user", 0)
        );
        assert_eq!(ds.len(), 2);
        assert!(ds
            .iter()
            .all(|d| d.state == state::PENDING && d.run_ids.is_empty()));
        let (none, ds) = store
            .publish_event(&event("nobody.listens", "b"), &[])
            .unwrap();
        assert!(ds.is_empty());
        assert_eq!(
            store.bus_events("/p", "nobody.listens").unwrap()[0].id,
            none.id
        );
        let refused = NewEvent {
            refused: Some("too deep"),
            ..event("review.requested", "c")
        };
        assert!(store
            .publish_event(&refused, &[sub("review", "review.*")])
            .unwrap()
            .1
            .is_empty());
        assert_eq!(
            store
                .bus_topics("/p")
                .unwrap()
                .iter()
                .map(|t| (t.0.as_str(), t.1))
                .collect::<Vec<_>>(),
            [("nobody.listens", 1), ("review.requested", 2)]
        );

        let big = "x".repeat(MAX_MESSAGE_BYTES + 1);
        let err = store.publish_event(&event("t", &big), &[]).unwrap_err();
        assert!(err.hint.unwrap().contains("file path"));
    }

    #[test]
    fn deliveries_move_between_states_once() {
        let (_d, mut store) = store();
        store
            .publish_event(&event("a", "1"), &[sub("w", "a")])
            .unwrap();
        store
            .publish_event(&event("a", "2"), &[sub("w", "a")])
            .unwrap();
        let pending = store
            .subscription_deliveries("/p", "w", "a", state::PENDING)
            .unwrap();
        assert_eq!(pending.len(), 2);
        let first = pending[0].id;
        assert!(store
            .move_delivery(first, state::PENDING, state::CLAIMED, Some(&[7]))
            .unwrap());
        assert!(
            !store
                .move_delivery(first, state::PENDING, state::CLAIMED, Some(&[8]))
                .unwrap(),
            "claimed once"
        );
        assert_eq!(store.delivery(first).unwrap().unwrap().run_ids, [7]);
        assert_eq!(store.deliveries_in(state::CLAIMED).unwrap().len(), 1);

        // The subscription goes away: its pending delivery is dropped.
        let dropped = store
            .drop_unsubscribed("/p", &[("w".into(), "b".into())], &[])
            .unwrap();
        assert_eq!(dropped.len(), 1);
        assert_eq!(
            store.delivery(dropped[0].id).unwrap().unwrap().state,
            state::DROPPED
        );
        assert_eq!(
            store.delivery(first).unwrap().unwrap().state,
            state::CLAIMED,
            "claimed ones stay"
        );

        store
            .move_delivery(first, state::CLAIMED, state::DONE, None)
            .unwrap();
        assert_eq!(store.gc_bus(true).unwrap(), (1, 1));
        assert_eq!(store.gc_bus(false).unwrap(), (1, 1));
        assert!(store.delivery(first).unwrap().is_none());
        assert_eq!(
            store.bus_events("/p", "a").unwrap().len(),
            1,
            "the event with a dropped delivery stays"
        );
    }

    #[test]
    fn invalid_workflows_keep_their_pending_deliveries() {
        let (_d, mut store) = store();
        store
            .publish_event(&event("a", "1"), &[sub("w", "a")])
            .unwrap();
        assert!(store
            .drop_unsubscribed("/p", &[], &["w".into()])
            .unwrap()
            .is_empty());
    }
}
