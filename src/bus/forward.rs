//! Forwarding events to other machines: a project's `bus.forward` rules
//! (`.tome/config.yaml`) give each matching event a delivery to a node,
//! which the daemon sends by publishing the event on the node's copy of the
//! project.
//!
//! A forward delivery is an ordinary delivery whose subscriber is `→ <node>`
//! (its workflow path `node:<node>`), so `tome events` shows, retries and
//! removes it like any other. Unsent ones stay `pending` and are retried
//! with backoff for a day.

use super::{Publish, TOME};
use crate::config::Config;
use crate::engine::Engine;
use crate::ids::DeliveryId;
use crate::node::{self, Node};
use crate::output::{CliError, CliResult, ErrorKind};
use crate::rpc;
use crate::store::{BusEvent, Delivery, DeliveryState, Subscriber};
use crate::topic;
use serde_json::{json, Value};
use serde_yaml::Value as Yaml;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How a forward delivery's subscriber is named: `→ mini`.
pub const ARROW: &str = "→ ";

/// The workflow path of a forward delivery: `node:mini`.
const PATH: &str = "node:";

/// How long an unsent delivery keeps being retried.
const GIVE_UP: Duration = Duration::from_secs(24 * 3600);

/// The longest wait between attempts.
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// One `bus.forward` rule.
#[derive(Debug, Clone)]
pub struct Rule {
    pub pattern: topic::Pattern,
    pub to: Vec<String>,
}

/// The node a delivery forwards to, if it's a forward delivery.
pub fn node_of(d: &Delivery) -> Option<&str> {
    d.workflow_path.strip_prefix(PATH)
}

/// Whether the subscriber name `wf` (as `--workflow` gives it) means this
/// delivery: `mini` and `→ mini` both name a forward to mini.
pub fn names(d: &Delivery, wf: &str) -> bool {
    d.workflow == wf || node_of(d).is_some_and(|n| n == wf.trim_start_matches(ARROW).trim())
}

/// A project's forwarding rules. A malformed `bus` is an error.
pub fn rules(project: &Path) -> CliResult<Vec<Rule>> {
    let cfg = Config::load(Some(project))?;
    let Some((bus, file)) = cfg.get("bus") else {
        return Ok(Vec::new());
    };
    let bus = bus
        .as_mapping()
        .ok_or_else(|| file.error("`bus` must be a mapping"))?;
    let mut out = Vec::new();
    for (key, value) in bus {
        let key = key.as_str().unwrap_or("?");
        if key != "forward" {
            return Err(file
                .error(format!("unknown key `bus.{key}`"))
                .with_hint("`bus` takes `forward`"));
        }
        if value.is_null() {
            continue;
        }
        let rules = value
            .as_sequence()
            .ok_or_else(|| file.error("`bus.forward` must be a list of {topic, to} rules"))?;
        for (i, rule) in rules.iter().enumerate() {
            let at = format!("bus.forward[{i}]");
            let rule = rule.as_mapping().ok_or_else(|| {
                file.error(format!("`{at}` must be a mapping with `topic` and `to`"))
            })?;
            for k in rule.keys() {
                let k = k.as_str().unwrap_or("?");
                if k != "topic" && k != "to" {
                    return Err(file
                        .error(format!("unknown key `{at}.{k}`"))
                        .with_hint("a forwarding rule takes `topic` and `to`"));
                }
            }
            let pattern = rule
                .get("topic")
                .and_then(Yaml::as_str)
                .ok_or_else(|| file.error(format!("`{at}.topic` must be a topic pattern")))?;
            let pattern = topic::Pattern::parse(pattern)
                .map_err(|e| file.error(format!("`{at}.topic`: {e}")))?;
            let to: Vec<String> = match rule.get("to") {
                Some(Yaml::String(s)) => vec![s.clone()],
                Some(Yaml::Sequence(list)) => list
                    .iter()
                    .map(|n| n.as_str().map(str::to_string))
                    .collect::<Option<_>>()
                    .ok_or_else(|| file.error(format!("`{at}.to` must be node names")))?,
                _ => {
                    return Err(
                        file.error(format!("`{at}.to` must be a node name or a list of them"))
                    )
                }
            };
            if to.is_empty() {
                return Err(file.error(format!("`{at}.to` names no node")));
            }
            for n in &to {
                node::check_name(n).map_err(|e| file.error(format!("`{at}.to`: {e}")))?;
            }
            out.push(Rule { pattern, to });
        }
    }
    Ok(out)
}

