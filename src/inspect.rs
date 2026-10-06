//! `tome runs list|show|logs` and `tome query`: inspection of current and
//! past runs. All data comes from the daemon.

use crate::output::{table, CliResult, Report};
use crate::paths;
use crate::rpc;
use chrono::{Local, NaiveDateTime, TimeZone};
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

/// Stored timestamps are UTC (`2026-01-02T03:04:05.000Z`); human output
/// shows them in local time as `YYYY-MM-DD HH:MM:SS`.
pub fn ts(v: &Value) -> String {
    match v.as_str() {
        Some(t) => local_time(t).unwrap_or_else(|| t.to_string()),
        None => s(v),
    }
}

fn local_time(utc: &str) -> Option<String> {
    let naive = NaiveDateTime::parse_from_str(utc, "%Y-%m-%dT%H:%M:%S%.fZ").ok()?;
    Some(
        Local
            .from_utc_datetime(&naive)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string(),
    )
}

/// `{"ticket": "ABC-1", "n": 2}` as `ticket=ABC-1  n=2`.
fn params(v: &Value) -> String {
    v.as_object()
        .into_iter()
        .flatten()
        .map(|(k, v)| match v {
            Value::String(s) => format!("{k}={s}"),
            other => format!("{k}={other}"),
        })
        .collect::<Vec<_>>()
        .join("  ")
}

pub fn list(status: Option<String>, workflow: Option<String>, limit: usize) -> CliResult<Report> {
    let data = call(
        "runs.list",
        json!({ "status": status, "workflow": workflow, "limit": limit }),
    )?;
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
        table(
            &["ID", "WORKFLOW", "STATUS", "STARTED", "FINISHED", "REASON"],
            rows,
        )
    };
    Ok(Report::new(data, human))
}

