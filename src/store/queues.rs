//! A project's message queues. A message is claimed by whoever pulls it and
//! removed when they ack it; if the claimer (a worker) ends first, the
//! message goes back on the queue. Queues belong to the project, not a run:
//! they outlive runs and anything in the project can use them.

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
    /// Who pushed it: `user`, `run 12`, `run 12 worker w1`, `trigger`.
    pub sender: String,
    pub created_at: String,
    /// Who holds it, if it's been pulled and not acked.
    pub claimed_by: Option<String>,
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

const MESSAGE_COLUMNS: &str = "id, queue, body, sender, created_at, claimed_by";

fn message_row(r: &duckdb::Row) -> duckdb::Result<QueueMessage> {
    Ok(QueueMessage {
        id: r.get(0)?,
        queue: r.get(1)?,
        body: r.get(2)?,
        sender: r.get(3)?,
        created_at: fmt_ts(r.get(4)?),
        claimed_by: r.get(5)?,
    })
}

impl Store {
    fn queue_closed(&self, project: &str, queue: &str) -> CliResult<Option<bool>> {
        self.conn
            .query_row(
                "SELECT closed FROM queues WHERE project_path = ? AND name = ?",
                params![project, queue],
                |r| r.get(0),
            )
            .optional()
            .map_err(internal)
    }

    fn ensure_queue(&mut self, project: &str, queue: &str) -> CliResult<bool> {
        match self.queue_closed(project, queue)? {
            Some(closed) => Ok(closed),
            None => {
                self.conn
                    .execute("INSERT INTO queues (project_path, name, closed, created_at) VALUES (?, ?, false, ?)", params![project, queue, now()])
                    .map_err(internal)?;
                Ok(false)
            }
        }
    }

    pub fn push_message(
        &mut self,
        project: &str,
        queue: &str,
        body: &str,
        sender: &str,
    ) -> CliResult<QueueMessage> {
        check_name("queue", queue)?;
        if body.len() > MAX_MESSAGE_BYTES {
            return Err(CliError::invalid(format!(
                "message is {} bytes; the limit is 1 MiB",
                body.len()
            ))
            .with_hint("write the data to a file and push its path instead"));
        }
        if self.ensure_queue(project, queue)? {
            return Err(CliError::invalid(format!("queue `{queue}` is closed")));
        }
        let id: i64 = self
            .conn
            .query_row(
                "INSERT INTO queue_messages (project_path, queue, body, sender, created_at) VALUES (?, ?, ?, ?, ?) RETURNING id",
                params![project, queue, body, sender, now()],
                |r| r.get(0),
            )
            .map_err(internal)?;
        Ok(self.message(project, id)?.expect("message just inserted"))
    }

    fn message(&self, project: &str, id: i64) -> CliResult<Option<QueueMessage>> {
        self.conn
            .query_row(
                &format!("SELECT {MESSAGE_COLUMNS} FROM queue_messages WHERE project_path = ? AND id = ?"),
                params![project, id],
                message_row,
            )
            .optional()
            .map_err(internal)
    }

