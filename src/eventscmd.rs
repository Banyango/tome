//! `tome publish` and `tome events ...`: the client side of the project
//! message bus.

use crate::output::{table, CliError, CliResult, Report};
use crate::triggerscmd::project_of;
use crate::{paths, rpc};
use serde_json::{json, Value};
use std::io::Read;
use std::path::Path;

fn call(method: &str, params: Value) -> CliResult<Value> {
    rpc::call(&paths::socket_path(), method, params)
}

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

/// `tome publish <topic> <text|-> [--dry-run]`: inside a run
/// (`TOME_RUN_ID`) it goes to the run's project, else to the current one.
pub fn publish(cwd: &Path, topic: &str, text: String, dry_run: bool) -> CliResult<Report> {
    let payload = if text == "-" {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        buf
    } else {
        text
    };
    let run = env("TOME_RUN_ID");
    let project = project_of(cwd);
    if run.is_none() && project.is_none() {
        return Err(CliError::invalid("not inside a project").with_hint("run `tome publish` in a project (a directory with .tome/)"));
    }
    let out = call(
        "events.publish",
        json!({
            "topic": topic,
            "payload": payload,
            "project_path": project,
            "run_id": run,
            "worker": env("TOME_WORKER_ID"),
            "dry_run": dry_run,
        }),
    )?;
    Ok(Report::new(out.clone(), publish_human(&out)))
}

fn publish_human(out: &Value) -> String {
    let matches = out["matches"].as_array().cloned().unwrap_or_default();
    if out["dry_run"] == true {
        if matches.is_empty() {
            return format!("dry run: no workflow subscribes to {}", text(&out["topic"]));
        }
        let mut s = format!("dry run: {} would be delivered to:", text(&out["topic"]));
        for m in &matches {
            let what = m["would"].as_str().map(|w| format!(": {w}")).unwrap_or_default();
            s.push_str(&format!("\n  {} ({}){what}", text(&m["workflow"]), text(&m["on"])));
        }
        return s;
    }
    let names: Vec<String> = out["delivered_to"].as_array().into_iter().flatten().map(text).collect();
    let to = if names.is_empty() { "no subscribers".to_string() } else { names.join(", ") };
    format!("event {} on {}: delivered to {to}", out["event"]["id"], text(&out["topic"]))
}

fn project(cwd: &Path) -> CliResult<String> {
    project_of(cwd)
        .map(|p| p.display().to_string())
        .ok_or_else(|| CliError::invalid("not inside a project").with_hint("the bus belongs to a project: run this in one"))
}

/// How long ago a stored (UTC) timestamp was: `42s`, `5m`, `3h`, `2d`.
fn age(v: &Value) -> String {
    let Some(t) = v.as_str().and_then(|t| chrono::NaiveDateTime::parse_from_str(t, "%Y-%m-%dT%H:%M:%S%.fZ").ok()) else {
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
pub fn ls(cwd: &Path) -> CliResult<Report> {
    let project = project(cwd)?;
    let out = call("events.ls", json!({ "project_path": project }))?;
    let topics = out["topics"].as_array().cloned().unwrap_or_default();
    if topics.is_empty() {
        return Ok(Report::new(out, format!("no events published in {project}")));
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
            vec![text(&t["topic"]), text(&t["events"]), format!("{} ago", age(&t["last_published"])), subs.join("; ")]
        })
        .collect();
    let human = table(&["TOPIC", "EVENTS", "LAST", "SUBSCRIBERS"], rows);
    Ok(Report::new(out, human))
}

/// `tome events show <topic> [--all]`
pub fn show(cwd: &Path, topic: &str, all: bool) -> CliResult<Report> {
    let project = project(cwd)?;
    let out = call("events.show", json!({ "project_path": project, "topic": topic, "all": all }))?;
    let events = out["events"].as_array().cloned().unwrap_or_default();
    let mut human = if events.is_empty() {
        format!("no unsettled events on {topic}")
    } else {
        let rows = events
            .iter()
            .map(|e| {
                let deliveries: Vec<String> = e["deliveries"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|d| {
                        let runs: Vec<String> = d["run_ids"].as_array().into_iter().flatten().map(text).collect();
                        let runs = if runs.is_empty() { String::new() } else { format!(" (run {})", runs.join(", ")) };
                        format!("{}: {}{runs}", text(&d["workflow"]), text(&d["state"]))
                    })
                    .collect();
                let deliveries = match (deliveries.is_empty(), e["refused"].as_str()) {
                    (_, Some(why)) => format!("refused: {why}"),
                    (true, None) => "no subscribers".into(),
                    (false, None) => deliveries.join("; "),
                };
                vec![text(&e["id"]), text(&e["sender"]), age(&e["published_at"]), text(&e["preview"]), deliveries]
            })
            .collect();
        table(&["ID", "SENDER", "AGE", "PAYLOAD", "DELIVERIES"], rows)
    };
    let hidden = out["hidden"].as_u64().unwrap_or(0);
    if hidden > 0 {
        human.push_str(&format!("\n({hidden} settled event{} hidden; --all shows them)", if hidden == 1 { "" } else { "s" }));
    }
    Ok(Report::new(out, human))
}

fn move_delivery(cwd: &Path, method: &str, event: i64, workflow: Option<&str>, verb: &str) -> CliResult<Report> {
    let project = project(cwd)?;
    let out = call(method, json!({ "project_path": project, "event": event, "workflow": workflow }))?;
    let human = format!("{verb} event {} for {}", event, text(&out["delivery"]["workflow"]));
    Ok(Report::new(out, human))
}

/// `tome events retry <event> [--workflow <wf>]`
pub fn retry(cwd: &Path, event: i64, workflow: Option<&str>) -> CliResult<Report> {
    move_delivery(cwd, "events.retry", event, workflow, "requeued")
}

/// `tome events remove <event> [--workflow <wf>]`
pub fn remove(cwd: &Path, event: i64, workflow: Option<&str>) -> CliResult<Report> {
    move_delivery(cwd, "events.remove", event, workflow, "dropped")
}

pub fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}
