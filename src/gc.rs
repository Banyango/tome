//! `tome gc --older-than <age>`: delete finished runs older than `age`, along
//! with their log directories and git worktrees. Nothing is deleted
//! automatically; this only runs when asked.
//!
//! A resume chain (a run and the runs that resumed it) is collected as one:
//! only once its newest run is finished and older than `age`, and then all
//! of it. A worktree a run adopted is listed under the newest run that had it.
//!
//! A worktree is only removed through git (`git worktree remove --force`),
//! never by deleting a directory tree directly. If git can't remove one that
//! still exists, the run is kept so the worktree isn't forgotten, and the
//! failure is reported.
//!
//! The branch tome made for a worktree (`tome/<run>/<name>`) is deleted with
//! it only if it's merged into the base it was made from; otherwise it's
//! kept and listed, so no work is lost silently.
//!
//! `tome runs rm <id>` removes one run the same way (cancelling it first if
//! it's still live, see `Engine::remove_run`). Only that run goes, even in a
//! resume chain; `--force` deletes its unmerged branches too.

use crate::duration;
use crate::output::{CliError, CliResult, Report};
use crate::paths;
use crate::rpc;
use crate::store::{self, Run, Store, Worktree};
use crate::worktree::{self, git};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

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
        render_run(&mut out, verb, r, dry_run);
    }
    for r in &kept {
        out.push_str(&format!(
            "kept run {}: {}\n",
            r["id"],
            r["error"].as_str().unwrap_or("?")
        ));
    }
    out.push_str(&match (deleted.len(), dry_run) {
        (0, _) => format!("no finished runs older than {older_than}"),
        (n, true) => format!("{n} run{} would be deleted (dry run)", plural(n)),
        (n, false) => format!("{n} run{} deleted", plural(n)),
    });
    let bus = (
        data["bus"]["deliveries"].as_u64().unwrap_or(0),
        data["bus"]["events"].as_u64().unwrap_or(0),
    );
    if bus != (0, 0) {
        let verb = if dry_run { "would remove" } else { "removed" };
        out.push_str(&format!(
            "\n{verb} {} done deliver{} and {} settled event{}",
            bus.0,
            if bus.0 == 1 { "y" } else { "ies" },
            bus.1,
            plural(bus.1 as usize)
        ));
    }
    let code = if kept.is_empty() {
        crate::output::exit::OK
    } else {
        crate::output::exit::FAILURE
    };
    Ok(Report::new(data, out).with_exit(code))
}

/// `tome runs rm <id> [--force] [--dry-run]`
pub fn rm(id: &str, force: bool, dry_run: bool) -> CliResult<Report> {
    let data = rpc::call(
        &paths::socket_path(),
        "runs.rm",
        json!({ "id": id, "force": force, "dry_run": dry_run }),
    )?;
    let run = &data["run"];
    let cancel = data["cancelled"] == true;
    let verb = match (dry_run, cancel) {
        (true, true) => "would cancel and remove",
        (true, false) => "would remove",
        (false, true) => "cancelled and removed",
        (false, false) => "removed",
    };
    let mut out = String::new();
    if let Some(error) = data["error"].as_str() {
        if cancel {
            out.push_str(&format!("cancelled run {}\n", run["id"]));
        }
        out.push_str(&format!("kept run {}: {error}", run["id"]));
        return Ok(Report::new(data, out).with_exit(crate::output::exit::FAILURE));
    }
    render_run(&mut out, verb, run, dry_run);
    if dry_run {
        out.push_str("dry run: nothing removed");
    }
    Ok(Report::new(data, out.trim_end().to_string()))
}

