//! `tome triggers ...`: the client side of triggers.

use crate::output::{exit, CliResult, Report};
use crate::workflow::{Library, Scope};
use crate::{paths, rpc};
use serde_json::{json, Value};
use std::path::Path;

fn call(method: &str, params: Value) -> CliResult<Value> {
    rpc::call(&paths::socket_path(), method, params)
}

/// `tome triggers fire <wf> [--index N] [--path p]... [--dry-run]`: send a
/// synthetic event down the real firing path.
pub fn fire(cwd: &Path, target: &str, index: Option<usize>, paths: &[String], dry_run: bool) -> CliResult<Report> {
    let library = Library::discover(cwd);
    // An invalid workflow still fires, so the daemon records the error.
    let path = match library.locate(target)? {
        Ok(wf) => wf.path,
        Err(inv) => inv.path,
    };
    let project = match library.scope_of(&path) {
        Scope::Project => library.project_root(),
        Scope::Global => None,
    };
    // Paths as a file watcher would report them: relative to the project
    // root, absolute for global workflows.
    let paths: Vec<String> = paths
        .iter()
        .map(|p| {
            let abs = cwd.join(p);
            match &project {
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
            "dry_run": dry_run,
        }),
    )?;
    let outcome = out["outcome"].as_str().unwrap_or("?");
    let message = out["message"].as_str().unwrap_or("");
    let mut human = if dry_run { format!("dry run: {message}") } else { format!("{outcome}: {message}") };
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
    let code = if matches!(outcome, "error" | "rejected") { exit::FAILURE } else { exit::OK };
    Ok(Report::new(out, human).with_exit(code))
}
