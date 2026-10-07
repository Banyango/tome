//! `tome triggers ...`: the client side of triggers.

use crate::output::{exit, table, CliError, CliResult, Report};
use crate::workflow::{Library, Scope};
use crate::{paths, rpc};
use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};

fn call(method: &str, params: Value) -> CliResult<Value> {
    rpc::call(&paths::socket_path(), method, params)
}

fn canonical(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

/// The project a directory is in, if any. Worktrees under `.tome/` aren't
/// projects of their own.
pub fn project_of(cwd: &Path) -> Option<PathBuf> {
    let root = canonical(&Library::discover(cwd).project_root()?);
    (!root
        .components()
        .any(|c| c == Component::Normal(".tome".as_ref())))
    .then_some(root)
}

/// Register the current project with the daemon, if there is one and the
/// daemon is up. Best effort: never fails a command.
pub fn register(cwd: &Path) {
    let socket = paths::socket_path();
    if !socket.exists() {
        return;
    }
    if let Some(root) = project_of(cwd) {
        let _ = rpc::call(&socket, "project.register", json!({ "path": root }));
    }
}

/// `tome triggers fire <wf> [--index N] [--path p]... [--payload text]
/// [--dry-run]`: send a synthetic event down the real firing path. A topic
/// trigger takes its next pending event, or a test event with `--payload`.
pub fn fire(
    cwd: &Path,
    target: &str,
    index: Option<usize>,
    paths: &[String],
    payload: Option<&str>,
    dry_run: bool,
) -> CliResult<Report> {
    let (path, project, root) = match crate::node::target() {
        Some(node) => on_node(node, cwd, target)?,
        None => {
            let library = Library::discover(cwd);
            // An invalid workflow still fires, so the daemon records the error.
            let path = match library.locate(target)? {
                Ok(wf) => wf.path().to_path_buf(),
                Err(inv) => inv.path,
            };
            let project = match library.scope_of(&path) {
                Scope::Project => library.project_root().map(|r| canonical(&r)),
                Scope::Global => None,
            };
            (canonical(&path), project.clone(), project)
        }
    };
    let cwd = canonical(cwd);
    // Paths as a file watcher would report them: relative to the project
    // root, absolute for global workflows.
    let paths: Vec<String> = paths
        .iter()
        .map(|p| {
            let abs = cwd.join(p);
            match &root {
                Some(root) => abs
                    .strip_prefix(root)
                    .map(|r| r.display().to_string())
                    .unwrap_or_else(|_| abs.display().to_string()),
                None => abs.display().to_string(),
            }
        })
        .collect();
    let out = call(
        "triggers.fire",
        json!({
            "workflow_path": path,
            "project_path": project,
            "index": index,
            "paths": paths,
            "payload": payload,
            "dry_run": dry_run,
        }),
    )?;
    let outcome = out["outcome"].as_str().unwrap_or("?");
    let message = out["message"].as_str().unwrap_or("");
    let mut human = if dry_run {
        format!("dry run: {message}")
    } else {
        format!("{outcome}: {message}")
    };
    if dry_run {
        if let Some(params) = out["params"].as_object() {
            for (k, v) in params {
                let v = match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                human.push_str(&format!("\n  {k} = {v}"));
            }
        }
    }
    if let Some(hint) = out["hint"].as_str() {
        human.push_str(&format!("\n  {hint}"));
    }
    let code = if matches!(outcome, "error" | "rejected") {
        exit::FAILURE
    } else {
        exit::OK
    };
    Ok(Report::new(out, human).with_exit(code))
}

/// `triggers fire --on <node>`: the workflow called `target` as the node
/// resolves it, with the node's copy of this project. Returns the
/// workflow's path and project there, and this project's root here (what
/// `--paths` are relative to).
fn on_node(
    node: &crate::node::Node,
    cwd: &Path,
    target: &str,
) -> CliResult<(PathBuf, Option<PathBuf>, Option<PathBuf>)> {
    if target.contains('/') || target.ends_with(".md") {
        return Err(CliError::invalid(format!(
            "`{target}` is a path; with --on, give the workflow's name"
        ))
        .with_hint(format!(
            "the node resolves it: `tome triggers fire <name> --on {}`",
            node.name
        )));
    }
    let local = project_of(cwd);
    let mapped = match &local {
        Some(l) => Some(crate::node::map_project(l)?),
        None => None,
    };
    let out = call(
        "workflow.resolve",
        json!({ "name": target, "project_path": mapped }),
    )?;
    let path = PathBuf::from(out["path"].as_str().unwrap_or_default());
    let project = mapped.filter(|_| out["scope"] == "project");
    Ok((path, project, local))
}

fn project_arg(cwd: &Path, project: Option<PathBuf>) -> CliResult<PathBuf> {
    match project {
        Some(p) => {
            let p = cwd.join(p);
            if !p.is_dir() {
                return Err(CliError::not_found(format!(
                    "no directory at {}",
                    p.display()
                )));
            }
            Ok(canonical(&p))
        }
        None => project_of(cwd).ok_or_else(|| {
            CliError::invalid("not inside a project")
                .with_hint("run this in a project, or pass --project <path>")
        }),
    }
}

/// `tome triggers enable|disable [--project <path>]`
pub fn enable(cwd: &Path, project: Option<PathBuf>, enabled: bool) -> CliResult<Report> {
    let path = crate::node::map_project(&project_arg(cwd, project)?)?;
    let p = call(
        "triggers.enable",
        json!({ "project": path, "enabled": enabled }),
    )?;
    let human = format!(
        "triggers {} for {}{}",
        if enabled { "enabled" } else { "disabled" },
        path.display(),
        crate::node::on_suffix()
    );
    Ok(Report::new(p, human))
}

/// `tome triggers ls`
pub fn ls() -> CliResult<Report> {
    let data = call("triggers.ls", json!({}))?;
    let mut out = String::new();
    let mut section = |title: String, s: &Value| {
        out.push_str(&title);
        out.push('\n');
        let triggers = s["triggers"].as_array().cloned().unwrap_or_default();
        let errors = s["errors"].as_array().cloned().unwrap_or_default();
        if triggers.is_empty() && errors.is_empty() {
            out.push_str("  (no triggers)\n");
        }
        if !triggers.is_empty() {
            let rows = triggers
                .iter()
                .map(|t| {
                    let last = &t["last"];
                    vec![
                        text(&t["workflow"]),
                        text(&t["trigger"]),
                        text(&t["to"]),
                        last["fired_at"].as_str().unwrap_or("never").to_string(),
                        result(last),
                    ]
                })
                .collect();
            for line in table(&["WORKFLOW", "TRIGGER", "TO", "LAST FIRED", "RESULT"], rows).lines()
            {
                out.push_str(&format!("  {line}\n"));
            }
        }
        for t in triggers.iter().filter(|t| t["polling"].is_string()) {
            out.push_str(&format!(
                "  {}: {} is polling: {}\n",
                text(&t["workflow"]),
                text(&t["trigger"]),
                text(&t["polling"])
            ));
        }
        for t in triggers.iter().filter(|t| t["topic"].is_object()) {
            let topic = &t["topic"];
            let mut line = format!(
                "  {}: {} has {} pending",
                text(&t["workflow"]),
                text(&t["trigger"]),
                topic["pending"]
            );
            if let Some(last) = topic["last_event"].as_object() {
                let runs: Vec<String> = last["run_ids"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(text)
                    .collect();
                line.push_str(&format!(
                    "; last took event {} (run {})",
                    last["event_id"],
                    runs.join(", ")
                ));
            }
            out.push_str(&format!("{line}\n"));
        }
        for e in &errors {
            out.push_str(&format!(
                "  {}: not armed: {}\n",
                text(&e["workflow"]),
                text(&e["message"])
            ));
        }
    };
    for p in data["projects"].as_array().cloned().unwrap_or_default() {
        let state = if p["enabled"] == true {
            ""
        } else {
            " (disabled)"
        };
        section(format!("{}{state}", text(&p["path"])), &p);
    }
    section("global".to_string(), &data["global"]);
    Ok(Report::new(data, out.trim_end().to_string()))
}

fn result(last: &Value) -> String {
    if last.is_null() {
        return String::new();
    }
    // A started or queued run's message is its status when it fired, so
    // show what it is now instead (runs gc'd since keep the message).
    let runs = last["runs"].as_array().cloned().unwrap_or_default();
    let live = matches!(last["outcome"].as_str(), Some("started" | "queued")) && !runs.is_empty();
    if live {
        let runs: Vec<String> = runs
            .iter()
            .map(|r| format!("run {} {}", r["id"], text(&r["status"])))
            .collect();
        return format!("{} ({})", text(&last["outcome"]), runs.join(", "));
    }
    match last["message"].as_str() {
        Some(m) => format!("{} ({m})", text(&last["outcome"])),
        None => text(&last["outcome"]),
    }
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}