pub fn show(id: &str, snapshot: bool) -> CliResult<Report> {
    let data = call("runs.show", json!({ "id": id, "snapshot": snapshot }))?;
    let run = &data["run"];
    let mut out = format!(
        "run {}{}  {}  [{}]\n",
        s(&run["id"]),
        crate::node::on_suffix(),
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
    field(&mut out, "mode", &run["mode"]);
    field(&mut out, "project", &run["project_path"]);
    field(&mut out, "workflow", &run["workflow_path"]);
    if let Some(t) = run["trigger"].as_object() {
        let mut cause = s(&t["trigger"]);
        let paths: Vec<String> = t
            .get("event")
            .and_then(|e| e["paths"].as_array())
            .into_iter()
            .flatten()
            .map(|p| format!("{} ({})", s(&p["path"]), s(&p["event"])))
            .collect();
        if !paths.is_empty() {
            cause.push_str(&format!(": {}", paths.join(", ")));
        }
        if t.get("synthetic") == Some(&Value::Bool(true)) {
            cause.push_str(" [fired by hand]");
        }
        out.push_str(&format!("  {:<10}{cause}\n", "trigger"));
        if t.get("event_id").is_some_and(|v| !v.is_null()) {
            out.push_str(&format!(
                "  {:<10}{} on {} from {}\n",
                "event",
                s(&t["event_id"]),
                s(&t["topic"]),
                s(&t["sender"])
            ));
        }
    }
    out.push_str(&format!("  {:<10}{}\n", "started", ts(&run["created_at"])));
    if !run["finished_at"].is_null() {
        out.push_str(&format!(
            "  {:<10}{}\n",
            "finished",
            ts(&run["finished_at"])
        ));
    }
    if run["params"].as_object().is_some_and(|p| !p.is_empty()) {
        out.push_str(&format!("  {:<10}{}\n", "params", params(&run["params"])));
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
        out.push_str(&indent(&table(
            &[
                "STEP", "STATUS", "ATTEMPTS", "STARTED", "FINISHED", "MESSAGE",
            ],
            rows,
        )));
    }

    let history = data["history"].as_array().cloned().unwrap_or_default();
    if !history.is_empty() {
        out.push_str("\nhistory:\n");
        let rows = history
            .iter()
            .map(|h| {
                vec![
                    ts(&h["occurred_at"]),
                    s(&h["event"]),
                    s(&h["step"]),
                    h["message"].as_str().unwrap_or("").to_string(),
                ]
            })
            .collect();
        out.push_str(&indent(&table(&["TIME", "EVENT", "STEP", "MESSAGE"], rows)));
    }

    let handshake = data["handshake"].as_array().cloned().unwrap_or_default();
    if !handshake.is_empty() {
        out.push_str("\nstart:\n");
        let rows = handshake
            .iter()
            .map(|h| {
                let main = match run["mode"].as_str() {
                    Some("single") => "agent",
                    _ => "orchestrator",
                };
                let agent = h["worker"]
                    .as_str()
                    .map_or(main.to_string(), |w| format!("worker {w}"));
                vec![
                    ts(&h["occurred_at"]),
                    agent,
                    s(&h["state"]),
                    h["message"].as_str().unwrap_or("").to_string(),
                ]
            })
            .collect();
        out.push_str(&indent(&table(
            &["TIME", "AGENT", "STATE", "MESSAGE"],
            rows,
        )));
    }

    let workers = data["workers"].as_array().cloned().unwrap_or_default();
    if !workers.is_empty() {
        out.push_str("\nworkers:\n");
        out.push_str(&indent(&crate::primitives::workers_table(&workers)));
    }

    let groups = data["groups"].as_array().cloned().unwrap_or_default();
    if !groups.is_empty() {
        out.push_str("\ngroups:\n");
        let rows = groups
            .iter()
            .map(|g| {
                vec![
                    s(&g["name"]),
                    s(&g["status"]),
                    if g["fail_fast"] == true { "yes" } else { "no" }.to_string(),
                    ts(&g["finished_at"]),
                ]
            })
            .collect();
        out.push_str(&indent(&table(
            &["GROUP", "STATUS", "FAIL-FAST", "FINISHED"],
            rows,
        )));
    }

    let worktrees = data["worktrees"].as_array().cloned().unwrap_or_default();
    if !worktrees.is_empty() {
        out.push_str("\nworktrees:\n");
        for w in &worktrees {
            let branch = w["branch"]
                .as_str()
                .map(|b| format!(" ({b})"))
                .unwrap_or_default();
            out.push_str(&format!("  {}{branch}\n", s(&w["path"])));
        }
    }

    let sessions = data["sessions"].as_array().cloned().unwrap_or_default();
    if !sessions.is_empty() {
        out.push_str("\nsessions:\n");
        let rows = sessions
            .iter()
            .map(|x| {
                vec![
                    s(&x["name"]),
                    s(&x["role"]),
                    x["harness"].as_str().unwrap_or("").to_string(),
                    x["agent_status"]
                        .as_str()
                        .map(|status| {
                            if let Some(at) = x["blocked_at"].as_str() {
                                format!("{status} since {at}")
                            } else {
                                status.to_string()
                            }
                        })
                        .unwrap_or_default(),
                    s(&x["attach"]),
                ]
            })
            .collect();
        out.push_str(&indent(&table(
            &["SESSION", "ROLE", "HARNESS", "AGENT", "ATTACH"],
            rows,
        )));
    }
    out.push_str(&placement(&run["placement"], &sessions));

    let logs = data["logs"].as_array().cloned().unwrap_or_default();
    if !logs.is_empty() {
        out.push_str("\nlogs:\n");
        let rows = logs
            .iter()
            .map(|l| vec![s(&l["step"]), size(&l["size"]), s(&l["path"])])
            .collect();
        out.push_str(&indent(&table(&["STEP", "SIZE", "PATH"], rows)));
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
            .map(|l| {
                format!(
                    "==> {} <==\n{}",
                    s(&l["step"]),
                    l["content"].as_str().unwrap_or("")
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n"),
    };
    Ok(Report::new(data, human))
}

pub fn query(sql: &str) -> CliResult<Report> {
    let data = call("query", json!({ "sql": sql }))?;
    let columns: Vec<String> = data["columns"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| c.as_str().unwrap_or("").to_string())
        .collect();
    let rows: Vec<Vec<String>> = data["rows"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| r.as_array().into_iter().flatten().map(query_cell).collect())
        .collect();
    let headers: Vec<&str> = columns.iter().map(String::as_str).collect();
    let n = rows.len();
    let human = format!(
        "{}({n} row{})",
        table(&headers, rows),
        if n == 1 { "" } else { "s" }
    );
    Ok(Report::new(data, human))
}

/// Query results show SQL NULL as `NULL`, so it isn't confused with a `-`
/// string.
fn query_cell(v: &Value) -> String {
    if v.is_null() {
        "NULL".into()
    } else {
        s(v)
    }
}

fn size(v: &Value) -> String {
    let Some(n) = v.as_u64() else { return s(v) };
    match n {
        n if n < 1024 => format!("{n} B"),
        n if n < 1024 * 1024 => format!("{:.1} KB", n as f64 / 1024.0),
        n => format!("{:.1} MB", n as f64 / (1024.0 * 1024.0)),
    }
}

/// Each session's resolved placement, where each setting came from and any
/// warnings, then the run's placement notes. Empty if there's none of that
/// (sessions from before placements).
fn placement(run: &Value, sessions: &[Value]) -> String {
    let mut out = String::new();
    for x in sessions {
        let p = &x["placement"];
        if !p.is_object() {
            continue;
        }
        out.push_str(&format!("  {}\n", s(&x["name"])));
        let rows = [
            ("layout", &p["layout"]),
            ("workspace", &p["workspace"]),
            ("split.direction", &p["direction"]),
            ("split.size", &p["size"]),
            ("from", &p["from"]),
        ]
        .into_iter()
        .filter(|(_, v)| !v.is_null())
        .map(|(key, v)| {
            vec![
                key.to_string(),
                s(v),
                p["sources"][key].as_str().unwrap_or("").to_string(),
            ]
        })
        .collect();
        out.push_str(&indent(&indent(&table(
            &["SETTING", "VALUE", "FROM"],
            rows,
        ))));
        if let Some(at) = p["caller"].as_str() {
            out.push_str(&format!("    caller: opened next to {at}\n"));
        }
        for w in p["warnings"].as_array().into_iter().flatten() {
            out.push_str(&format!("    warning: {}\n", s(w)));
        }
    }
    let notes: Vec<String> = run["notes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|n| format!("  note: {}\n", s(n)))
        .collect();
    if let Some(id) = run["focused"]["id"].as_str() {
        out.push_str(&format!("  focused workspace at start: {id}\n"));
    }
    if let Some(surface) = run["caller"]["surface"].as_str() {
        let mut at = format!("surface {surface}");
        if let Some(pane) = run["caller"]["pane"].as_str() {
            at = format!("pane {pane} ({at})");
        }
        if let Some(ws) = run["caller"]["workspace"].as_str() {
            at.push_str(&format!(" in workspace {ws}"));
        }
        out.push_str(&format!("  caller at start: {at}\n"));
    }
    out.push_str(&notes.concat());
    if out.is_empty() {
        return out;
    }
    format!("\nplacement:\n{out}")
}

fn indent(text: &str) -> String {
    text.lines().map(|l| format!("  {l}\n")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_render_in_local_time() {
        let utc =
            NaiveDateTime::parse_from_str("2026-01-02T03:04:05", "%Y-%m-%dT%H:%M:%S").unwrap();
        let expected = Local
            .from_utc_datetime(&utc)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        assert_eq!(ts(&json!("2026-01-02T03:04:05.123Z")), expected);
        assert_eq!(ts(&Value::Null), "-");
        assert_eq!(ts(&json!("not a time")), "not a time");
    }

    #[test]
    fn params_render_as_key_value() {
        assert_eq!(
            params(&json!({"ticket": "ABC-1", "n": 2, "dry": true})),
            "ticket=ABC-1  n=2  dry=true"
        );
    }

    #[test]
    fn placement_shows_settings_sources_warnings_and_notes() {
        let sessions = [
            json!({"name": "tome-1-b", "placement": {
                "layout": "split", "workspace": "project", "direction": "down", "size": "95%",
                "sources": {"layout": "`tome run` flags", "workspace": "the default", "split.direction": "preset `x` (from `tome run` flags)", "split.size": "`tome run` flags"},
                "warnings": ["split.size 95% is outside 10%..90%; clamped to 90%"]}}),
            json!({"name": "old"}),
        ];
        let run = json!({
            "notes": ["workspace: focused: no tmux client is attached; used the project workspace"],
            "caller": {"surface": "S1", "pane": "P1", "workspace": "W1"},
        });
        let out = placement(&run, &sessions);
        assert!(
            out.contains("  caller at start: pane P1 (surface S1) in workspace W1\n"),
            "{out}"
        );
        assert!(out.starts_with("\nplacement:\n  tome-1-b\n"), "{out}");
        assert!(
            out.lines().any(|l| l.split_whitespace().collect::<Vec<_>>()
                == [
                    "split.direction",
                    "down",
                    "preset",
                    "`x`",
                    "(from",
                    "`tome",
                    "run`",
                    "flags)"
                ]),
            "{out}"
        );
        assert!(
            out.contains("    warning: split.size 95% is outside"),
            "{out}"
        );
        assert!(
            out.contains("  note: workspace: focused: no tmux client"),
            "{out}"
        );
        assert!(!out.contains("old"), "{out}");
        assert_eq!(placement(&json!(null), &[json!({"name": "old"})]), "");
    }

    #[test]
    fn cells() {
        assert_eq!(query_cell(&Value::Null), "NULL");
        assert_eq!(query_cell(&json!("-")), "-");
        assert_eq!(size(&json!(17)), "17 B");
        assert_eq!(size(&json!(2048)), "2.0 KB");
    }
}
