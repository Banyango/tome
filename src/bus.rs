//! The project message bus, daemon side: publishing events, recording a
//! delivery for each subscribing workflow, and the RPCs behind `tome
//! publish` and `tome events`.
//!
//! A subscription is an `on:` trigger of a project workflow, known by the
//! workflow's name and the trigger's pattern. A workflow with several
//! matching triggers gets one delivery, for the first.

use crate::api::{opt_str, req_id_at, req_str};
use crate::arming::{self, Armed};
use crate::engine::Engine;
use crate::output::{CliError, CliResult};
use crate::store::{BusEvent, Delivery, NewEvent, Run, Subscriber};
use crate::topic;
use crate::workflow::TriggerKind;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub const METHODS: &[&str] = &["events.publish"];

/// Who publishes tome's own events.
pub const TOME: &str = "tome";

/// The topic triggers of a project's workflows, as armed (whether the
/// project's triggers are enabled or not).
pub fn subscriptions(project: &Path) -> Vec<Armed> {
    arming::scan(&[project.to_path_buf()])
        .armed
        .into_iter()
        .filter(|a| a.project.is_some() && matches!(a.trigger.kind, TriggerKind::Topic { .. }))
        .collect()
}

/// The pattern of a topic trigger.
pub fn pattern_of(a: &Armed) -> Option<&topic::Pattern> {
    match &a.trigger.kind {
        TriggerKind::Topic { on } => Some(on),
        _ => None,
    }
}

/// The subscriptions `topic` is delivered to: each workflow's first
/// matching trigger.
pub fn matching(subs: &[Armed], topic: &str) -> Vec<Armed> {
    let mut out: Vec<Armed> = Vec::new();
    for a in subs {
        if pattern_of(a).is_some_and(|p| p.matches(topic)) && !out.iter().any(|m| m.name == a.name) {
            out.push(a.clone());
        }
    }
    out
}

/// An event to publish.
pub struct Publish<'a> {
    pub project: &'a Path,
    pub topic: &'a str,
    pub payload: &'a str,
    pub sender: String,
    pub sender_run: Option<i64>,
    pub depth: i64,
    /// Deliver only to this workflow (a test event).
    pub only: Option<&'a str>,
}

pub struct Published {
    /// `None` on a dry run.
    pub event: Option<BusEvent>,
    pub deliveries: Vec<Delivery>,
    pub matches: Vec<Armed>,
}

/// `run 12`, or `run 12 worker w1`.
pub fn run_sender(run_id: i64, worker: Option<&str>) -> String {
    match worker {
        Some(w) => format!("run {run_id} worker {w}"),
        None => format!("run {run_id}"),
    }
}

/// The depth of events a run publishes: one more than the event that
/// started it, else 0.
pub fn depth_from(run: &Run) -> i64 {
    run.trigger
        .as_ref()
        .filter(|c| c["event_id"].is_i64())
        .and_then(|c| c["depth"].as_i64())
        .map_or(0, |d| d + 1)
}

impl Engine {
    pub(crate) fn dispatch_bus(&self, method: &str, p: &Value) -> CliResult<Value> {
        match method {
            "events.publish" => self.publish_rpc(p),
            _ => Err(CliError::invalid(format!("unknown method `{method}`"))),
        }
    }

    /// `events.publish {topic, payload, project_path?, run_id?, worker?,
    /// dry_run?}`: from inside a run (`run_id`) it goes to the run's
    /// project.
    fn publish_rpc(&self, p: &Value) -> CliResult<Value> {
        let topic = req_str(p, "topic")?;
        let payload = opt_str(p, "payload").unwrap_or_default();
        topic::check_name(topic).map_err(CliError::invalid)?;
        if topic::is_reserved(topic) {
            return Err(CliError::invalid(format!("topic `{topic}` is reserved: `{}.*` events are tome's own", topic::RESERVED))
                .with_hint("pick another first segment"));
        }
        crate::store::check_payload(payload)?;
        let run = match p.get("run_id").filter(|v| !v.is_null()) {
            Some(_) => Some(self.with_store(|store| store.require_run(req_id_at(p, "run_id")?))?),
            None => None,
        };
        let project = match (&run, opt_str(p, "project_path")) {
            (Some(run), _) => run.project_path.clone().ok_or_else(|| {
                CliError::invalid(format!("run {} has no project to publish to", run.id))
                    .with_hint("the bus belongs to a project: run the workflow inside one")
            })?,
            (None, Some(path)) => path.to_string(),
            (None, None) => {
                return Err(CliError::invalid("not inside a project").with_hint("run `tome publish` in a project (a directory with .tome/)"))
            }
        };
        let worker = opt_str(p, "worker").filter(|w| !w.is_empty());
        let publish = Publish {
            project: Path::new(&project),
            topic,
            payload,
            sender: run.as_ref().map_or_else(|| "user".to_string(), |r| run_sender(r.id, worker)),
            sender_run: run.as_ref().map(|r| r.id),
            depth: run.as_ref().map_or(0, depth_from),
            only: None,
        };
        let dry_run = p["dry_run"] == true;
        if !dry_run {
            self.with_store(|store| store.register_project(&project).map(|_| ()))?;
        }
        let published = self.publish(&publish, dry_run)?;
        let names: Vec<&str> = published.matches.iter().map(|a| a.name.as_str()).collect();
        let matches: Vec<Value> = published
            .matches
            .iter()
            .map(|a| json!({ "workflow": a.name, "workflow_path": a.workflow_path, "index": a.index, "on": a.trigger.describe() }))
            .collect();
        Ok(json!({
            "event": published.event,
            "project": project,
            "topic": topic,
            "delivered_to": if dry_run { Vec::new() } else { names },
            "deliveries": published.deliveries,
            "matches": matches,
            "dry_run": dry_run,
        }))
    }