    /// The oldest `limit` messages on a queue, claimed ones included,
    /// without claiming anything.
    pub fn peek_messages(
        &self,
        project: &str,
        queue: &str,
        limit: usize,
    ) -> CliResult<Vec<QueueMessage>> {
        check_name("queue", queue)?;
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {MESSAGE_COLUMNS} FROM queue_messages WHERE project_path = ? AND queue = ? ORDER BY id LIMIT ?"
            ))
            .map_err(internal)?;
        let rows = stmt
            .query_map(params![project, queue, limit as i64], message_row)
            .map_err(internal)?;
        rows.collect::<Result<_, _>>().map_err(internal)
    }

    /// Claim the oldest unclaimed message for `claimer`.
    pub fn pull_message(&mut self, project: &str, queue: &str, claimer: &str) -> CliResult<Pulled> {
        check_name("queue", queue)?;
        let next: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM queue_messages WHERE project_path = ? AND queue = ? AND claimed_by IS NULL ORDER BY id LIMIT 1",
                params![project, queue],
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
                    self.message(project, id)?.expect("message exists"),
                ))
            }
            None if self.queue_closed(project, queue)? == Some(true) => Ok(Pulled::Closed),
            None => Ok(Pulled::Empty),
        }
    }

    /// Remove a claimed message.
    pub fn ack_message(&mut self, project: &str, id: i64) -> CliResult<QueueMessage> {
        let msg = self
            .message(project, id)?
            .ok_or_else(|| CliError::not_found(format!("this project has no message {id}")))?;
        if msg.claimed_by.is_none() {
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
    pub fn close_queue(&mut self, project: &str, queue: &str) -> CliResult<QueueInfo> {
        check_name("queue", queue)?;
        self.ensure_queue(project, queue)?;
        self.conn
            .execute(
                "UPDATE queues SET closed = true WHERE project_path = ? AND name = ?",
                params![project, queue],
            )
            .map_err(internal)?;
        Ok(self
            .queues(project)?
            .into_iter()
            .find(|q| q.name == queue)
            .expect("queue exists"))
    }

    pub fn queues(&self, project: &str) -> CliResult<Vec<QueueInfo>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT q.name, q.closed,
                        count(m.id) FILTER (WHERE m.claimed_by IS NULL),
                        count(m.id) FILTER (WHERE m.claimed_by IS NOT NULL)
                 FROM queues q LEFT JOIN queue_messages m ON m.project_path = q.project_path AND m.queue = q.name
                 WHERE q.project_path = ? GROUP BY q.name, q.closed, q.created_at ORDER BY q.created_at, q.name",
            )
            .map_err(internal)?;
        let rows = stmt
            .query_map(params![project], |r| {
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
    pub fn release_claims(&mut self, claimer: &str) -> CliResult<usize> {
        self.conn
            .execute(
                "UPDATE queue_messages SET claimed_by = NULL, claimed_at = NULL WHERE claimed_by = ?",
                params![claimer],
            )
            .map_err(internal)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::store;
    use super::*;

    const P: &str = "/proj";

    fn pulled_id(p: Pulled) -> Option<i64> {
        match p {
            Pulled::Message(m) => Some(m.id),
            _ => None,
        }
    }

    #[test]
    fn claim_ack_release_and_close() {
        let (_d, mut store) = store();
        assert!(matches!(
            store.pull_message(P, "q", "a").unwrap(),
            Pulled::Empty
        ));
        let m1 = store.push_message(P, "q", "one", "user").unwrap();
        let m2 = store.push_message(P, "q", "two", "user").unwrap();
        assert_eq!(
            pulled_id(store.pull_message(P, "q", "a").unwrap()),
            Some(m1.id)
        );
        assert_eq!(
            pulled_id(store.pull_message(P, "q", "b").unwrap()),
            Some(m2.id)
        );
        assert!(matches!(
            store.pull_message(P, "q", "c").unwrap(),
            Pulled::Empty
        ));

        store.ack_message(P, m2.id).unwrap();
        assert_eq!(
            store.ack_message(P, m2.id).unwrap_err().kind,
            crate::output::ErrorKind::NotFound
        );
        // a goes away without acking: m1 is back.
        assert_eq!(store.release_claims("a").unwrap(), 1);
        let q = &store.queues(P).unwrap()[0];
        assert_eq!((q.pending, q.claimed, q.closed), (1, 0, false));

        store.close_queue(P, "q").unwrap();
        assert!(store
            .push_message(P, "q", "late", "user")
            .unwrap_err()
            .message
            .contains("closed"));
        assert_eq!(
            pulled_id(store.pull_message(P, "q", "c").unwrap()),
            Some(m1.id),
            "closed queues still drain"
        );
        assert!(matches!(
            store.pull_message(P, "q", "c").unwrap(),
            Pulled::Closed
        ));
    }

    #[test]
    fn peek_claims_nothing_and_projects_are_separate() {
        let (_d, mut store) = store();
        let m1 = store.push_message(P, "q", "one", "user").unwrap();
        store.push_message(P, "q", "two", "user").unwrap();
        store
            .push_message("/other", "q", "elsewhere", "user")
            .unwrap();
        store.pull_message(P, "q", "a").unwrap();

        let seen = store.peek_messages(P, "q", 10).unwrap();
        assert_eq!(
            seen.iter().map(|m| m.body.as_str()).collect::<Vec<_>>(),
            ["one", "two"]
        );
        assert_eq!(seen[0].claimed_by.as_deref(), Some("a"));
        assert_eq!(seen[1].claimed_by, None);
        assert_eq!(store.peek_messages(P, "q", 1).unwrap().len(), 1);
        let q = &store.queues(P).unwrap()[0];
        assert_eq!((q.pending, q.claimed), (1, 1), "peeking claims nothing");
        assert_eq!(
            store.ack_message("/other", m1.id).unwrap_err().kind,
            crate::output::ErrorKind::NotFound
        );
    }

    #[test]
    fn big_messages_are_refused() {
        let (_d, mut store) = store();
        let err = store
            .push_message(P, "q", &"x".repeat(MAX_MESSAGE_BYTES + 1), "w1")
            .unwrap_err();
        assert!(
            err.hint.as_deref().unwrap_or("").contains("file"),
            "{err:?}"
        );
    }
}