/// What `tome validate` says about a project's forwarding rules: errors for
/// malformed ones, warnings for nodes this machine doesn't know (the global
/// config can differ from machine to machine).
pub fn check(project: &Path) -> (Vec<String>, Vec<String>) {
    let rules = match rules(project) {
        Ok(r) => r,
        Err(e) => return (vec![e.message], Vec::new()),
    };
    let known: HashSet<String> = node::all()
        .map(|nodes| nodes.into_keys().collect())
        .unwrap_or_default();
    let mut warnings = Vec::new();
    for (i, r) in rules.iter().enumerate() {
        for n in r.to.iter().filter(|n| !known.contains(*n)) {
            warnings.push(format!(
                "bus.forward[{i}] ({}) forwards to `{n}`, which isn't a node here (add it with `tome node add {n} <ssh destination>`)",
                r.pattern
            ));
        }
    }
    (Vec::new(), warnings)
}

/// The origin of an event that came from another machine: `mini` for
/// `node mini (run 4)`.
fn origin(sender: &str) -> Option<&str> {
    let rest = sender.strip_prefix("node ")?;
    Some(rest.split_once(" (").map_or(rest, |(o, _)| o))
}

/// The forward deliveries an event gets: one per node its topic is
/// forwarded to, never back to the node it came from.
pub fn subscribers(e: &Publish<'_>) -> Vec<Subscriber> {
    let rules = match rules(e.project) {
        Ok(r) => r,
        Err(err) => {
            eprintln!("tome daemon: not forwarding {}: {}", e.topic, err.message);
            return Vec::new();
        }
    };
    let from = origin(&e.sender);
    let mut out: Vec<Subscriber> = Vec::new();
    for r in rules.iter().filter(|r| r.pattern.matches(e.topic)) {
        for n in &r.to {
            if Some(n.as_str()) == from
                || out.iter().any(|s| s.workflow_path == format!("{PATH}{n}"))
            {
                continue;
            }
            out.push(Subscriber {
                workflow_name: format!("{ARROW}{n}"),
                workflow_path: format!("{PATH}{n}"),
                pattern: r.pattern.to_string(),
            });
        }
    }
    out
}

/// What the forwarder remembers between ticks. Attempts aren't stored: a
/// restarted daemon tries everything again straight away.
#[derive(Default)]
pub struct State {
    /// Nodes being sent to right now.
    busy: HashSet<String>,
    /// Per delivery: when it was made pending (its `updated_at`), failed
    /// attempts since, and when to try next.
    attempts: HashMap<DeliveryId, (String, u32, Instant)>,
    /// Nodes whose authentication failure has been notified.
    auth_told: HashSet<String>,
}

/// The wait after `n` failed attempts: 5s, 10s, 20s, 40s, then a minute.
fn backoff(n: u32) -> Duration {
    Duration::from_secs(5)
        .saturating_mul(1 << n.saturating_sub(1).min(6))
        .min(MAX_BACKOFF)
}

/// How long ago a stored (UTC) timestamp was.
fn age(t: &str) -> Duration {
    chrono::NaiveDateTime::parse_from_str(t, "%Y-%m-%dT%H:%M:%S%.fZ")
        .ok()
        .and_then(|t| (chrono::Utc::now().naive_utc() - t).to_std().ok())
        .unwrap_or_default()
}

/// The payload as the node gets it: a `tome.*` event's JSON gains `node`,
/// the machine it happened on.
fn payload_for(event: &BusEvent) -> String {
    if !topic::is_reserved(&event.topic) || event.sender != TOME {
        return event.payload.clone();
    }
    match serde_json::from_str::<Value>(&event.payload) {
        Ok(Value::Object(mut map)) => {
            map.entry("node")
                .or_insert_with(|| json!(node::host_name()));
            Value::Object(map).to_string()
        }
        _ => event.payload.clone(),
    }
}

impl Engine {
    /// A forward delivery that can't be sent: `failed`, and notified.
    fn forward_failed(&self, d: &Delivery, why: &str) {
        let moved = self.with_store(|store| {
            let event = store.bus_event(d.event_id)?;
            Ok(store
                .move_delivery(d.id, DeliveryState::Pending, DeliveryState::Failed, None)?
                .then_some(event)
                .flatten())
        });
        if let Ok(Some(event)) = moved {
            eprintln!(
                "tome daemon: delivery {} of event {} {}: failed: {why}",
                d.id, d.event_id, d.workflow
            );
            crate::orchestrator::notify_delivery(
                &format!("couldn't forward {}", event.topic),
                &format!("event {} {}: {why}", event.id, d.workflow),
            );
        }
    }

