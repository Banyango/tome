//! Tome's own events: `tome.run.<workflow>.started` once a run's
//! orchestrator has made its first tome call, and `.succeeded`, `.failed` or
//! `.cancelled` once it has ended.

use super::{depth_from, Publish, ENDED, STARTED, TOME};
use crate::engine::Engine;
use crate::ids::RunId;
use crate::store::{Run, RunStatus};
use crate::topic;
use chrono::NaiveDateTime;
use serde_json::{json, Value};
use std::path::Path;

/// Whole seconds from a run's creation to its end.
fn duration(run: &Run) -> Option<i64> {
    let parse = |t: &str| NaiveDateTime::parse_from_str(t, "%Y-%m-%dT%H:%M:%S%.fZ").ok();
    let (start, end) = (parse(&run.created_at)?, parse(run.finished_at.as_deref()?)?);
    Some((end - start).num_seconds())
}

/// The event for a run that has ended, and its payload.
fn ended(run: &Run) -> Option<(&'static str, Value)> {
    let base = json!({ "run_id": run.id, "workflow": run.workflow_name });
    let (what, extra) = match run.status {
        RunStatus::Succeeded => (
            "succeeded",
            json!({ "summary": run.summary, "duration": duration(run) }),
        ),
        RunStatus::Failed => (
            "failed",
            json!({ "reason": run.reason, "summary": run.summary }),
        ),
        RunStatus::Cancelled => ("cancelled", json!({})),
        _ => return None,
    };
    let mut payload = base;
    if let (Value::Object(p), Value::Object(e)) = (&mut payload, extra) {
        p.extend(e);
    }
    Some((what, payload))
}

impl Engine {
    /// A run's orchestrator (or single agent) has started: publish
    /// `.started`, once.
    pub(crate) fn announce_started(&self, run_id: RunId) {
        let first = self.with_store(|store| {
            if store.announced(run_id)?.is_some() {
                return Ok(None);
            }
            store.set_announced(run_id, STARTED)?;
            Ok(store.get_run(run_id, false).map_err(crate::api::internal)?)
        });
        match first {
            Ok(Some(run)) => {
                let payload = json!({ "run_id": run.id, "workflow": run.workflow_name, "params": run.params, "cause": run.trigger });
                self.publish_lifecycle(&run, STARTED, &payload);
            }
            Ok(None) => {}
            Err(e) => eprintln!(
                "tome daemon: announcing run {run_id}'s start failed: {}",
                e.message
            ),
        }
    }

    /// A run has ended: publish how, once. Its end is marked handled first,
    /// so a failed publish isn't retried every tick.
    pub(crate) fn announce_ended(&self, run_id: RunId) {
        let run = self.with_store(|store| {
            store.set_announced(run_id, ENDED)?;
            store.get_run(run_id, false).map_err(crate::api::internal)
        });
        match run {
            Ok(Some(run)) => {
                if let Some((what, payload)) = ended(&run) {
                    self.publish_lifecycle(&run, what, &payload);
                }
            }
            Ok(None) => {}
            Err(e) => eprintln!(
                "tome daemon: marking the end of run {run_id} failed: {}",
                e.message
            ),
        }
    }

    /// Publish `tome.run.<workflow>.<what>` to the run's project, a step
    /// deeper than the event that started the run.
    pub(crate) fn publish_lifecycle(&self, run: &Run, what: &str, payload: &Value) {
        let Some(project) = &run.project_path else {
            return;
        };
        let topic = format!("{}.run.{}.{what}", topic::RESERVED, run.workflow_name);
        if let Err(e) = topic::check_name(&topic) {
            eprintln!("tome daemon: run {} has no lifecycle events: {e}", run.id);
            return;
        }
        let publish = Publish {
            project: Path::new(project),
            topic: &topic,
            payload: &payload.to_string(),
            sender: TOME.into(),
            sender_run: Some(run.id),
            depth: depth_from(run),
            only: None,
        };
        if let Err(e) = self.publish(&publish, false) {
            eprintln!(
                "tome daemon: publishing {topic} for run {} failed: {}",
                run.id, e.message
            );
        }
    }

    pub(crate) fn announce_blocked(&self, run_id: RunId, session: &str, pane: &str, at: &str) {
        if let Ok(Some(run)) =
            self.with_store(|store| store.get_run(run_id, false).map_err(crate::api::internal))
        {
            self.publish_lifecycle(&run, "blocked", &json!({"run_id":run.id,"workflow":run.workflow_name,"session":session,"pane":pane,"at":at}));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(status: RunStatus) -> Run {
        Run {
            id: RunId::new(3),
            workflow_name: "implement".into(),
            workflow_path: None,
            project_path: None,
            params: json!({}),
            status,
            reason: Some("orchestrator_exited".into()),
            summary: Some("done".into()),
            created_at: "2026-01-02T03:04:05.000Z".into(),
            finished_at: Some("2026-01-02T03:05:10.500Z".into()),
            trigger: None,
            workflow_snapshot: None,
            placement: None,
            mode: crate::workflow::Mode::Orchestrated,
            resumed_from: None,
            custom_status: None,
        }
    }

    #[test]
    fn ended_payloads_fit_how_the_run_ended() {
        let (what, p) = ended(&run(RunStatus::Succeeded)).unwrap();
        assert_eq!(
            (what, p),
            (
                "succeeded",
                json!({ "run_id": 3, "workflow": "implement", "summary": "done", "duration": 65 })
            )
        );
        let (what, p) = ended(&run(RunStatus::Failed)).unwrap();
        assert_eq!(
            (what, p),
            (
                "failed",
                json!({ "run_id": 3, "workflow": "implement", "reason": "orchestrator_exited", "summary": "done" })
            )
        );
        assert_eq!(
            ended(&run(RunStatus::Cancelled)).unwrap().1,
            json!({ "run_id": 3, "workflow": "implement" })
        );
        assert!(ended(&run(RunStatus::Running)).is_none());
    }
}
