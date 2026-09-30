//! A run's message queues. A message is claimed by whoever pulls it and
//! removed when they ack it; if the claimer (a worker) ends first, the
//! message goes back on the queue.

use super::workers::check_name;
use super::{fmt_ts, internal, now, Store};
use crate::output::{CliError, CliResult};
use duckdb::{params, OptionalExt};
use serde::Serialize;

/// Largest message body; bigger data belongs in a file whose path is sent.
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct QueueMessage {
    pub id: i64,
    pub queue: String,
    pub body: String,
    /// `orchestrator` or a worker name.
    pub sender: String,
    pub created_at: String,
}

/// What a pull found.
#[derive(Debug, Clone)]
pub enum Pulled {
    Message(QueueMessage),
    /// Nothing to claim right now.
    Empty,
    /// Nothing to claim, and nothing more will come.
    Closed,
}

#[derive(Debug, Clone, Serialize)]
pub struct QueueInfo {
    pub name: String,
    pub closed: bool,
    /// Messages waiting to be claimed.
    pub pending: i64,
    /// Messages claimed but not yet acked.
    pub claimed: i64,
}

impl Store {
    fn require_live_run(&self, run_id: i64) -> CliResult<()> {
        let run = self.require_run(run_id)?;
        if run.status.is_finished() {
            return Err(CliError::invalid(format!(
                "run {run_id} has already finished ({})",
                run.status.as_str()
            )));
        }
        Ok(())
    }

    fn queue_closed(&self, run_id: i64, queue: &str) -> CliResult<Option<bool>> {
        self.conn
            .query_row(
                "SELECT closed FROM queues WHERE run_id = ? AND name = ?",
                params![run_id, queue],
                |r| r.get(0),
            )
            .optional()
            .map_err(internal)
    }

    fn ensure_queue(&mut self, run_id: i64, queue: &str) -> CliResult<bool> {
        match self.queue_closed(run_id, queue)? {
            Some(closed) => Ok(closed),
            None => {
                self.conn
                    .execute("INSERT INTO queues (run_id, name, closed, created_at) VALUES (?, ?, false, ?)", params![run_id, queue, now()])
                    .map_err(internal)?;
                Ok(false)
            }
        }
    }

    pub fn push_message(
        &mut self,
        run_id: i64,
        queue: &str,
        body: &str,
        sender: &str,
    ) -> CliResult<QueueMessage> {
        check_name("queue", queue)?;
        self.require_live_run(run_id)?;
        if body.len() > MAX_MESSAGE_BYTES {
            return Err(CliError::invalid(format!(
                "message is {} bytes; the limit is 1 MiB",
                body.len()
            ))
            .with_hint("write the data to a file and push its path instead"));
        }
        if self.ensure_queue(run_id, queue)? {
            return Err(CliError::invalid(format!("queue `{queue}` is closed")));
        }
        let id: i64 = self
            .conn
            .query_row(
                "INSERT INTO queue_messages (run_id, queue, body, sender, created_at) VALUES (?, ?, ?, ?, ?) RETURNING id",
                params![run_id, queue, body, sender, now()],
                |r| r.get(0),
            )
            .map_err(internal)?;
        Ok(self.message(run_id, id)?.expect("message just inserted"))
    }

    fn message(&self, run_id: i64, id: i64) -> CliResult<Option<QueueMessage>> {
        self.conn
            .query_row(
                "SELECT id, queue, body, sender, created_at FROM queue_messages WHERE run_id = ? AND id = ?",
                params![run_id, id],
                |r| Ok(QueueMessage { id: r.get(0)?, queue: r.get(1)?, body: r.get(2)?, sender: r.get(3)?, created_at: fmt_ts(r.get(4)?) }),
            )
            .optional()
            .map_err(internal)
    }

