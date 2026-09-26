//! `tome gc --older-than <age>`: delete finished runs older than `age`, along
//! with their log directories and git worktrees. Nothing is deleted
//! automatically; this only runs when asked.
//!
//! A worktree is only removed through git (`git worktree remove --force`),
//! never by deleting a directory tree directly. If git can't remove one that
//! still exists, the run is kept so the worktree isn't forgotten, and the
//! failure is reported.

use crate::duration;
use crate::output::{CliError, CliResult, Report};
use crate::paths;
use crate::rpc;
use crate::store::{self, Store, Worktree};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

// --- client ----------------------------------------------------------------

pub fn run(older_than: &str, dry_run: bool) -> CliResult<Report> {
    let age = duration::parse(older_than).map_err(CliError::invalid)?;
    let data = rpc::call(
        &paths::socket_path(),
        "runs.gc",
        json!({ "older_than_secs": age.as_secs(), "dry_run": dry_run }),
    )?;

    let deleted = data["deleted"].as_array().cloned().unwrap_or_default();
    let kept = data["kept"].as_array().cloned().unwrap_or_default();
    let mut out = String::new();
    let verb = if dry_run { "would delete" } else { "deleted" };
    for r in &deleted {
        out.push_str(&format!("{verb} run {} ({}, {})\n", r["id"], r["workflow_name"].as_str().unwrap_or("?"), r["status"].as_str().unwrap_or("?")));
        for w in r["worktrees"].as_array().into_iter().flatten() {
            out.push_str(&format!("  worktree {}\n", w["path"].as_str().unwrap_or("?")));
        }
    }
    for r in &kept {
        out.push_str(&format!("kept run {}: {}\n", r["id"], r["error"].as_str().unwrap_or("?")));
    }
    out.push_str(&match (deleted.len(), dry_run) {
        (0, _) => format!("no finished runs older than {older_than}"),
        (n, true) => format!("{n} run{} would be deleted (dry run)", plural(n)),
        (n, false) => format!("{n} run{} deleted", plural(n)),
    });
    let code = if kept.is_empty() { crate::output::exit::OK } else { crate::output::exit::FAILURE };
    Ok(Report::new(data, out).with_exit(code))
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

// --- daemon ----------------------------------------------------------------

/// `runs.gc {older_than_secs, dry_run?}`
pub fn collect(store: &mut Store, p: &Value) -> CliResult<Value> {
    let secs = p
        .get("older_than_secs")
        .and_then(Value::as_u64)
        .ok_or_else(|| CliError::invalid("missing integer `older_than_secs`"))?;
    let dry_run = p.get("dry_run").and_then(Value::as_bool).unwrap_or(false);
    let age = chrono::Duration::try_seconds(secs as i64).ok_or_else(|| CliError::invalid("age is too large"))?;
    let cutoff = store::now().checked_sub_signed(age).unwrap_or(chrono::NaiveDateTime::MIN);

    let internal = |e: anyhow::Error| CliError::internal(format!("{e:#}"));
    let mut deleted = Vec::new();
    let mut kept = Vec::new();
    for run in store.finished_runs_before(cutoff).map_err(internal)? {
        let worktrees = store.worktrees(run.id).map_err(internal)?;
        let entry = json!({
            "id": run.id,
            "workflow_name": run.workflow_name,
            "status": run.status,
            "finished_at": run.finished_at,
            "worktrees": worktrees,
        });
        if dry_run {
            deleted.push(entry);
            continue;
        }
        let errors: Vec<String> = worktrees.iter().filter_map(|w| remove_worktree(w).err()).collect();
        if !errors.is_empty() {
            kept.push(json!({ "id": run.id, "error": errors.join("; ") }));
            continue;
        }
        let dir = store.run_dir(run.id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)
                .map_err(|e| CliError::internal(format!("removing {}: {e}", dir.display())))?;
        }
        store.delete_run(run.id).map_err(internal)?;
        deleted.push(entry);
    }
    Ok(json!({ "dry_run": dry_run, "cutoff": store::fmt_ts(cutoff), "deleted": deleted, "kept": kept }))
}

/// Remove a worktree through git. A worktree that's already gone is fine
/// (git's bookkeeping for it is pruned when the repo is known).
fn remove_worktree(w: &Worktree) -> Result<(), String> {
    let path = Path::new(&w.path);
    let repo = w.repo_path.as_ref().map(PathBuf::from).or_else(|| main_repo_of(path));
    if !path.exists() {
        if let Some(repo) = &repo {
            let _ = git(repo, &["worktree", "prune"]);
        }
        return Ok(());
    }
    let Some(repo) = repo else {
        return Err(format!("{}: not a git worktree, left in place", w.path));
    };
    git(&repo, &["worktree", "remove", "--force", &w.path]).map_err(|e| format!("{}: {e}", w.path))?;
    Ok(())
}

/// The main repository a worktree belongs to (the parent of its common git
/// dir).
fn main_repo_of(worktree: &Path) -> Option<PathBuf> {
    let common = git(worktree, &["rev-parse", "--path-format=absolute", "--git-common-dir"]).ok()?;
    let common = PathBuf::from(common.trim());
    // Only a linked worktree has a `.git` file; refuse to treat a main
    // checkout (or anything else) as removable.
    if !worktree.join(".git").is_file() {
        return None;
    }
    common.parent().map(Path::to_path_buf)
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().map_err(|e| format!("running git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}
