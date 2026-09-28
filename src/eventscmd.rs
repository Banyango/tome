//! `tome publish` and `tome events ...`: the client side of the project
//! message bus.

use crate::output::{CliError, CliResult, Report};
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

pub fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}