    /// Claim the oldest unclaimed message for `claimer`.
    pub fn pull_message(&mut self, run_id: i64, queue: &str, claimer: &str) -> CliResult<Pulled> {
        check_name("queue", queue)?;
        self.require_run(run_id)?;
        let next: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM queue_messages WHERE run_id = ? AND queue = ? AND claimed_by IS NULL ORDER BY id LIMIT 1",
                params![run_id, queue],
                |r| r.get(0),
            )
            .optional()
            .map_err(internal)?;
        match next {
            Some(id) => {
                self.conn
                    .execute(
                        "UPDATE queue_messages SET claimed_by = ?, claimed_at = ? WHERE id = ?",
                        params![claimer, now(), id],
                    )
                    .map_err(internal)?;
                Ok(Pulled::Message(
                    self.message(run_id, id)?.expect("message exists"),
                ))
            }
            None if self.queue_closed(run_id, queue)? == Some(true) => Ok(Pulled::Closed),
            None => Ok(Pulled::Empty),
        }
    }

    /// Remove a claimed message.
    pub fn ack_message(&mut self, run_id: i64, id: i64) -> CliResult<QueueMessage> {
        let msg = self
            .message(run_id, id)?
            .ok_or_else(|| CliError::not_found(format!("run {run_id} has no message {id}")))?;
        let claimed: Option<String> = self
            .conn
            .query_row(
                "SELECT claimed_by FROM queue_messages WHERE id = ?",
                params![id],
                |r| r.get(0),
            )
            .map_err(internal)?;
        if claimed.is_none() {
            return Err(
                CliError::invalid(format!("message {id} isn't claimed")).with_hint("pull it first")
            );
        }
        self.conn
            .execute("DELETE FROM queue_messages WHERE id = ?", params![id])
            .map_err(internal)?;
        Ok(msg)
    }

    /// Stop a queue taking messages; pulls report `closed` once it's empty.
    pub fn close_queue(&mut self, run_id: i64, queue: &str) -> CliResult<QueueInfo> {
        check_name("queue", queue)?;
        self.require_live_run(run_id)?;
        self.ensure_queue(run_id, queue)?;
        self.conn
            .execute(
                "UPDATE queues SET closed = true WHERE run_id = ? AND name = ?",
                params![run_id, queue],
            )
            .map_err(internal)?;
        Ok(self
            .queues(run_id)?
            .into_iter()
            .find(|q| q.name == queue)
            .expect("queue exists"))
    }

    pub fn queues(&self, run_id: i64) -> CliResult<Vec<QueueInfo>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT q.name, q.closed,
                        count(m.id) FILTER (WHERE m.claimed_by IS NULL),
                        count(m.id) FILTER (WHERE m.claimed_by IS NOT NULL)
                 FROM queues q LEFT JOIN queue_messages m ON m.run_id = q.run_id AND m.queue = q.name
                 WHERE q.run_id = ? GROUP BY q.name, q.closed, q.created_at ORDER BY q.created_at, q.name",
            )
            .map_err(internal)?;
        let rows = stmt
            .query_map(params![run_id], |r| {
                Ok(QueueInfo {
                    name: r.get(0)?,
                    closed: r.get(1)?,
                    pending: r.get(2)?,
                    claimed: r.get(3)?,
                })
            })
            .map_err(internal)?;
        rows.collect::<Result<_, _>>().map_err(internal)
    }

    /// Put a claimer's unacked messages back on their queues.
    pub fn release_claims(&mut self, run_id: i64, claimer: &str) -> CliResult<usize> {
        self.conn
            .execute(
                "UPDATE queue_messages SET claimed_by = NULL, claimed_at = NULL WHERE run_id = ? AND claimed_by = ?",
                params![run_id, claimer],
            )
            .map_err(internal)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{new_run, store};
    use super::*;

    fn pulled_id(p: Pulled) -> Option<i64> {
        match p {
            Pulled::Message(m) => Some(m.id),
            _ => None,
        }
    }

    #[test]
    fn claim_ack_release_and_close() {
        let (_d, mut store) = store();
        let run = new_run(&mut store, "wf").id;
        assert!(matches!(
            store.pull_message(run, "q", "a").unwrap(),
            Pulled::Empty
        ));
        let m1 = store.push_message(run, "q", "one", "orchestrator").unwrap();
        let m2 = store.push_message(run, "q", "two", "orchestrator").unwrap();
        assert_eq!(
            pulled_id(store.pull_message(run, "q", "a").unwrap()),
            Some(m1.id)
        );
        assert_eq!(
            pulled_id(store.pull_message(run, "q", "b").unwrap()),
            Some(m2.id)
        );
        assert!(matches!(
            store.pull_message(run, "q", "c").unwrap(),
            Pulled::Empty
        ));

        store.ack_message(run, m2.id).unwrap();
        assert_eq!(
            store.ack_message(run, m2.id).unwrap_err().kind,
            crate::output::ErrorKind::NotFound
        );
        // a goes away without acking: m1 is back.
        assert_eq!(store.release_claims(run, "a").unwrap(), 1);
        let q = &store.queues(run).unwrap()[0];
        assert_eq!((q.pending, q.claimed, q.closed), (1, 0, false));

        store.close_queue(run, "q").unwrap();
        assert!(store
            .push_message(run, "q", "late", "orchestrator")
            .unwrap_err()
            .message
            .contains("closed"));
        assert_eq!(
            pulled_id(store.pull_message(run, "q", "c").unwrap()),
            Some(m1.id),
            "closed queues still drain"
        );
        assert!(matches!(
            store.pull_message(run, "q", "c").unwrap(),
            Pulled::Closed
        ));
    }

    #[test]
    fn big_messages_are_refused() {
        let (_d, mut store) = store();
        let run = new_run(&mut store, "wf").id;
        let err = store
            .push_message(run, "q", &"x".repeat(MAX_MESSAGE_BYTES + 1), "w1")
            .unwrap_err();
        assert!(
            err.hint.as_deref().unwrap_or("").contains("file"),
            "{err:?}"
        );
    }
}