/// One removed (or to-be-removed) run with its worktrees and branches.
fn render_run(out: &mut String, verb: &str, r: &Value, dry_run: bool) {
    out.push_str(&format!(
        "{verb} run {} ({}, {})\n",
        r["id"],
        r["workflow_name"].as_str().unwrap_or("?"),
        r["status"].as_str().unwrap_or("?")
    ));
    for w in r["worktrees"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "  worktree {}\n",
            w["path"].as_str().unwrap_or("?")
        ));
    }
    for b in r["branches"].as_array().into_iter().flatten() {
        let (name, base) = (
            b["branch"].as_str().unwrap_or("?"),
            b["base"].as_str().unwrap_or("?"),
        );
        let why = match b["note"].as_str() {
            Some(note) => format!("{note}, --force"),
            None => format!("merged into {base}"),
        };
        match (b["deleted"].as_bool(), dry_run) {
            (Some(true), true) => out.push_str(&format!("  branch {name} ({why})\n")),
            (Some(true), false) => out.push_str(&format!("  deleted branch {name} ({why})\n")),
            _ => out.push_str(&format!(
                "  kept branch {name}: {}\n",
                b["note"].as_str().unwrap_or("?")
            )),
        }
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

// --- daemon ----------------------------------------------------------------

/// `runs.gc {older_than_secs, dry_run?}`
pub fn collect(store: &mut Store, p: &Value) -> CliResult<Value> {
    let secs = p
        .get("older_than_secs")
        .and_then(Value::as_u64)
        .ok_or_else(|| CliError::invalid("missing integer `older_than_secs`"))?;
    let dry_run = p.get("dry_run").and_then(Value::as_bool).unwrap_or(false);
    let age = chrono::Duration::try_seconds(secs as i64)
        .ok_or_else(|| CliError::invalid("age is too large"))?;
    let cutoff = store::now()
        .checked_sub_signed(age)
        .unwrap_or(chrono::NaiveDateTime::MIN);

    let internal = |e: anyhow::Error| CliError::internal(format!("{e:#}"));
    let mut deleted = Vec::new();
    let mut kept = Vec::new();
    for newest in store.finished_runs_before(cutoff).map_err(internal)? {
        // A resume chain goes as one, once its newest run is old enough.
        if store.resumed_as(newest.id).map_err(internal)?.is_some() {
            continue;
        }
        let chain = chain_of(store, newest).map_err(internal)?;
        let owned = owned_worktrees(store, &chain).map_err(internal)?;
        if dry_run {
            deleted.extend(owned.iter().map(|(run, own)| entry(run, own, false)));
            continue;
        }
        match remove(store, &owned, false)? {
            Ok(entries) => deleted.extend(entries),
            Err(error) => kept.extend(chain.iter().map(|r| json!({ "id": r.id, "error": error }))),
        }
    }
    // Settled deliveries, and events nothing is waiting on any more.
    let (deliveries, events) = store.gc_bus(dry_run)?;
    Ok(json!({
        "dry_run": dry_run,
        "cutoff": store::fmt_ts(cutoff),
        "deleted": deleted,
        "kept": kept,
        "bus": { "deliveries": deliveries, "events": events },
    }))
}

/// `runs.rm {id, force?, dry_run?}` for a finished run (the engine cancels
/// a live one first): remove just that run. A worktree a newer run in its
/// resume chain adopted stays with that run.
pub fn remove_one(store: &mut Store, run: &Run, force: bool, dry_run: bool) -> CliResult<Value> {
    let internal = |e: anyhow::Error| CliError::internal(format!("{e:#}"));
    let mut newer = Vec::new();
    let mut next = store.resumed_as(run.id).map_err(internal)?;
    while let Some(id) = next {
        newer.extend(
            store
                .worktrees(id)
                .map_err(internal)?
                .into_iter()
                .map(|w| w.path),
        );
        next = store.resumed_as(id).map_err(internal)?;
    }
    let own: Vec<Worktree> = store
        .worktrees(run.id)
        .map_err(internal)?
        .into_iter()
        .filter(|w| !newer.contains(&w.path))
        .collect();
    if dry_run {
        return Ok(json!({ "dry_run": true, "run": entry(run, &own, force) }));
    }
    Ok(match remove(store, &[(run, own)], force)? {
        Ok(mut entries) => json!({ "dry_run": false, "run": entries.remove(0) }),
        Err(error) => json!({ "dry_run": false, "run": entry(run, &[], force), "error": error }),
    })
}

/// What removing `run` (with the worktrees it owns) involves.
fn entry(run: &Run, own: &[Worktree], force: bool) -> Value {
    let branches: Vec<Value> = own.iter().filter_map(|w| branch_plan(w, force)).collect();
    json!({
        "id": run.id,
        "workflow_name": run.workflow_name,
        "status": run.status,
        "finished_at": run.finished_at,
        "resumed_from": run.resumed_from,
        "worktrees": own,
        "branches": branches,
    })
}

/// Remove runs with the worktrees each owns: the worktrees through git
/// first, and if any can't be removed, nothing else (the git errors are
/// the `Err`). Then their branches, log directories and records.
fn remove(
    store: &mut Store,
    owned: &[(&Run, Vec<Worktree>)],
    force: bool,
) -> CliResult<Result<Vec<Value>, String>> {
    let errors: Vec<String> = owned
        .iter()
        .flat_map(|(_, own)| own)
        .filter_map(|w| remove_worktree(w).err())
        .collect();
    if !errors.is_empty() {
        return Ok(Err(errors.join("; ")));
    }
    let mut out = Vec::new();
    for (run, own) in owned {
        let mut entry = entry(run, own, force);
        // Worktrees are gone, so their branches can be deleted now.
        entry["branches"] = json!(own
            .iter()
            .filter_map(|w| delete_branch(w, force))
            .collect::<Vec<_>>());
        let dir = store.run_dir(run.id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)
                .map_err(|e| CliError::internal(format!("removing {}: {e}", dir.display())))?;
        }
        store
            .delete_run(run.id)
            .map_err(|e| CliError::internal(format!("{e:#}")))?;
        out.push(entry);
    }
    Ok(Ok(out))
}

