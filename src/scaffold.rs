//! `tome workflow new <name>`: write a starter workflow file, and
//! `tome workflow rm <name|path>`: delete one.
//!
//! Both run locally (they only touch a file), though `rm` first asks the daemon
//! whether the file has live runs unless given `--force`. The file lands where `tome run <name>` will find it: the nearest project
//! `.tome/workflows` (created in the working directory if there is none), or
//! `~/.tome/workflows` with `--global`.

use crate::output::{CliError, CliResult, ErrorKind, Report};
use crate::paths;
use crate::rpc;
use crate::workflow::{self, Entry, Library, Scope};
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
    let mut human = format!("created workflow `{name}` at {}\nedit it, then run: tome run {name}", path.display());
    if let Some(g) = &shadows {
        human.push_str(&format!("\nnote: this overrides the global workflow at {}", g.display()));
    }
    Ok(Report::new(json!({ "name": name, "path": path, "scope": scope_str(scope), "shadows": shadows }), human))
}

/// `tome workflow rm <name|path>`: delete one workflow file.
///
/// A name is looked up in one scope only (the project, or `~/.tome/workflows`
/// with `--global`); a path must be a `.md` directly inside a workflows
/// directory, and its location decides the scope. Only the file goes: runs,
/// logs, worktrees and directories are left alone.
///
/// Unless `force` is set, it refuses while the file has running or queued
/// runs, or when the daemon (which alone can tell) isn't running. Live runs
/// carry on from their saved snapshots either way.
pub fn rm(cwd: &Path, target: &str, global: bool, force: bool) -> CliResult<Report> {
    let library = Library::discover(cwd);
    let (scope, path, name) = if target.contains('/') || target.ends_with(".md") {
        by_path(&library, &cwd.join(target), target, global)?
    } else {
        by_name(&library, target, global)?
    };
    if !force {
        refuse_if_live(&path)?;
    }
    fs::remove_file(&path)?;

    // With the project copy gone, `tome run <name>` falls back to a global
    // workflow of the same name, unless another project file still has it.
    let falls_back_to: Option<PathBuf> = match (scope, &name) {
        (Scope::Project, Some(name)) => {
            let project_root = path.parent().and_then(Path::parent).and_then(Path::parent).unwrap_or(cwd);
            let entries = Library::discover(project_root).entries();
            let named = |s: Scope| entries.iter().find(|e| e.scope == s && e.name() == Some(name.as_str()));
            named(Scope::Project).is_none().then(|| named(Scope::Global).map(|e| e.path.clone())).flatten()
        }
        _ => None,
    };
    let mut human = match &name {
        Some(name) => format!("removed workflow `{name}` at {}", path.display()),
        None => format!("removed workflow file {}", path.display()),
    };
    if let (Some(name), Some(g)) = (&name, &falls_back_to) {
        human.push_str(&format!("\nnote: `tome run {name}` now uses the global workflow at {}", g.display()));
    }
    Ok(Report::new(
        json!({ "name": name, "path": path, "scope": scope_str(scope), "falls_back_to": falls_back_to }),
        human,
    ))
}

/// Ask the daemon for the running and queued runs of the file at `path`.
fn refuse_if_live(path: &Path) -> CliResult<()> {
    let data = match rpc::call(&paths::socket_path(), "runs.live", json!({ "workflow_path": path })) {
        Ok(data) => data,
        Err(e) if e.kind == ErrorKind::DaemonNotRunning => {
            return Err(CliError::new(ErrorKind::DaemonNotRunning, "can't check for live runs: daemon is not running")
                .with_hint("start it with `tome daemon start`, or pass --force to remove the file anyway"));
        }
        Err(e) => return Err(e),
    };
    let ids: Vec<i64> = data["runs"].as_array().into_iter().flatten().filter_map(|r| r["id"].as_i64()).collect();
    if ids.is_empty() {
        return Ok(());
    }
    let list: Vec<String> = ids.iter().map(|id| format!("#{id}")).collect();
    Err(CliError::conflict(format!("{} has running or queued runs: {}", path.display(), list.join(", ")))
        .with_hint(format!(
            "cancel them with `tome run cancel {}`, or pass --force to remove the file anyway (they carry on from their saved copy)",
            ids[0]
        ))
        .with_details(json!({ "runs": ids })))
}

