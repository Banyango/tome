//! `tome runs list|show|logs` and `tome query`: inspection of current and
//! past runs. All data comes from the daemon.

use crate::output::{table, CliResult, Report};
use crate::paths;
use crate::rpc;
use serde_json::{json, Value};

fn call(method: &str, params: Value) -> CliResult<Value> {
    rpc::call(&paths::socket_path(), method, params)
}

fn s(v: &Value) -> String {
    match v {
        Value::Null => "-".into(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Timestamps render as `YYYY-MM-DD HH:MM:SS` in tables.
fn ts(v: &Value) -> String {
    match v.as_str() {
        Some(t) if t.len() >= 19 => t[..19].replace('T', " "),
        _ => s(v),
    }
}

pub fn list(status: Option<String>, workflow: Option<String>, limit: usize) -> CliResult<Report> {
    let data = call("runs.list", json!({ "status": status, "workflow": workflow, "limit": limit }))?;
    let runs = data["runs"].as_array().cloned().unwrap_or_default();
    let human = if runs.is_empty() {
        "no runs".to_string()
    } else {
        let rows = runs
            .iter()
            .map(|r| {
                vec![
                    s(&r["id"]),
                    s(&r["workflow_name"]),
                    s(&r["status"]),
                    ts(&r["created_at"]),
                    ts(&r["finished_at"]),
                    s(&r["reason"]),
                ]
            })
            .collect();
        table(&["ID", "WORKFLOW", "STATUS", "STARTED", "FINISHED", "REASON"], rows)
    };
    Ok(Report::new(data, human))
}

pub fn show(id: &str, snapshot: bool) -> CliResult<Report> {
    let data = call("runs.show", json!({ "id": id, "snapshot": snapshot }))?;
    let run = &data["run"];
    let mut out = format!(
        "run {}  {}  [{}]\n",
        s(&run["id"]),
        s(&run["workflow_name"]),
        s(&run["status"])
    );
    let field = |out: &mut String, label: &str, v: &Value| {
        if !v.is_null() {
            out.push_str(&format!("  {label:<10}{}\n", s(v)));
        }
    };
    field(&mut out, "reason", &run["reason"]);
    field(&mut out, "summary", &run["summary"]);
    field(&mut out, "project", &run["project_path"]);
    field(&mut out, "workflow", &run["workflow_path"]);
    out.push_str(&format!("  {:<10}{}\n", "started", ts(&run["created_at"])));
    if !run["finished_at"].is_null() {
        out.push_str(&format!("  {:<10}{}\n", "finished", ts(&run["finished_at"])));
    }
    if run["params"].as_object().is_some_and(|p| !p.is_empty()) {
        out.push_str(&format!("  {:<10}{}\n", "params", run["params"]));
    }

    let steps = data["steps"].as_array().cloned().unwrap_or_default();
    out.push_str("\nsteps:\n");
    if steps.is_empty() {
        out.push_str("  (none reported)\n");
    } else {
        let rows = steps
            .iter()
            .map(|st| {
                vec![
                    s(&st["name"]),
                    s(&st["status"]),
                    s(&st["attempts"]),
                    ts(&st["started_at"]),
                    ts(&st["finished_at"]),
                    s(&st["message"]),
                ]
            })
            .collect();
        out.push_str(&indent(&table(&["STEP", "STATUS", "ATTEMPTS", "STARTED", "FINISHED", "MESSAGE"], rows)));
    }

    let history = data["history"].as_array().cloned().unwrap_or_default();
    if !history.is_empty() {
        out.push_str("\nhistory:\n");
        for h in &history {
            let msg = h["message"].as_str().map(|m| format!("  {m}")).unwrap_or_default();
            out.push_str(&format!("  {}  {:<5} {}{}\n", ts(&h["occurred_at"]), s(&h["event"]), s(&h["step"]), msg));
        }
    }

    let worktrees = data["worktrees"].as_array().cloned().unwrap_or_default();
    if !worktrees.is_empty() {
        out.push_str("\nworktrees:\n");
        for w in &worktrees {
            let branch = w["branch"].as_str().map(|b| format!(" ({b})")).unwrap_or_default();
            out.push_str(&format!("  {}{branch}\n", s(&w["path"])));
        }
    }

    let logs = data["logs"].as_array().cloned().unwrap_or_default();
    if !logs.is_empty() {
        out.push_str("\nlogs:\n");
        for l in &logs {
            out.push_str(&format!("  {}  {} bytes  {}\n", s(&l["step"]), s(&l["size"]), s(&l["path"])));
        }
    }

    if let Some(snap) = run["workflow_snapshot"].as_str() {
        out.push_str("\nworkflow snapshot:\n");
        out.push_str(snap);
        out.push('\n');
    }
    Ok(Report::new(data, out))
}

pub fn logs(id: &str, step: Option<String>, tail: Option<usize>) -> CliResult<Report> {
    let data = call("runs.logs", json!({ "id": id, "step": step, "tail": tail }))?;
    let logs = data["logs"].as_array().cloned().unwrap_or_default();
    let human = match logs.as_slice() {
        [] => "no logs".to_string(),
        [only] if step.is_some() => s(&only["content"]),
        many => many
            .iter()
            .map(|l| format!("==> {} <==\n{}", s(&l["step"]), l["content"].as_str().unwrap_or("")))
            .collect::<Vec<_>>()
            .join("\n\n"),
    };
    Ok(Report::new(data, human))
}

pub fn query(sql: &str) -> CliResult<Report> {
    let data = call("query", json!({ "sql": sql }))?;
    let columns: Vec<String> =
        data["columns"].as_array().into_iter().flatten().map(|c| c.as_str().unwrap_or("").to_string()).collect();
    let rows: Vec<Vec<String>> = data["rows"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| r.as_array().into_iter().flatten().map(s).collect())
        .collect();
    let headers: Vec<&str> = columns.iter().map(String::as_str).collect();
    let n = rows.len();
    let human = format!("{}({n} row{})", table(&headers, rows), if n == 1 { "" } else { "s" });
    Ok(Report::new(data, human))
}

fn indent(text: &str) -> String {
    text.lines().map(|l| format!("  {l}\n")).collect()
}
