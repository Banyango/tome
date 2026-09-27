//! `tome workflow new <name>`: write a starter workflow file.
//!
//! Runs locally (it only writes a file), so it works without the daemon. The
//! file lands where `tome run <name>` will find it: the nearest project
//! `.tome/workflows` (created in the working directory if there is none), or
//! `~/.tome/workflows` with `--global`.

use crate::output::{CliError, CliResult, Report};
use crate::workflow::{self, Library, Scope};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

pub fn new(cwd: &Path, name: &str, description: Option<&str>, global: bool, force: bool) -> CliResult<Report> {
    if !workflow::is_identifier(name) {
        return Err(CliError::invalid(format!("invalid workflow name `{name}`"))
            .with_hint("use letters, digits, `_` and `-`, starting with a letter or `_`"));
    }
    let library = Library::discover(cwd);
    let (scope, dir) = match (global, &library.project_dir) {
        (true, _) => (Scope::Global, library.global_dir.clone()),
        (false, Some(dir)) => (Scope::Project, dir.clone()),
        (false, None) => (Scope::Project, cwd.join(".tome").join("workflows")),
    };
    let path = dir.join(format!("{name}.md"));

    if path.exists() && !force {
        return Err(CliError::invalid(format!("{} already exists", path.display()))
            .with_hint("pass --force to overwrite it"));
    }
    // Another file in the same directory with this name would make
    // `tome run <name>` ambiguous.
    let entries = library.entries();
    if let Some(other) = entries.iter().find(|e| e.scope == scope && e.name() == Some(name) && !same_file(&e.path, &path)) {
        return Err(CliError::invalid(format!("a workflow named `{name}` already exists at {}", other.path.display()))
            .with_hint("pick another name, or edit that file"));
    }

    fs::create_dir_all(&dir)?;
    fs::write(&path, template(name, description))?;
    // The template is ours; if it doesn't validate that's a bug, not user error.
    if let Err(inv) = workflow::load(&path) {
        return Err(CliError::internal(format!("the starter workflow at {} doesn't validate", path.display()))
            .with_details(json!({ "errors": inv.errors })));
    }

    let shadows: Option<PathBuf> = (scope == Scope::Project)
        .then(|| entries.iter().find(|e| e.scope == Scope::Global && e.name() == Some(name)).map(|e| e.path.clone()))
        .flatten();
    let scope_str = match scope {
        Scope::Global => "global",
        Scope::Project => "project",
    };
    let mut human = format!("created workflow `{name}` at {}\nedit it, then run: tome run {name}", path.display());
    if let Some(g) = &shadows {
        human.push_str(&format!("\nnote: this overrides the global workflow at {}", g.display()));
    }
    Ok(Report::new(json!({ "name": name, "path": path, "scope": scope_str, "shadows": shadows }), human))
}

fn template(name: &str, description: Option<&str>) -> String {
    let description = description.unwrap_or("What this workflow does, in one line.");
    format!(
        r#"---
name: {name}
description: {description}
params:
  base: {{type: string, default: main, description: "Branch to start from"}}
# Optional keys:
# defaults:
#   backend: cmux               # where agents run: tmux or cmux (default: cmux inside cmux, else tmux)
#   harness: claude             # agent CLI for the steps (see ~/.tome/config.yaml)
#   orchestrator_harness: claude
#   timeout: 30m
# concurrency: 1                # max simultaneous runs of this workflow
# on_conflict: queue            # (default) wait for a slot, or reject
---
Write the workflow as plain English. The orchestrator reads it and reports
each step with `tome step start|done|fail`. Placeholders like
{{{{params.base}}}} and {{{{run.id}}}} are filled in when the run starts.

## Plan
Work out what needs doing, starting from {{{{params.base}}}}.

## Implement
Make the change.

## Verify
Check the result, then finish the run.
"#,
        description = yaml_scalar(description),
    )
}

/// Quote a free-text value only if YAML would otherwise misread it.
fn yaml_scalar(s: &str) -> String {
    let plain = !s.is_empty()
        && !s.contains(": ")
        && !s.contains(" #")
        && !s.starts_with(|c: char| "!&*{}[]|>'\"%@`#,?:-".contains(c) || c.is_whitespace())
        && !s.ends_with(|c: char| c.is_whitespace() || c == ':')
        && !s.contains('\n');
    if plain {
        s.to_string()
    } else {
        serde_json::to_string(s).expect("strings serialize")
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    a.canonicalize().ok().zip(b.canonicalize().ok()).is_some_and(|(a, b)| a == b)
}

