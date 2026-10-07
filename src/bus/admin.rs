//! Inspecting and repairing the bus (`tome events ...`), and firing a topic
//! trigger by hand (`tome triggers fire`).

use super::{matching, pattern_of, subscriptions, Publish, TEST};
use crate::api::{opt_str, req_num_at, req_str};
use crate::arming::Armed;
use crate::engine::Engine;
use crate::ids::{DeliveryId, EventId, RunId};
use crate::output::{CliError, CliResult};
use crate::store::{BusEvent, Delivery, DeliveryState};
use crate::triggers::{outcome, Event, FireRequest, Fired};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;

/// A payload's first line, cut to fit a table.
pub fn preview(payload: &str) -> String {
    let first = payload.lines().next().unwrap_or("").trim();
    if first.chars().count() > 60 {
        format!("{}…", first.chars().take(59).collect::<String>())
    } else {
        first.to_string()
    }
}

fn settled(d: &Delivery) -> bool {
    d.state == DeliveryState::Done || d.state == DeliveryState::Dropped
}

impl Engine {
    /// `events.ls {project_path}`: each topic with its event count, last
    /// publish and each subscriber's pending, claimed and failed counts.
    pub(crate) fn events_ls(&self, p: &Value) -> CliResult<Value> {
        let project = req_str(p, "project_path")?;
        let subs = subscriptions(Path::new(project));
        let (topics, deliveries, events) = self.with_store(|store| {
            let topics = store.bus_topics(project)?;
            let mut events = Vec::new();
            for (topic, _, _) in &topics {
                events.extend(store.bus_events(project, topic)?);
            }
            Ok((topics, store.project_deliveries(project)?, events))
        })?;
        let topic_of: BTreeMap<EventId, &str> =
            events.iter().map(|e| (e.id, e.topic.as_str())).collect();
        let topics: Vec<Value> = topics
            .iter()
            .map(|(topic, count, last)| {
                // Current subscribers first, so they show with nothing waiting too.
                let mut counts: BTreeMap<String, [i64; 3]> =
                    matching(&subs, topic).into_iter().map(|a| (a.name, [0; 3])).collect();
                for d in deliveries.iter().filter(|d| topic_of.get(&d.event_id) == Some(&topic.as_str())) {
                    let c = counts.entry(d.workflow.clone()).or_default();
                    match d.state {
                        DeliveryState::Pending => c[0] += 1,
                        DeliveryState::Claimed => c[1] += 1,
                        DeliveryState::Failed => c[2] += 1,
                        DeliveryState::Done | DeliveryState::Dropped => {}
                    }
                }
                let subscribers: Vec<Value> = counts
                    .into_iter()
                    .map(|(wf, [pending, claimed, failed])| json!({ "workflow": wf, "pending": pending, "claimed": claimed, "failed": failed }))
                    .collect();
                json!({ "topic": topic, "events": count, "last_published": last, "subscribers": subscribers })
            })
            .collect();
        Ok(json!({ "project": project, "topics": topics }))
    }

    /// `events.show {project_path, topic, all?}`: the topic's events, oldest
    /// first, with each delivery; settled ones only with `all`.
    pub(crate) fn events_show(&self, p: &Value) -> CliResult<Value> {
        let project = req_str(p, "project_path")?;
        let topic = req_str(p, "topic")?;
        let all = p["all"] == true;
        let events = self.with_store(|store| {
            let mut out = Vec::new();
            for e in store.bus_events(project, topic)? {
                let deliveries = store.deliveries_of(e.id)?;
                out.push((e, deliveries));
            }
            Ok(out)
        })?;
        let total = events.len();
        let shown: Vec<Value> = events
            .into_iter()
            .filter(|(_, ds)| all || !ds.iter().all(settled))
            .map(|(e, ds)| {
                let deliveries: Vec<Value> =
                    ds.iter().map(|d| json!({ "id": d.id, "workflow": d.workflow, "state": d.state, "run_ids": d.run_ids })).collect();
                json!({
                    "id": e.id,
                    "sender": e.sender,
                    "published_at": e.published_at,
                    "depth": e.depth,
                    "refused": e.refused,
                    "preview": preview(&e.payload),
                    "bytes": e.payload.len(),
                    "deliveries": deliveries,
                })
            })
            .collect();
        let hidden = total - shown.len();
        Ok(
            json!({ "project": project, "topic": topic, "all": all, "events": shown, "hidden": hidden }),
        )
    }

