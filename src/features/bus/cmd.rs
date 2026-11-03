//! `tome publish` and `tome events ...`: the client side of the project
//! message bus.

use crate::features::nodes as node;
use crate::ids::RunId;
use crate::output::{table, CliError, CliResult, Report};
use crate::scope::Shared;
use crate::triggerscmd::project_of;
use crate::{paths, rpc};
use serde_json::{json, Value};
use std::io::Read;
use std::path::{Path, PathBuf};

fn call(method: &str, params: Value) -> CliResult<Value> {
    rpc::call(&paths::socket_path(), method, params)
}

/// Add a shared bus's `hub` / `global` params. On another node a shared
/// bus is the node's own: no project, marked as from this machine.
fn on_bus(mut params: Value, bus: &Option<Shared>) -> Value {
    match bus {
        Some(Shared::Hub(h)) => params["hub"] = json!(h.as_str()),
        Some(Shared::Global) => params["global"] = json!(true),
        None => return params,
    }
    if node::target().is_some() {
        if let Some(m) = params.as_object_mut() {
            m.remove("project_path");
        }
        if !params["forwarded"].is_object() {
            params["from_node"] = json!(node::host_name());
        }
    }
    params
}

/// The project an `events` command is about; none for a node's shared bus.
fn project_for(cwd: &Path, bus: &Option<Shared>) -> CliResult<String> {
    if bus.is_some() && node::target().is_some() {
        return Ok(String::new());
    }
    project(cwd)
}

/// ` on hub release`, ` on global`, or nothing for the project's bus.
fn bus_suffix(bus: &Option<Shared>) -> String {
    bus.as_ref()
        .map_or_else(String::new, |b| format!(" on {}", b.label()))
}

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

/// `tome publish <topic> <text|-> [--dry-run]`: inside a run
/// (`TOME_RUN_ID`) it goes to the run's project, else to the current one.
pub fn publish(
    cwd: &Path,
    topic: &str,
    text: String,
    dry_run: bool,
    bus: &Option<Shared>,
) -> CliResult<Report> {
    let payload = if text == "-" {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        buf
    } else {
        text
    };
    let run = env("TOME_RUN_ID");
    if let Some(node) = node::target() {
        let out = publish_to(node, cwd, topic, &payload, run.as_deref(), dry_run, bus)?;
        return Ok(Report::new(out.clone(), publish_human(&out)));
    }
    let project = project_of(cwd);
    if run.is_none() && project.is_none() {
        return Err(CliError::invalid("not inside a project")
            .with_hint("run `tome publish` in a project (a directory with .tome/)"));
    }
    let out = call(
        "events.publish",
        on_bus(
            json!({
                "topic": topic,
                "payload": payload,
                "project_path": project,
                "run_id": run,
                "worker": env("TOME_WORKER_ID"),
                "dry_run": dry_run,
            }),
            bus,
        ),
    )?;
    Ok(Report::new(out.clone(), publish_human(&out)))
}

/// `tome publish --on <node>`: to the node's copy of this project (inside
/// a run, the run's project), or with `--hub`/`--global` to the node's hub
/// or global bus, sent as forwarded from this machine so the node's
/// subscribers see who published it and the chain keeps its depth.
fn publish_to(
    node: &node::Node,
    cwd: &Path,
    topic: &str,
    payload: &str,
    run: Option<&str>,
    dry_run: bool,
    bus: &Option<Shared>,
) -> CliResult<Value> {
    let (local, sender, depth) = match run {
        Some(id) => {
            let shown = rpc::Client::local(&paths::socket_path())?
                .call("runs.show", json!({ "id": id }))?;
            let run = &shown["run"];
            let id = RunId::new(run["id"].as_i64().unwrap_or_default());
            let project = run["project_path"].as_str().map(PathBuf::from);
            let depth = Some(&run["trigger"])
                .filter(|t| t["event_id"].is_i64())
                .and_then(|t| t["depth"].as_i64())
                .map_or(0, |d| d + 1);
            let worker = env("TOME_WORKER_ID");
            (
                project,
                crate::ids::run_sender(id, worker.as_deref()),
                depth,
            )
        }
        None => (project_of(cwd), "user".to_string(), 0),
    };
    let mut params = json!({
        "topic": topic,
        "payload": payload,
        "dry_run": dry_run,
        "forwarded": { "origin": node::host_name(), "sender": sender, "depth": depth },
    });
    if bus.is_some() {
        params = on_bus(params, bus);
    } else {
        let local = local.ok_or_else(|| {
            CliError::invalid("not inside a project")
                .with_hint("run `tome publish` in a project (a directory with .tome/)")
        })?;
        params["project_path"] = json!(node::map_project(&local)?);
    }
    let mut client = rpc::Client::to_node(node)?;
    let mut out = client.call("events.publish", params)?;
    out["node"] = json!(node.name);
    Ok(out)
}

fn publish_human(out: &Value) -> String {
    let matches = out["matches"].as_array().cloned().unwrap_or_default();
    if out["dry_run"] == true {
        if matches.is_empty() {
            return format!("dry run: no workflow subscribes to {}", text(&out["topic"]));
        }
        let mut s = format!("dry run: {} would be delivered to:", text(&out["topic"]));
        for m in &matches {
            let what = m["would"]
                .as_str()
                .map(|w| format!(": {w}"))
                .unwrap_or_default();
            s.push_str(&format!(
                "\n  {} ({}){what}",
                text(&m["workflow"]),
                text(&m["on"])
            ));
        }
        return s;
    }
    let names: Vec<String> = out["delivered_to"]
        .as_array()
        .into_iter()
        .flatten()
        .map(text)
        .collect();
    let to = if names.is_empty() {
        "no subscribers".to_string()
    } else {
        names.join(", ")
    };
    let bus = out["bus"]
        .as_str()
        .map(|b| format!(" ({b})"))
        .unwrap_or_default();
    format!(
        "event {} on {}{bus}{}: delivered to {to}",
        out["event"]["id"],
        text(&out["topic"]),
        node::on_suffix()
    )
}