/// `newest` and the runs it resumed, oldest first.
fn chain_of(store: &Store, newest: Run) -> anyhow::Result<Vec<Run>> {
    let mut chain = vec![newest];
    while let Some(id) = chain.last().and_then(|r| r.resumed_from) {
        match store.get_run(id, false)? {
            Some(run) => chain.push(run),
            None => break,
        }
    }
    chain.reverse();
    Ok(chain)
}

/// Each run of a chain (oldest first) with the worktrees it's the last to
/// hold: an adopted worktree belongs to the newest run that adopted it.
fn owned_worktrees<'a>(
    store: &Store,
    chain: &'a [Run],
) -> anyhow::Result<Vec<(&'a Run, Vec<Worktree>)>> {
    let mut held: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for run in chain.iter().rev() {
        let own: Vec<Worktree> = store
            .worktrees(run.id)?
            .into_iter()
            .filter(|w| !held.contains(&w.path))
            .collect();
        held.extend(own.iter().map(|w| w.path.clone()));
        out.push((run, own));
    }
    out.reverse();
    Ok(out)
}

/// What gc would do with the branch tome made for a worktree: delete it
/// only if it's merged into its base, or anyway with `force` (the note
/// still says why it wouldn't otherwise be). `None` for worktrees without
/// one (a worktree tome only recorded).
fn branch_plan(w: &Worktree, force: bool) -> Option<Value> {
    let (branch, base) = (w.branch.as_deref()?, w.base.as_deref()?);
    let repo = PathBuf::from(w.repo_path.as_deref()?);
    let (deleted, note) = match worktree::merged(&repo, branch, base) {
        Some(true) => (true, None),
        Some(false) => (false, Some(format!("not merged into {base}"))),
        None => (
            false,
            Some(format!("can't tell whether it's merged into {base}")),
        ),
    };
    let deleted = deleted || force;
    Some(
        json!({ "branch": branch, "base": base, "worker": w.worker, "deleted": deleted, "note": note }),
    )
}

/// Delete a removed worktree's branch if it's merged; report what happened.
fn delete_branch(w: &Worktree, force: bool) -> Option<Value> {
    let mut plan = branch_plan(w, force)?;
    if plan["deleted"] == true {
        let repo = PathBuf::from(w.repo_path.as_deref()?);
        if let Err(e) = git(&repo, &["branch", "-D", w.branch.as_deref()?]) {
            // Already gone is as good as deleted.
            if git(
                &repo,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{}", plan["branch"].as_str()?),
                ],
            )
            .is_ok()
            {
                plan["deleted"] = json!(false);
                plan["note"] = json!(format!("git couldn't delete it: {e}"));
            }
        }
    }
    Some(plan)
}

/// Remove a worktree through git. A worktree that's already gone is fine
/// (git's bookkeeping for it is pruned when the repo is known).
fn remove_worktree(w: &Worktree) -> Result<(), String> {
    let path = Path::new(&w.path);
    let repo = w
        .repo_path
        .as_ref()
        .map(PathBuf::from)
        .or_else(|| main_repo_of(path));
    if !path.exists() {
        if let Some(repo) = &repo {
            let _ = git(repo, &["worktree", "prune"]);
        }
        return Ok(());
    }
    let Some(repo) = repo else {
        return Err(format!("{}: not a git worktree, left in place", w.path));
    };
    git(&repo, &["worktree", "remove", "--force", &w.path])
        .map_err(|e| format!("{}: {e}", w.path))?;
    Ok(())
}