    /// Publish an event: record it with a pending delivery per matching
    /// subscription. A publish from a run shows in the run's stream.
    pub fn publish(&self, e: &Publish<'_>, dry_run: bool) -> CliResult<Published> {
        let mut matches = matching(&subscriptions(e.project), e.topic);
        if let Some(only) = e.only {
            matches.retain(|a| a.name == only);
        }
        if dry_run {
            return Ok(Published { event: None, deliveries: Vec::new(), matches });
        }
        let project = e.project.display().to_string();
        let subscribers: Vec<Subscriber> = matches
            .iter()
            .map(|a| Subscriber {
                workflow_name: a.name.clone(),
                workflow_path: a.workflow_path.display().to_string(),
                pattern: pattern_of(a).map(ToString::to_string).unwrap_or_default(),
            })
            .collect();
        let (event, deliveries) = self.with_store(|store| {
            let published = store.publish_event(
                &NewEvent {
                    project_path: &project,
                    topic: e.topic,
                    payload: e.payload,
                    sender: &e.sender,
                    sender_run_id: e.sender_run,
                    depth: e.depth,
                    refused: None,
                },
                &subscribers,
            )?;
            if let Some(run_id) = e.sender_run.filter(|_| e.sender != TOME) {
                let names: Vec<&str> = published.1.iter().map(|d| d.workflow.as_str()).collect();
                let to = if names.is_empty() { "no subscribers".to_string() } else { names.join(", ") };
                let what = format!("{} (event {}) by {} to {to}", e.topic, published.0.id, e.sender);
                store.worker_event(run_id, None, None, PUBLISHED, Some(&what))?;
                self.sync(store, run_id);
            }
            Ok(published)
        })?;
        eprintln!(
            "tome daemon: event {} on {} from {} delivered to {}",
            event.id,
            e.topic,
            e.sender,
            deliveries.len()
        );
        Ok(Published { event: Some(event), deliveries, matches })
    }

    /// Re-armed: drop the pending deliveries of subscriptions that are gone
    /// (a workflow deleted, or its `on:` trigger removed). An invalid
    /// workflow's are kept until it's fixed.
    pub(crate) fn drop_unsubscribed(&self, scan: &arming::Scan, projects: &[PathBuf]) {
        for root in projects {
            let live: Vec<(String, String)> = scan
                .armed
                .iter()
                .filter(|a| a.project.as_deref() == Some(root.as_path()))
                .filter_map(|a| pattern_of(a).map(|p| (a.name.clone(), p.to_string())))
                .collect();
            let keep: Vec<String> =
                scan.broken.iter().filter(|b| b.project.as_deref() == Some(root.as_path())).map(|b| b.name.clone()).collect();
            match self.with_store(|store| store.drop_unsubscribed(&root.display().to_string(), &live, &keep)) {
                Ok(dropped) => {
                    for d in dropped {
                        eprintln!("tome daemon: dropped delivery {} of event {} to {} ({}): no longer subscribed", d.id, d.event_id, d.workflow, d.pattern);
                    }
                }
                Err(e) => eprintln!("tome daemon: dropping stale deliveries failed: {}", e.message),
            }
        }
    }
}

/// The run history event recorded when a run publishes.
pub const PUBLISHED: &str = "published";

#[cfg(test)]
mod tests {
    use super::*;

    fn armed(name: &str, on: &str, index: usize) -> Armed {
        let text = format!("---\nname: {name}\ntriggers:\n  - on: {on}\n---\n");
        let wf = crate::workflow::parse(Path::new("w.md"), &text).unwrap();
        Armed { workflow_path: "w.md".into(), name: name.into(), project: Some("/p".into()), index, trigger: wf.frontmatter.triggers[0].clone() }
    }

    #[test]
    fn each_workflow_gets_its_first_matching_trigger() {
        let subs = [armed("a", "review.*", 0), armed("a", "review.requested", 1), armed("b", "review.**", 0), armed("c", "deploy", 0)];
        let m = matching(&subs, "review.requested");
        let got: Vec<(&str, usize)> = m.iter().map(|a| (a.name.as_str(), a.index)).collect();
        assert_eq!(got, [("a", 0), ("b", 0)]);
        assert!(matching(&subs, "other").is_empty());
    }

    #[test]
    fn depth_counts_up_from_the_starting_event() {
        let run = |cause: Option<Value>| Run {
            id: 1,
            workflow_name: "w".into(),
            workflow_path: None,
            project_path: None,
            params: json!({}),
            status: crate::store::RunStatus::Running,
            reason: None,
            summary: None,
            created_at: String::new(),
            finished_at: None,
            trigger: cause,
            workflow_snapshot: None,
        };
        assert_eq!(depth_from(&run(None)), 0);
        assert_eq!(depth_from(&run(Some(json!({ "kind": "cron" })))), 0);
        assert_eq!(depth_from(&run(Some(json!({ "event_id": 4, "depth": 2 })))), 3);
        assert_eq!(run_sender(3, Some("w1")), "run 3 worker w1");
    }
}