    /// Fail forward deliveries to nodes this machine doesn't know: straight
    /// away, since waiting won't help.
    pub(crate) fn forward_unknown(&self, deliveries: &[Delivery]) {
        // An unreadable config is reported elsewhere; the sender says why.
        let Ok(nodes) = node::all() else {
            return;
        };
        let known: HashSet<String> = nodes.into_keys().collect();
        for d in deliveries {
            if let Some(n) = node_of(d).filter(|n| !known.contains(*n)) {
                self.forward_failed(
                    d,
                    &format!("`{n}` isn't a node here (add it with `tome node add {n} <ssh destination>`)"),
                );
            }
        }
    }

    /// Send the pending forward deliveries that are due, each node's on its
    /// own thread so a slow node doesn't hold up the trigger loop. Called
    /// every trigger-loop tick.
    pub(crate) fn forward_pending(self: &Arc<Self>) {
        let pending = match self.with_store(|store| store.deliveries_in(DeliveryState::Pending)) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("tome daemon: reading deliveries failed: {}", e.message);
                return;
            }
        };
        let mut by_node: BTreeMap<String, Vec<Delivery>> = BTreeMap::new();
        for d in pending {
            if let Some(n) = node_of(&d) {
                by_node.entry(n.to_string()).or_default().push(d);
            }
        }
        if by_node.is_empty() {
            return;
        }
        let now = Instant::now();
        for (name, deliveries) in by_node {
            let mut due = Vec::new();
            {
                let mut st = self.forwarding.lock().unwrap_or_else(|p| p.into_inner());
                if st.busy.contains(&name) {
                    continue;
                }
                for d in deliveries {
                    if age(&d.updated_at) > GIVE_UP {
                        due.push((d, true));
                        continue;
                    }
                    match st.attempts.get(&d.id) {
                        Some((since, _, next)) if *since == d.updated_at && *next > now => {}
                        _ => due.push((d, false)),
                    }
                }
                if due.is_empty() {
                    continue;
                }
                st.busy.insert(name.clone());
            }
            let engine = Arc::clone(self);
            std::thread::spawn(move || {
                engine.forward_to(&name, due);
                engine
                    .forwarding
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .busy
                    .remove(&name);
            });
        }
    }

    /// Send `due` deliveries to one node, oldest first, over one connection.
    /// The flag marks deliveries that have waited too long.
    fn forward_to(&self, name: &str, due: Vec<(Delivery, bool)>) {
        let mut rest = Vec::new();
        for (d, expired) in due {
            if expired {
                let tries = self.attempts_of(d.id);
                self.forward_failed(
                    &d,
                    &format!("{name} couldn't be reached for a day ({tries} attempts)"),
                );
            } else {
                rest.push(d);
            }
        }
        if rest.is_empty() {
            return;
        }
        let node = match node::get(name) {
            Ok(n) => n,
            Err(e) => {
                for d in &rest {
                    self.forward_failed(d, &e.message);
                }
                return;
            }
        };
        let mut client = match rpc::Client::to_node(&node) {
            Ok(c) => c,
            Err(e) => return self.forward_retry(&node, &rest, &e),
        };
        let host = client
            .pong
            .as_ref()
            .and_then(|p| p["host"].as_str())
            .map(str::to_string);
        // Each project is located on the node once per connection.
        let mut located: HashMap<String, CliResult<String>> = HashMap::new();
        for (i, d) in rest.iter().enumerate() {
            match self.send_one(&mut client, &node, host.as_deref(), d, &mut located) {
                Ok(()) => {
                    self.forwarding
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .attempts
                        .remove(&d.id);
                }
                Err(e) if is_transport(&e) => return self.forward_retry(&node, &rest[i..], &e),
                Err(e) => {
                    let why = match &e.hint {
                        Some(h) => format!("{} ({h})", e.message),
                        None => e.message.clone(),
                    };
                    self.forward_failed(d, &why);
                }
            }
        }
    }

    fn attempts_of(&self, id: DeliveryId) -> u32 {
        self.forwarding
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .attempts
            .get(&id)
            .map_or(0, |a| a.1)
    }

    /// Publish one delivery's event on the node.
    fn send_one(
        &self,
        client: &mut rpc::Client,
        node: &Node,
        host: Option<&str>,
        d: &Delivery,
        located: &mut HashMap<String, CliResult<String>>,
    ) -> CliResult<()> {
        let Some(event) = self.with_store(|store| store.bus_event(d.event_id))? else {
            return Ok(());
        };
        // It came from that machine: never send it back.
        if host.is_some_and(|h| origin(&event.sender) == Some(h)) {
            self.with_store(|store| {
                store.move_delivery(d.id, DeliveryState::Pending, DeliveryState::Dropped, None)
            })?;
            eprintln!(
                "tome daemon: delivery {} of event {} {}: dropped: the event came from there",
                d.id, d.event_id, d.workflow
            );
            return Ok(());
        }
        let project = match located.get(&d.project_path) {
            Some(found) => found.clone()?,
            None => {
                let found =
                    node::locate(client, node, Path::new(&d.project_path), false).map(|l| l.path);
                located.insert(d.project_path.clone(), found.clone());
                found?
            }
        };
        let out = client.call(
            "events.publish",
            json!({
                "topic": event.topic,
                "payload": payload_for(&event),
                "project_path": project,
                "forwarded": {
                    "origin": node::host_name(),
                    "sender": event.sender,
                    "depth": event.depth,
                },
            }),
        )?;
        let sent = self.with_store(|store| {
            store.move_delivery(d.id, DeliveryState::Pending, DeliveryState::Done, None)
        })?;
        if sent {
            eprintln!(
                "tome daemon: delivery {} of event {} {}: sent as event {} there",
                d.id, d.event_id, d.workflow, out["event"]["id"]
            );
        }
        Ok(())
    }

    /// `node` couldn't be reached: try `ds` again later. The first
    /// authentication failure is notified, since it won't fix itself.
    fn forward_retry(&self, node: &Node, ds: &[Delivery], e: &CliError) {
        let now = Instant::now();
        let tell_auth = {
            let mut st = self.forwarding.lock().unwrap_or_else(|p| p.into_inner());
            for d in ds {
                let n = match st.attempts.get(&d.id) {
                    Some((since, n, _)) if *since == d.updated_at => n + 1,
                    _ => 1,
                };
                st.attempts
                    .insert(d.id, (d.updated_at.clone(), n, now + backoff(n)));
            }
            rpc::is_auth_failure(e) && st.auth_told.insert(node.name.clone())
        };
        eprintln!(
            "tome daemon: forwarding {} event(s) to {}: {}; will retry",
            ds.len(),
            node.name,
            e.message
        );
        if tell_auth {
            crate::orchestrator::notify_delivery(
                &format!("can't sign in to {} to forward events", node.name),
                &format!(
                    "{} The daemon may not see your ssh-agent: set an IdentityFile for {} in ~/.ssh/config. Events wait and retry for a day.",
                    e.message, node.ssh
                ),
            );
        }
    }
}