/// The main repository a worktree belongs to (the parent of its common git
/// dir).
fn main_repo_of(worktree: &Path) -> Option<PathBuf> {
    let common = git(
        worktree,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .ok()?;
    let common = PathBuf::from(common.trim());
    // Only a linked worktree has a `.git` file; refuse to treat a main
    // checkout (or anything else) as removable.
    if !worktree.join(".git").is_file() {
        return None;
    }
    common.parent().map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::RunId;
    use crate::store::tests::{new_run, store};
    use crate::store::{NewWorktree, RunStatus};

    fn ids(v: &Value, key: &str) -> Vec<RunId> {
        v[key]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| RunId::new(r["id"].as_i64().unwrap()))
            .collect()
    }

    #[test]
    fn a_resume_chain_goes_once_its_newest_run_does() {
        let (_dir, mut store) = store();
        let first = new_run(&mut store, "build");
        let path = Path::new("/nowhere/.tome/worktrees/1-api");
        let wt = NewWorktree {
            path,
            repo_path: None,
            branch: None,
            base: None,
            worker: None,
        };
        store.add_worktree(first.id, &wt).unwrap();
        store
            .finish_run(first.id, RunStatus::Failed, None, None)
            .unwrap();
        let first = store.get_run(first.id, true).unwrap().unwrap();
        let origin = crate::api::Origin {
            focused: json!({}),
            caller: json!({}),
        };
        let second = crate::resume::create(
            &mut store,
            &first,
            "s",
            None,
            None,
            &origin,
            RunStatus::Running,
        )
        .unwrap();
        // Adopted (the path isn't there, so it's recorded by hand).
        store.add_worktree(second.id, &wt).unwrap();
        let p = json!({ "older_than_secs": 0 });

        let v = collect(&mut store, &p).unwrap();
        assert_eq!(ids(&v, "deleted"), [], "{v}");

        store
            .finish_run(second.id, RunStatus::Failed, None, None)
            .unwrap();
        let v = collect(
            &mut store,
            &json!({ "older_than_secs": 0, "dry_run": true }),
        )
        .unwrap();
        assert_eq!(ids(&v, "deleted"), [first.id, second.id], "{v}");
        // The adopted worktree is the newest run's.
        assert_eq!(v["deleted"][0]["worktrees"], json!([]));
        assert_eq!(
            v["deleted"][1]["worktrees"][0]["path"],
            path.to_str().unwrap()
        );

        let v = collect(&mut store, &p).unwrap();
        assert_eq!(ids(&v, "deleted"), [first.id, second.id]);
        assert!(store.get_run(first.id, false).unwrap().is_none());
        assert!(store.get_run(second.id, false).unwrap().is_none());
    }

    #[test]
    fn rm_takes_one_run_of_a_chain_and_leaves_adopted_worktrees() {
        let (_dir, mut store) = store();
        let first = new_run(&mut store, "build");
        let path = Path::new("/nowhere/.tome/worktrees/1-api");
        let wt = NewWorktree {
            path,
            repo_path: None,
            branch: None,
            base: None,
            worker: None,
        };
        store.add_worktree(first.id, &wt).unwrap();
        store
            .finish_run(first.id, RunStatus::Failed, None, None)
            .unwrap();
        let first = store.get_run(first.id, true).unwrap().unwrap();
        let origin = crate::api::Origin {
            focused: json!({}),
            caller: json!({}),
        };
        let second = crate::resume::create(
            &mut store,
            &first,
            "s",
            None,
            None,
            &origin,
            RunStatus::Running,
        )
        .unwrap();
        store.add_worktree(second.id, &wt).unwrap();

        let v = remove_one(&mut store, &first, false, false).unwrap();
        assert_eq!(v["run"]["worktrees"], json!([]), "{v}");
        assert!(store.get_run(first.id, false).unwrap().is_none());
        assert_eq!(store.worktrees(second.id).unwrap().len(), 1);

        // The newest holder takes the worktree with it.
        let v = remove_one(&mut store, &second, false, false).unwrap();
        assert_eq!(v["run"]["worktrees"][0]["path"], path.to_str().unwrap());
        assert!(store.get_run(second.id, false).unwrap().is_none());
    }
}