    /// `events.retry` / `events.remove {event, workflow?, project_path?}`:
    /// move an event's delivery from one of `from` to `to`. Refused if it's
    /// in another state, or several would move and no workflow was named.
    /// Removing an event with no deliveries (no subscribers, or refused)
    /// deletes the event itself, since there's nothing to drop.
    pub(crate) fn events_move(
        &self,
        p: &Value,
        from: &[DeliveryState],
        to: DeliveryState,
    ) -> CliResult<Value> {
        let id = EventId::new(req_num_at(p, "event")?);
        let workflow = opt_str(p, "workflow");
        let project = opt_str(p, "project_path");
        let verb = if to == DeliveryState::Pending {
            "retried"
        } else {
            "removed"
        };
        let moved = self.with_store(|store| {
            let event = store
                .bus_event(id)?
                .filter(|e| project.is_none_or(|p| p == e.project_path))
                .ok_or_else(|| CliError::not_found(format!("no event {id} in this project")))?;
            let mut deliveries = store.deliveries_of(id)?;
            if deliveries.is_empty() && workflow.is_none() && to == DeliveryState::Dropped {
                store.delete_bus_event(id)?;
                return Ok((event, None));
            }
            if let Some(wf) = workflow {
                deliveries.retain(|d| super::forward::names(d, wf));
                if deliveries.is_empty() {
                    return Err(CliError::not_found(format!(
                        "event {id} wasn't delivered to `{wf}`"
                    )));
                }
            }
            let states = || {
                deliveries
                    .iter()
                    .map(|d| format!("{} ({})", d.workflow, d.state.as_str()))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let movable: Vec<&Delivery> = deliveries
                .iter()
                .filter(|d| from.contains(&d.state))
                .collect();
            if movable.is_empty() {
                let what = if deliveries.is_empty() {
                    "no deliveries".to_string()
                } else {
                    states()
                };
                return Err(
                    CliError::invalid(format!("event {id} can't be {verb}: {what}")).with_hint(
                        format!(
                            "only {} deliveries can be {verb}",
                            from.iter()
                                .map(|s| s.as_str())
                                .collect::<Vec<_>>()
                                .join(" or ")
                        ),
                    ),
                );
            }
            if movable.len() > 1 && workflow.is_none() {
                return Err(CliError::invalid(format!(
                    "event {id} went to several workflows: {}",
                    states()
                ))
                .with_hint("name one with --workflow <name>"));
            }
            let d = movable[0].clone();
            // A retried delivery starts over: no runs yet.
            let runs: Option<&[RunId]> = (to == DeliveryState::Pending).then_some(&[]);
            if !store.move_delivery(d.id, d.state, to, runs)? {
                return Err(CliError::conflict(format!(
                    "event {id}'s delivery to `{}` changed meanwhile; try again",
                    d.workflow
                )));
            }
            Ok((event, Some(store.delivery(d.id)?.unwrap_or(d))))
        })?;
        let (event, delivery) = match moved {
            (event, None) => {
                eprintln!(
                    "tome daemon: event {} removed by hand (no deliveries)",
                    event.id
                );
                return Ok(json!({ "event": event, "delivery": null, "deleted": true }));
            }
            (event, Some(delivery)) => (event, delivery),
        };
        eprintln!(
            "tome daemon: delivery {} of event {} to {} {verb} by hand",
            delivery.id, event.id, delivery.workflow
        );
        Ok(json!({ "event": event, "delivery": delivery }))
    }

    /// A topic trigger's backlog for `tome triggers ls`: how many events wait,
    /// and the last one a run took.
    pub(crate) fn topic_status(&self, a: &Armed) -> Option<Value> {
        let (project, pattern) = (a.project.as_ref()?, pattern_of(a)?.to_string());
        let ds = self
            .with_store(|store| store.project_deliveries(&project.display().to_string()))
            .ok()?;
        let ours: Vec<&Delivery> = ds
            .iter()
            .filter(|d| d.workflow == a.name && d.pattern == pattern)
            .collect();
        let pending = ours
            .iter()
            .filter(|d| d.state == DeliveryState::Pending)
            .count();
        let last = ours
            .iter()
            .rev()
            .find(|d| !d.run_ids.is_empty())
            .map(|d| json!({ "event_id": d.event_id, "run_ids": d.run_ids }));
        Some(json!({ "pending": pending, "last_event": last }))
    }

    /// `tome triggers fire` on a topic trigger: take the subscription's next
    /// pending event, or with `payload` publish a test event to this
    /// workflow alone and take that.
    pub(crate) fn fire_topic(
        &self,
        a: &Armed,
        payload: Option<&str>,
        dry_run: bool,
    ) -> CliResult<Value> {
        let project = a
            .project
            .clone()
            .ok_or_else(|| CliError::invalid("topic triggers belong to project workflows"))?;
        let pattern = pattern_of(a).expect("a topic trigger").clone();
        // Hold off the trigger loop, so it doesn't take the event first.
        let _draining = self.draining.lock().unwrap_or_else(|p| p.into_inner());
        let respond = |fired: Fired, event: Option<EventId>| {
            let mut out = json!({
                "outcome": fired.outcome,
                "message": fired.message,
                "run_ids": fired.runs,
                "dry_run": dry_run,
                "index": a.index,
                "event_id": event,
            });
            if let (Value::Object(extra), Value::Object(o)) = (fired.data, &mut out) {
                o.extend(extra);
            }
            out
        };
        let dry = |event: BusEvent| {
            self.fire(&FireRequest {
                workflow_path: a.workflow_path.clone(),
                project: Some(project.clone()),
                index: a.index,
                event: Event {
                    bus: Some((event, DeliveryId::new(0))),
                    synthetic: true,
                    ..Default::default()
                },
                dry_run: true,
            })
        };
        let delivery = match payload {
            Some(payload) => {
                crate::store::check_payload(payload)?;
                let topic = pattern.example();
                if dry_run {
                    let event = BusEvent {
                        id: EventId::new(0),
                        project_path: project.display().to_string(),
                        topic,
                        payload: payload.to_string(),
                        sender: TEST.into(),
                        sender_run_id: None,
                        depth: 0,
                        refused: None,
                        published_at: String::new(),
                    };
                    return Ok(respond(dry(event), None));
                }
                let published = self.publish(
                    &Publish {
                        project: &project,
                        topic: &topic,
                        payload,
                        sender: TEST.into(),
                        sender_run: None,
                        depth: 0,
                        only: Some(&a.name),
                    },
                    false,
                )?;
                published.deliveries.into_iter().next().ok_or_else(|| {
                    CliError::internal(format!(
                        "the test event on {topic} wasn't delivered to {}",
                        a.name
                    ))
                })?
            }
            None => match self.pending_of(a).into_iter().next() {
                Some(d) => d,
                None => {
                    let fired = Fired {
                        outcome: outcome::NO_TARGET,
                        message: Some(format!("no pending event for on {pattern}")),
                        runs: Vec::new(),
                        data: json!({ "hint": "publish one with `tome publish`, or send a test event with --payload <text>" }),
                    };
                    return Ok(respond(fired, None));
                }
            },
        };
        if dry_run {
            let event = self
                .with_store(|store| store.bus_event(delivery.event_id))?
                .ok_or_else(|| CliError::not_found("the event is gone"))?;
            let mut fired = dry(event);
            fired.message = fired
                .message
                .map(|m| format!("event {}: {m}", delivery.event_id));
            return Ok(respond(fired, Some(delivery.event_id)));
        }
        match self.take(a, &delivery, true) {
            Some((fired, _)) => Ok(respond(fired, Some(delivery.event_id))),
            None => Err(CliError::conflict(format!(
                "event {} was taken meanwhile",
                delivery.event_id
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previews_are_the_first_line_cut_short() {
        assert_eq!(preview("  hello\nworld"), "hello");
        assert_eq!(preview(""), "");
        let long = "x".repeat(100);
        assert_eq!(preview(&long).chars().count(), 60);
    }
}
