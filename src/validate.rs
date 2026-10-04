//! `tome validate [workflow] [--param k=v]...`
//!
//! Runs locally (it only reads workflow files), so it works without the
//! daemon. Exits `2` if anything is invalid.

use crate::output::{exit, CliResult, Report};
use crate::placement;
use crate::workflow::{self, Entry, Invalid, Library, Scope, Workflow};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;

pub fn run(cwd: &Path, target: Option<&str>, params: &[String]) -> CliResult<Report> {
    let library = Library::discover(cwd);
    let overrides = workflow::parse_param_args(params)?;

    let mut results = Vec::new();
    match target {
        Some(target) => match library.locate(target)? {
            Ok(wf) => {
                let check = wf.resolve_params(&overrides, !overrides.is_empty()).err();
                let mut r = result_json(wf.name(), &wf.path, None, check.as_ref(), None);
                r["warnings"] = warnings_json(&wf, cwd);
                results.push(r);
            }
            Err(inv) => results.push(result_json(
                inv.name.as_deref().unwrap_or(""),
                &inv.path,
                None,
                Some(&inv),
                None,
            )),
        },
        None => {
            let entries = library.entries();
            let duplicates = duplicate_names(&entries);
            for entry in &entries {
                let overridden_by = match entry.scope {
                    Scope::Global => entry.name().and_then(|n| {
                        entries
                            .iter()
                            .find(|e| e.scope == Scope::Project && e.name() == Some(n))
                            .map(|e| &e.path)
                    }),
                    Scope::Project => None,
                };
                let mut invalid = entry.result.as_ref().err().cloned();
                if let Some(name) = entry.name() {
                    if duplicates.contains_key(&(entry.scope, name.to_string())) {
                        let err = workflow::Diagnostic {
                            line: 1,
                            message: format!("workflow name `{name}` is defined more than once in the same directory"),
                        };
                        match invalid.as_mut() {
                            Some(inv) => inv.errors.insert(0, err),
                            None => {
                                invalid = Some(Invalid {
                                    path: entry.path.clone(),
                                    name: Some(name.into()),
                                    errors: vec![err],
                                })
                            }
                        }
                    }
                }
                let mut r = result_json(
                    entry.name().unwrap_or(""),
                    &entry.path,
                    Some(entry.scope),
                    invalid.as_ref(),
                    overridden_by.map(|p| p.as_path()),
                );
                if let Ok(wf) = &entry.result {
                    r["warnings"] = warnings_json(wf, cwd);
                }
                results.push(r);
            }
        }
    }

    // The project's forwarding rules, when checking everything.
    let bus = target
        .is_none()
        .then(|| crate::triggerscmd::project_of(cwd))
        .flatten()
        .map(|root| crate::bus::forward::check(&root));
    let bus_ok = bus.as_ref().is_none_or(|(errors, _)| errors.is_empty());
    let valid = results.iter().all(|r| r["valid"] == true) && bus_ok;
    let mut human = render_human(&results, target.is_none());
    let mut data = json!({ "valid": valid, "workflows": results });
    if let Some((errors, warnings)) = bus {
        for e in &errors {
            human.push_str(&format!("invalid bus.forward: {e}\n"));
        }
        for w in &warnings {
            human.push_str(&format!("warning: {w}\n"));
        }
        data["bus"] = json!({ "errors": errors, "warnings": warnings });
    }
    Ok(Report::new(data, human).with_exit(if valid { exit::OK } else { exit::INVALID }))
}

fn duplicate_names(entries: &[Entry]) -> HashMap<(Scope, String), usize> {
    let mut counts: HashMap<(Scope, String), usize> = HashMap::new();
    for e in entries {
        if let Some(n) = e.name() {
            *counts.entry((e.scope, n.to_string())).or_default() += 1;
        }
    }
    counts.retain(|_, n| *n > 1);
    counts
}

fn result_json(
    name: &str,
    path: &Path,
    scope: Option<Scope>,
    invalid: Option<&Invalid>,
    overridden_by: Option<&Path>,
) -> Value {
    let mut v = json!({
        "name": name,
        "path": path,
        "valid": invalid.is_none(),
        "errors": invalid.map(Invalid::errors_json).unwrap_or_else(|| json!([])),
        "warnings": [],
    });
    if let Some(scope) = scope {
        v["scope"] = json!(scope);
    }
    if let Some(p) = overridden_by {
        v["overridden_by"] = json!(p);
    }
    v
}

fn warnings_json(wf: &Workflow, cwd: &Path) -> Value {
    let mut all: Vec<Value> = wf
        .warnings()
        .iter()
        .map(|d| json!({ "line": d.line, "message": d.message }))
        .collect();
    // Presets depend on the environment, so a missing one only warns.
    for name in placement::undefined_presets(wf.frontmatter.defaults.layout.as_ref(), Some(cwd)) {
        let line = preset_line(wf, &name);
        all.push(json!({
            "line": line,
            "message": format!("layout preset `{name}` isn't defined here (known: built-in presets and `layout_presets` in the tome config)"),
        }));
    }
    json!(all)
}

/// The file line naming a preset, or the frontmatter's first.
fn preset_line(wf: &Workflow, name: &str) -> usize {
    wf.source
        .lines()
        .position(|l| {
            let l = l.trim_start().trim_start_matches("- ");
            l.contains("preset:")
                && l.split("preset:")
                    .nth(1)
                    .is_some_and(|v| v.trim().trim_matches(['"', '\'', '}', ',', ' ']) == name)
        })
        .map(|i| i + 1)
        .unwrap_or(1)
}

fn render_human(results: &[Value], listing: bool) -> String {
    if results.is_empty() {
        return "no workflows found".into();
    }
    let mut out = String::new();
    for r in results {
        let name = r["name"]
            .as_str()
            .filter(|n| !n.is_empty())
            .unwrap_or("<unnamed>");
        let path = r["path"].as_str().unwrap_or("");
        let status = if r["valid"] == true { "ok" } else { "invalid" };
        let scope = r["scope"]
            .as_str()
            .map(|s| format!(" [{s}]"))
            .unwrap_or_default();
        out.push_str(&format!("{status:<8}{name}{scope}  {path}\n"));
        if let Some(by) = r["overridden_by"].as_str() {
            out.push_str(&format!("        overridden by {by}\n"));
        }
        // The path is on the header line; errors only need the line number.
        for e in r["errors"].as_array().into_iter().flatten() {
            out.push_str(&format!(
                "        line {}: {}\n",
                e["line"],
                e["message"].as_str().unwrap_or("")
            ));
        }
        for w in r["warnings"].as_array().into_iter().flatten() {
            out.push_str(&format!(
                "        warning: line {}: {}\n",
                w["line"],
                w["message"].as_str().unwrap_or("")
            ));
        }
    }
    if listing {
        let bad = results.iter().filter(|r| r["valid"] != true).count();
        out.push_str(&format!("{} workflow(s), {bad} invalid\n", results.len()));
    }
    out
}