fn project(cwd: &Path) -> CliResult<String> {
    node::project(cwd)?
        .map(|p| p.display().to_string())
        .ok_or_else(|| {
            CliError::invalid("not inside a project")
                .with_hint("the bus belongs to a project: run this in one")
        })
}

/// How long ago a stored (UTC) timestamp was: `42s`, `5m`, `3h`, `2d`.
fn age(v: &Value) -> String {
    let Some(t) = v
        .as_str()
        .and_then(|t| chrono::NaiveDateTime::parse_from_str(t, "%Y-%m-%dT%H:%M:%S%.fZ").ok())
    else {
        return "-".into();
    };
    let secs = (chrono::Utc::now().naive_utc() - t).num_seconds().max(0);
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86400),
    }
}

/// `tome events ls`
pub fn ls(cwd: &Path, bus: &Option<Shared>) -> CliResult<Report> {
    let project = project_for(cwd, bus)?;
    let out = call("events.ls", on_bus(json!({ "project_path": project }), bus))?;
    let topics = out["topics"].as_array().cloned().unwrap_or_default();
    if topics.is_empty() {
        let place = bus.as_ref().map_or(project, Shared::label);
        return Ok(Report::new(out, format!("no events published in {place}")));
    }
    let rows = topics
        .iter()
        .map(|t| {
            let subs: Vec<String> = t["subscribers"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|s| {
                    let counts: Vec<String> = ["pending", "claimed", "failed"]
                        .iter()
                        .filter(|k| s[**k].as_i64().unwrap_or(0) > 0)
                        .map(|k| format!("{} {k}", s[*k]))
                        .collect();
                    if counts.is_empty() {
                        text(&s["workflow"])
                    } else {
                        format!("{} ({})", text(&s["workflow"]), counts.join(", "))
                    }
                })
                .collect();
            vec![
                text(&t["topic"]),
                text(&t["events"]),
                format!("{} ago", age(&t["last_published"])),
                subs.join("; "),
            ]
        })
        .collect();
    let human = table(&["TOPIC", "EVENTS", "LAST", "SUBSCRIBERS"], rows);
    Ok(Report::new(out, human))
}

/// `tome events show <topic> [--all]`
pub fn show(cwd: &Path, topic: &str, all: bool, bus: &Option<Shared>) -> CliResult<Report> {
    let project = project_for(cwd, bus)?;
    let out = call(
        "events.show",
        on_bus(
            json!({ "project_path": project, "topic": topic, "all": all }),
            bus,
        ),
    )?;
    let events = out["events"].as_array().cloned().unwrap_or_default();
    let shared = bus.is_some();
    let mut human = if events.is_empty() {
        format!("no unsettled events on {topic}{}", bus_suffix(bus))
    } else {
        let rows = events
            .iter()
            .map(|e| {
                let deliveries: Vec<String> = e["deliveries"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|d| {
                        let runs: Vec<String> = d["run_ids"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(text)
                            .collect();
                        let runs = if runs.is_empty() {
                            String::new()
                        } else {
                            format!(" (run {})", runs.join(", "))
                        };
                        format!("{}: {}{runs}", text(&d["workflow"]), text(&d["state"]))
                    })
                    .collect();
                let deliveries = match (deliveries.is_empty(), e["refused"].as_str()) {
                    (_, Some(why)) => format!("refused: {why}"),
                    (true, None) => "no subscribers".into(),
                    (false, None) => deliveries.join("; "),
                };
                let sender = if shared {
                    format!("{} ({})", text(&e["sender"]), text(&e["publisher"]))
                } else {
                    text(&e["sender"])
                };
                vec![
                    text(&e["id"]),
                    sender,
                    age(&e["published_at"]),
                    text(&e["preview"]),
                    deliveries,
                ]
            })
            .collect();
        table(&["ID", "SENDER", "AGE", "PAYLOAD", "DELIVERIES"], rows)
    };
    let hidden = out["hidden"].as_u64().unwrap_or(0);
    if hidden > 0 {
        human.push_str(&format!(
            "\n({hidden} settled event{} hidden; --all shows them)",
            if hidden == 1 { "" } else { "s" }
        ));
    }
    Ok(Report::new(out, human))
}

fn move_delivery(
    cwd: &Path,
    method: &str,
    event: i64,
    workflow: Option<&str>,
    verb: &str,
    bus: &Option<Shared>,
) -> CliResult<Report> {
    let project = project_for(cwd, bus)?;
    let out = call(
        method,
        on_bus(
            json!({ "project_path": project, "event": event, "workflow": workflow }),
            bus,
        ),
    )?;
    let human = if out["deleted"] == true {
        format!("removed event {event}: it had no deliveries")
    } else {
        format!(
            "{verb} event {} for {}",
            event,
            text(&out["delivery"]["workflow"])
        )
    };
    Ok(Report::new(out, human))
}

/// `tome events retry <event> [--workflow <wf>]`
pub fn retry(
    cwd: &Path,
    event: i64,
    workflow: Option<&str>,
    bus: &Option<Shared>,
) -> CliResult<Report> {
    move_delivery(cwd, "events.retry", event, workflow, "requeued", bus)
}

/// `tome events remove <event> [--workflow <wf>]`
pub fn remove(
    cwd: &Path,
    event: i64,
    workflow: Option<&str>,
    bus: &Option<Shared>,
) -> CliResult<Report> {
    move_delivery(cwd, "events.remove", event, workflow, "dropped", bus)
}

pub fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}