fn by_path(library: &Library, path: &Path, target: &str, global: bool) -> CliResult<(Scope, PathBuf, Option<String>)> {
    if !path.is_file() {
        return Err(CliError::not_found(format!("no workflow file at `{target}`")));
    }
    let where_hint = "workflow files are the `.md` files in a project's .tome/workflows or in ~/.tome/workflows";
    let path = path.canonicalize()?;
    let dir = path.parent().unwrap_or(Path::new("/"));
    let in_project_dir =
        dir.file_name().is_some_and(|n| n == "workflows") && dir.parent().and_then(Path::file_name).is_some_and(|n| n == ".tome");
    let scope = if path.extension().is_none_or(|x| x != "md") {
        None
    } else if workflow::same_path(dir, &library.global_dir) {
        Some(Scope::Global)
    } else if in_project_dir {
        Some(Scope::Project)
    } else {
        None
    };
    let Some(scope) = scope else {
        return Err(CliError::invalid(format!("{} is not a workflow file", path.display())).with_hint(where_hint));
    };
    if global && scope == Scope::Project {
        return Err(CliError::invalid(format!("{} is a project workflow, but --global was passed", path.display()))
            .with_hint("drop --global: a path's own location decides its scope"));
    }
    let name = match workflow::load(&path) {
        Ok(wf) => Some(wf.name().to_string()),
        Err(inv) => inv.name,
    };
    Ok((scope, path, name))
}

fn by_name(library: &Library, name: &str, global: bool) -> CliResult<(Scope, PathBuf, Option<String>)> {
    let (scope, other) = if global { (Scope::Global, Scope::Project) } else { (Scope::Project, Scope::Global) };
    let entries = library.entries();
    let matches: Vec<&Entry> = entries.iter().filter(|e| e.scope == scope && e.name() == Some(name)).collect();
    match matches.as_slice() {
        [only] => Ok((scope, only.path.clone(), Some(name.to_string()))),
        [] => {
            let message = format!("no {} workflow named `{name}`", scope_str(scope));
            let hint = if let Some(e) = entries.iter().find(|e| e.scope == other && e.name() == Some(name)) {
                match other {
                    Scope::Global => format!("there's a global one at {}; pass --global to remove it", e.path.display()),
                    Scope::Project => format!("there's a project one at {}; drop --global to remove it", e.path.display()),
                }
            } else if let Some(e) = entries
                .iter()
                .find(|e| e.scope == scope && e.name().is_none() && e.path.file_stem().is_some_and(|s| s == name))
            {
                format!("{} has no readable name; pass its path to remove it", e.path.display())
            } else {
                match (scope, &library.project_dir) {
                    (Scope::Global, _) => format!("global workflows are loaded from {}", library.global_dir.display()),
                    (Scope::Project, Some(dir)) => format!("project workflows are loaded from {}", dir.display()),
                    (Scope::Project, None) => "there's no .tome/workflows directory here or above".to_string(),
                }
            };
            Err(CliError::not_found(message).with_hint(hint))
        }
        many => {
            let paths: Vec<String> = many.iter().map(|e| e.path.display().to_string()).collect();
            Err(CliError::invalid(format!("workflow name `{name}` is defined more than once: {}", paths.join(", ")))
                .with_hint("pass the path of the one to remove"))
        }
    }
}

fn scope_str(scope: Scope) -> &'static str {
    match scope {
        Scope::Global => "global",
        Scope::Project => "project",
    }
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