/// An error that means "try again later" rather than "this will never
/// work": the node unreachable, or the connection lost.
fn is_transport(e: &CliError) -> bool {
    e.kind == ErrorKind::Unreachable
        || (e.kind == ErrorKind::Internal
            && (e.message.contains("closed the connection")
                || e.message.contains("timed out")
                || e.message.contains("before the request was sent")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{EventId, RunId};

    #[test]
    fn backoff_doubles_up_to_a_minute() {
        let secs: Vec<u64> = (1..=8).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(secs, [5, 10, 20, 40, 60, 60, 60, 60]);
    }

    #[test]
    fn origins_come_from_forwarded_senders() {
        assert_eq!(origin("node mini (run 4)"), Some("mini"));
        assert_eq!(origin("node mini (node laptop (user))"), Some("mini"));
        assert_eq!(origin("run 4"), None);
        assert_eq!(origin("user"), None);
    }

    fn delivery(workflow: &str, path: &str) -> Delivery {
        Delivery {
            id: DeliveryId::new(1),
            event_id: EventId::new(1),
            project_path: "/p".into(),
            workflow: workflow.into(),
            workflow_path: path.into(),
            pattern: "x".into(),
            state: DeliveryState::Pending.into(),
            run_ids: Vec::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn forward_deliveries_answer_to_the_node_name() {
        let d = delivery("→ mini", "node:mini");
        assert_eq!(node_of(&d), Some("mini"));
        assert!(names(&d, "mini") && names(&d, "→ mini"));
        assert!(!names(&d, "build"));
        let w = delivery("mini", "/p/.tome/workflows/mini.md");
        assert_eq!(node_of(&w), None);
        assert!(names(&w, "mini"));
    }

    #[test]
    fn lifecycle_payloads_gain_the_node() {
        let mut e = BusEvent {
            id: EventId::new(1),
            project_path: "/p".into(),
            topic: "tome.run.build.succeeded".into(),
            payload: r#"{"run_id":3}"#.into(),
            sender: TOME.into(),
            sender_run_id: Some(RunId::new(3)),
            depth: 0,
            refused: None,
            published_at: String::new(),
        };
        let p: Value = serde_json::from_str(&payload_for(&e)).unwrap();
        assert_eq!(p["node"], json!(node::host_name()));
        assert_eq!(p["run_id"], 3);
        e.topic = "build.done".into();
        e.sender = "user".into();
        assert_eq!(payload_for(&e), r#"{"run_id":3}"#);
    }
}
