//! `tome run resume <id>`: start a new run that picks up a failed or
//! cancelled one. The new run records the old one as `resumed_from` and
//! uses its workflow snapshot as stored, so params and `{{run.id}}` keep the
//! old run's values. Nothing about the old run changes.

use crate::api::{self, internal, Origin};
use crate::ids::{DeliveryId, RunId};
use crate::output::{CliError, CliResult};
use crate::placement::Settings;
use crate::store::{
    DeliveryState, Group, NewRun, NewWorktree, Run, RunStatus, Session, StepEvent, StepStatus,
    Store, Worker, WorkerStatus, Worktree,
};
use crate::triggers;
use crate::workflow::Mode;
use crate::workflow::{self, Invalid};
use crate::worktree;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// A decoded `run.resume` request.
#[derive(Debug, Clone)]
pub struct ResumeRequest {
    pub id: RunId,
    /// Placement flags; without them the old run's flags are used.
    pub placement: Option<Settings>,
    /// The cmux or herdr pane that ran `tome run resume`.
    pub cmux_caller: Option<Value>,
    /// Cancel an attached run when its caller disconnects.
    pub cancel_on_disconnect: bool,
    /// Where the agent should start, in the user's words (`--start-at`).
    pub start_at: Option<String>,
}

impl ResumeRequest {
    /// `run.resume {id, start_at?, placement?, cmux_caller?, cancel_on_disconnect?}`
    pub fn from_json(p: &Value) -> CliResult<ResumeRequest> {
        Ok(ResumeRequest {
            id: api::req_id(p)?,
            placement: api::opt_placement(p)?,
            cmux_caller: api::opt_object(p, "cmux_caller")?.map(|o| Value::Object(o.clone())),
            cancel_on_disconnect: api::opt_flag(p, "cancel_on_disconnect", true)?,
            start_at: api::opt_string(p, "start_at")?.filter(|s| !s.trim().is_empty()),
        })
    }
}

/// Refuse to resume `run` unless it ended as failed or cancelled, hasn't
/// been resumed already and its project is still there. Every refusal is
/// exit `2`.
pub fn check(store: &Store, run: &Run) -> CliResult<()> {
    let id = run.id;
    match run.status {
        RunStatus::Failed | RunStatus::Cancelled => {}
        RunStatus::Succeeded => {
            return Err(CliError::invalid(format!(
                "run {id} succeeded; only failed or cancelled runs can be resumed"
            ))
            .with_hint("start a fresh run with `tome run <workflow>`"))
        }
        RunStatus::Running | RunStatus::Queued => {
            return Err(CliError::invalid(format!(
                "run {id} is still {}; only failed or cancelled runs can be resumed",
                run.status.as_str()
            ))
            .with_hint(format!(
                "wait for it to end, or cancel it with `tome run cancel {id}`"
            )))
        }
    }
    if let Some(next) = store.resumed_as(id).map_err(internal)? {
        let newest = newest_in_chain(store, next)?;
        return Err(
            CliError::invalid(format!("run {id} was resumed as {next}")).with_hint(format!(
                "if run {newest} failed too, resume it: `tome run resume {newest}`"
            )),
        );
    }
    if let Some(project) = run
        .project_path
        .as_deref()
        .filter(|p| !Path::new(p).is_dir())
    {
        return Err(CliError::invalid(format!(
            "run {id}'s project `{project}` no longer exists"
        )));
    }
    Ok(())
}

/// The last run of the resume chain that `id` is in, from `id` on.
fn newest_in_chain(store: &Store, mut id: RunId) -> CliResult<RunId> {
    while let Some(next) = store.resumed_as(id).map_err(internal)? {
        id = next;
    }
    Ok(id)
}

/// The snapshot the resumed run gets: the old run's, as stored. A run a
/// `while_running: queue` trigger queued keeps its placeholders until it
/// starts, so if it was cancelled before it did (it has no sessions), they're
/// filled in now, from its params, its id and the event recorded with it.
pub fn snapshot(run: &Run, sessions: &[Session]) -> CliResult<String> {
    let stored = run
        .workflow_snapshot
        .as_deref()
        .ok_or_else(|| CliError::internal(format!("run {} has no workflow snapshot", run.id)))?;
    if !(triggers::waits_for_idle(run.trigger.as_ref()) && sessions.is_empty()) {
        return Ok(stored.to_string());
    }
    render_template(run, stored).map_err(Invalid::into_cli_error)
}

/// Fill in the placeholders of a run's unrendered snapshot (see
/// [`Workflow::template_snapshot`](workflow::Workflow::template_snapshot))
/// from its params and the trigger event recorded in its cause.
pub fn render_template(run: &Run, template: &str) -> Result<String, Invalid> {
    let path = PathBuf::from(run.workflow_path.as_deref().unwrap_or_default());
    let wf = workflow::parse(&path, template)?;
    let params = run.params.as_object().cloned().unwrap_or_default();
    let event = run
        .trigger
        .as_ref()
        .and_then(|c| c["event"].as_object())
        .cloned()
        .unwrap_or_default();
    Ok(wf.render_snapshot(&params, &run.id.to_string(), &event))
}

/// Record the run that resumes `old`, with `snapshot` and `old`'s params,
/// project and mode. `flags` are its placement flags; `start_at` is where
/// its agent should start, if the user said.
pub fn create(
    store: &mut Store,
    old: &Run,
    snapshot: &str,
    start_at: Option<&str>,
    flags: Option<&Settings>,
    origin: &Origin,
    status: RunStatus,
) -> CliResult<Run> {
    let params: Map<String, Value> = old.params.as_object().cloned().unwrap_or_default();
    let placement = api::placement_state(flags, Some(origin));
    let run = store
        .create_run(
            NewRun {
                workflow_name: &old.workflow_name,
                workflow_path: old.workflow_path.as_deref().map(Path::new),
                project_path: old.project_path.as_deref().map(Path::new),
                params: &params,
                status,
                trigger: None,
                placement: placement.as_ref(),
                mode: old.mode,
                resumed_from: Some(old.id),
            },
            |_| snapshot.to_string(),
        )
        .map_err(internal)?;
    if let Some(start) = start_at {
        store.set_resume_start(run.id, start).map_err(internal)?;
    }
    adopt_worktrees(store, old.id, run.id).map_err(internal)?;
    reattach_delivery(store, old, run.id)?;
    Ok(run)
}

/// If `old` was started for a topic event whose delivery it left parked
/// (`failed`), hand the delivery to `new`: it's claimed again and settles
/// when `new` ends. One that was retried or removed since is left alone.
fn reattach_delivery(store: &mut Store, old: &Run, new: RunId) -> CliResult<bool> {
    let Some(id) = old
        .trigger
        .as_ref()
        .and_then(|c| c["delivery"].as_i64())
        .map(DeliveryId::new)
    else {
        return Ok(false);
    };
    let ours = store
        .delivery(id)?
        .is_some_and(|d| d.state == DeliveryState::Failed && d.run_ids.contains(&old.id));
    Ok(ours
        && store.move_delivery(
            id,
            DeliveryState::Failed,
            DeliveryState::Claimed,
            Some(&[new]),
        )?)
}

/// Record `old`'s worktrees that are still there under `new`, with the same
/// paths and branches.
fn adopt_worktrees(store: &mut Store, old: RunId, new: RunId) -> anyhow::Result<()> {
    for w in store.worktrees(old)? {
        if !Path::new(&w.path).is_dir() {
            continue;
        }
        store.add_worktree(
            new,
            &NewWorktree {
                path: Path::new(&w.path),
                repo_path: w.repo_path.as_deref().map(Path::new),
                branch: w.branch.as_deref(),
                base: w.base.as_deref(),
                worker: w.worker.as_deref(),
            },
        )?;
    }
    Ok(())
}

/// The worktree an earlier run of `run`'s chain made as `name` in `repo`,
/// adopted by `run`: one made for worker `name` when `for_worker`, else one
/// made with `tome worktree create`.
pub fn adopted(
    store: &Store,
    run: RunId,
    repo: &Path,
    name: &str,
    for_worker: bool,
) -> anyhow::Result<Option<Worktree>> {
    let own = format!("{run}-{name}");
    Ok(store.worktrees(run)?.into_iter().find(|w| {
        let path = Path::new(&w.path);
        worktree::name_of(path) == Some(name)
            && path.file_name().is_some_and(|f| f != own.as_str())
            && w.repo_path.as_deref().map(Path::new) == Some(repo)
            && w.worker.is_some() == for_worker
            && path.is_dir()
    }))
}

/// What a resumed run's agent is told about the runs before it.
#[derive(Debug, Clone, PartialEq)]
pub struct Context {
    /// The run it resumes and the ones that one resumed, oldest first.
    pub earlier: Vec<Earlier>,
    /// Every step reported in the chain, in the order first reported.
    pub steps: Vec<StepSoFar>,
    pub start: Start,
    /// The worktrees it adopted.
    pub worktrees: Vec<Worktree>,
    /// The resumed run's worktrees that were gone, so weren't adopted.
    pub gone: Vec<Worktree>,
    /// The resumed run's workers and groups, for an orchestrator.
    pub workers: Vec<Worker>,
    pub groups: Vec<Group>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Earlier {
    pub id: RunId,
    pub status: RunStatus,
    pub reason: Option<String>,
    pub summary: Option<String>,
}

/// A step's latest outcome across the chain.
#[derive(Debug, Clone, PartialEq)]
pub struct StepSoFar {
    pub name: String,
    pub status: StepStatus,
    /// Its done or fail message.
    pub message: Option<String>,
    /// The run that reported it.
    pub run: RunId,
    /// The failures before the latest outcome: the run and its message.
    pub failed_before: Vec<(RunId, Option<String>)>,
}

/// Where the resumed agent should start.
#[derive(Debug, Clone, PartialEq)]
pub enum Start {
    /// The user's words (`--start-at`), passed on unchecked.
    Given(String),
    /// A step that failed or was still running.
    Redo(String),
    /// The step after this one, the last that finished.
    After(String),
    /// No step was reported.
    Beginning,
}

/// The resume context of `run`, if it resumes another.
pub fn context(store: &Store, run: &Run) -> anyhow::Result<Option<Context>> {
    let mut chain = Vec::new();
    let mut next = run.resumed_from;
    while let Some(id) = next {
        let Some(old) = store.get_run(id, false)? else {
            break;
        };
        next = old.resumed_from;
        chain.push(old);
    }
    if chain.is_empty() {
        return Ok(None);
    }
    chain.reverse();
    let mut steps: Vec<StepSoFar> = Vec::new();
    for old in &chain {
        let mut failures: Vec<(String, Option<String>)> = store
            .step_history(old.id)?
            .into_iter()
            .filter(|h| h.event == StepEvent::Fail)
            .map(|h| (h.step, h.message))
            .collect();
        for step in store.steps(old.id)? {
            // The step's own failures in this run, but not the one that's
            // its latest outcome.
            let mut mine: Vec<(RunId, Option<String>)> = Vec::new();
            failures.retain(|(name, msg)| {
                let hit = *name == step.name;
                if hit {
                    mine.push((old.id, msg.clone()));
                }
                !hit
            });
            if step.status == StepStatus::Failed {
                mine.pop();
            }
            let latest = StepSoFar {
                name: step.name.clone(),
                status: step.status,
                message: step.message,
                run: old.id,
                failed_before: Vec::new(),
            };
            match steps.iter_mut().find(|s| s.name == step.name) {
                Some(seen) => {
                    let mut before = std::mem::take(&mut seen.failed_before);
                    if seen.status != StepStatus::Done {
                        before.push((seen.run, seen.message.clone()));
                    }
                    before.extend(mine);
                    *seen = StepSoFar {
                        failed_before: before,
                        ..latest
                    };
                }
                None => steps.push(StepSoFar {
                    failed_before: mine,
                    ..latest
                }),
            }
        }
    }
    let start = match store.resume_start(run.id)? {
        Some(text) => Start::Given(text),
        None => start_from(&steps),
    };
    let worktrees = store.worktrees(run.id)?;
    let gone = match chain.last() {
        Some(last) => store
            .worktrees(last.id)?
            .into_iter()
            .filter(|w| !worktrees.iter().any(|a| a.path == w.path))
            .collect(),
        None => Vec::new(),
    };
    let (workers, groups) = match chain.last() {
        Some(last) if run.mode == Mode::Orchestrated => {
            (store.workers(last.id)?, store.groups(last.id)?)
        }
        _ => (Vec::new(), Vec::new()),
    };
    let earlier = chain
        .into_iter()
        .map(|r| Earlier {
            id: r.id,
            status: r.status,
            reason: r.reason,
            summary: r.summary,
        })
        .collect();
    Ok(Some(Context {
        earlier,
        steps,
        start,
        worktrees,
        gone,
        workers,
        groups,
    }))
}

/// The first step that didn't finish, else the one after the last that did.
fn start_from(steps: &[StepSoFar]) -> Start {
    if let Some(s) = steps.iter().find(|s| s.status != StepStatus::Done) {
        return Start::Redo(s.name.clone());
    }
    match steps.last() {
        Some(s) => Start::After(s.name.clone()),
        None => Start::Beginning,
    }
}

/// The "Resuming" section of the agent's prompt.
pub fn render(ctx: &Context) -> String {
    let mut out = String::from("## Resuming\n\n");
    if let Some(last) = ctx.earlier.last() {
        out.push_str(&format!(
            "This run picks up run #{}, which {}. Work it and earlier runs did is still in place, so carry on from where they stopped instead of starting over. The workflow below is the one run #{} was given, so where it uses the run id it means #{}, and names made from it (branches, files) are the old run's.\n",
            last.id,
            ended(last),
            ctx.earlier[0].id,
            ctx.earlier[0].id,
        ));
    }
    if ctx.earlier.len() > 1 {
        out.push_str("\nEarlier runs:\n");
        for run in &ctx.earlier {
            out.push_str(&format!("- run #{}: {}\n", run.id, ended(run)));
        }
    } else if let Some(summary) = ctx.earlier.last().and_then(|r| r.summary.as_deref()) {
        out.push_str(&format!("Its summary: {}\n", one_line(summary)));
    }
    if !ctx.steps.is_empty() {
        out.push_str("\nSteps so far:\n");
        for step in &ctx.steps {
            out.push_str(&format!(
                "- `{}`: {} in run #{}",
                step.name,
                step.status.as_str(),
                step.run
            ));
            if let Some(msg) = &step.message {
                out.push_str(&format!(": {}", one_line(msg)));
            }
            out.push('\n');
            for (run, msg) in &step.failed_before {
                out.push_str(&format!("  - failed earlier in run #{run}"));
                if let Some(msg) = msg {
                    out.push_str(&format!(": {}", one_line(msg)));
                }
                out.push('\n');
            }
        }
    }
    if !ctx.worktrees.is_empty() {
        out.push_str("\nWorktrees kept from earlier runs, with their work in them:\n");
        for w in &ctx.worktrees {
            out.push_str(&format!("- {}\n", describe(w)));
        }
        out.push_str("Use them rather than making new ones: `tome worktree create <name>` with the same name returns the same worktree, and a worker spawned with `--worktree` under its old name gets its old one.\n");
    }
    if !ctx.gone.is_empty() {
        out.push_str(
            "\nWorktrees that are gone (deleted since), so their uncommitted work is lost:\n",
        );
        for w in &ctx.gone {
            out.push_str(&format!("- {}\n", describe(w)));
        }
    }
    if let Some(last) = ctx.earlier.last().filter(|_| !ctx.workers.is_empty()) {
        out.push_str(&format!(
            "\nWorkers of run #{} (none of them is running now; spawn again only the ones still needed):\n",
            last.id
        ));
        for w in &ctx.workers {
            out.push_str(&format!("- {}\n", worker(w, last)));
        }
    }
    if !ctx.groups.is_empty() {
        out.push_str("\nIts groups:\n");
        for g in &ctx.groups {
            out.push_str(&format!(
                "- `{}`: {}{}\n",
                g.name,
                g.status,
                if g.fail_fast { " (fail-fast)" } else { "" }
            ));
        }
    }
    out.push_str("\nWhere to start: ");
    out.push_str(&match &ctx.start {
        Start::Given(text) => format!("the user said: {}\n", text.trim()),
        Start::Redo(step) => format!("step `{step}`, which didn't finish.\n"),
        Start::After(step) => {
            format!("the step after `{step}`, the last one that finished.\n")
        }
        Start::Beginning => {
            "the beginning; no step was reported, but check what the earlier run left behind.\n"
                .to_string()
        }
    });
    out.push_str("\nSkip the steps that are done; don't report them again. Before you redo a step that was interrupted, check the state it left behind (files, commits, branches, half-made changes) and build on it.\n");
    out
}

/// A worktree's name (and worker), path and branch.
fn describe(w: &Worktree) -> String {
    let path = Path::new(&w.path);
    let mut out = match (worktree::name_of(path), &w.worker) {
        (Some(name), Some(_)) => format!("`{name}` (worker {name}) at {}", w.path),
        (Some(name), None) => format!("`{name}` at {}", w.path),
        (None, _) => w.path.clone(),
    };
    if let Some(branch) = &w.branch {
        out.push_str(&format!(", branch `{branch}`"));
    }
    out
}

/// A worker's kind, group and how it ended. One the run's end cut off is
/// marked as stopped mid-task.
fn worker(w: &Worker, run: &Earlier) -> String {
    let mut out = format!("`{}` ({}", w.name, w.kind);
    if let Some(group) = &w.group {
        out.push_str(&format!(", group `{group}`"));
    }
    out.push_str(&format!("): {}", w.status.as_str()));
    let cut = match w.status {
        WorkerStatus::Pending | WorkerStatus::Running => true,
        WorkerStatus::Done => false,
        WorkerStatus::Failed | WorkerStatus::Cancelled => {
            w.reason.is_some() && w.reason == run.reason
        }
    };
    if let Some(reason) = &w.reason {
        out.push_str(&format!(" ({reason})"));
    }
    if cut {
        out.push_str(match w.started_at {
            Some(_) => ", stopped mid-task when the run ended",
            None => ", never started",
        });
    }
    if let Some(summary) = &w.summary {
        out.push_str(&format!(": {}", one_line(summary)));
    }
    out
}

/// How a run ended: its status, reason and summary.
fn ended(run: &Earlier) -> String {
    let mut out = run.status.as_str().to_string();
    if let Some(reason) = &run.reason {
        out.push_str(&format!(" ({reason})"));
    }
    if let Some(summary) = &run.summary {
        out.push_str(&format!(": {}", one_line(summary)));
    }
    out
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::ErrorKind;
    use crate::store::tests::{new_run, store};
    use serde_json::json;

    fn origin() -> Origin {
        Origin {
            focused: json!({ "unknown": "test" }),
            caller: json!({ "unknown": "test" }),
        }
    }

    /// A finished run whose project is `project`.
    fn ended(store: &mut Store, status: RunStatus, project: &Path) -> Run {
        let run = new_run(store, "build");
        store.finish_run(run.id, status, Some("why"), None).unwrap();
        let mut run = store.get_run(run.id, true).unwrap().unwrap();
        run.project_path = Some(project.display().to_string());
        run
    }

    #[test]
    fn decodes_requests() {
        let req = ResumeRequest::from_json(&json!({ "id": "#5", "placement": {} })).unwrap();
        assert_eq!(req.id, RunId::new(5));
        assert!(req.placement.is_none());
        assert!(req.cancel_on_disconnect);
        for bad in [
            json!({}),
            json!({ "id": 5, "placement": "tab" }),
            json!({ "id": 5, "cancel_on_disconnect": 1 }),
        ] {
            let err = ResumeRequest::from_json(&bad).unwrap_err();
            assert_eq!(err.kind, ErrorKind::Invalid, "{bad}");
        }
    }

    #[test]
    fn only_failed_or_cancelled_runs_can_be_resumed() {
        let (dir, mut store) = store();
        for status in [RunStatus::Failed, RunStatus::Cancelled] {
            let run = ended(&mut store, status, dir.path());
            check(&store, &run).unwrap();
        }
        let done = ended(&mut store, RunStatus::Succeeded, dir.path());
        let err = check(&store, &done).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Invalid);
        assert!(err.message.contains("succeeded"), "{}", err.message);

        let mut live = new_run(&mut store, "build");
        live.project_path = Some(dir.path().display().to_string());
        let err = check(&store, &live).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Invalid);
        assert!(err.message.contains("still running"), "{}", err.message);
    }

    #[test]
    fn a_run_is_resumed_once_and_the_hint_names_the_newest() {
        let (dir, mut store) = store();
        let first = ended(&mut store, RunStatus::Failed, dir.path());
        let second = create(
            &mut store,
            &first,
            "s",
            None,
            None,
            &origin(),
            RunStatus::Running,
        )
        .unwrap();
        assert_eq!(second.resumed_from, Some(first.id));
        store
            .finish_run(second.id, RunStatus::Failed, None, None)
            .unwrap();
        let second = store.get_run(second.id, true).unwrap().unwrap();
        let third = create(
            &mut store,
            &second,
            "s",
            None,
            None,
            &origin(),
            RunStatus::Running,
        )
        .unwrap();

        let err = check(&store, &first).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Invalid);
        assert_eq!(
            err.message,
            format!("run {} was resumed as {}", first.id, second.id)
        );
        let hint = err.hint.unwrap_or_default();
        assert!(
            hint.contains(&format!("tome run resume {}", third.id)),
            "{hint}"
        );
    }

    #[test]
    fn a_missing_project_is_refused() {
        let (dir, mut store) = store();
        let run = ended(&mut store, RunStatus::Failed, &dir.path().join("gone"));
        let err = check(&store, &run).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Invalid);
        assert!(err.message.contains("no longer exists"), "{}", err.message);
    }

    #[test]
    fn the_resumed_run_keeps_the_snapshot_params_and_mode() {
        let (dir, mut store) = store();
        let old = ended(&mut store, RunStatus::Cancelled, dir.path());
        let snap = snapshot(&old, &[]).unwrap();
        assert_eq!(snap, format!("snapshot for run {}", old.id));
        let new = create(
            &mut store,
            &old,
            &snap,
            None,
            None,
            &origin(),
            RunStatus::Queued,
        )
        .unwrap();
        assert_eq!(new.workflow_snapshot.as_deref(), Some(snap.as_str()));
        assert_eq!(new.params, old.params);
        assert_eq!(new.mode, old.mode);
        assert_eq!(new.status, RunStatus::Queued);
        assert!(new.trigger.is_none());
        assert_eq!(store.resumed_as(old.id).unwrap(), Some(new.id));
    }

    #[test]
    fn an_unstarted_deferred_run_gets_its_placeholders_filled_in() {
        let (_dir, mut store) = store();
        let mut old = new_run(&mut store, "build");
        old.workflow_snapshot = Some(
            "---\nname: build\nparams:\n  base: {}\n---\nRun {{run.id}} on {{params.base}}.\n"
                .into(),
        );
        old.trigger = Some(json!({ "while_running": "queue", "event": {} }));
        let snap = snapshot(&old, &[]).unwrap();
        assert!(snap.contains(&format!("Run {} on main.", old.id)), "{snap}");
    }

    /// Report `step` for `run`: `events` in order, each with its message.
    fn steps(store: &mut Store, run: RunId, step: &str, events: &[(StepEvent, &str)]) {
        for (event, msg) in events {
            let msg = Some(*msg).filter(|m| !m.is_empty());
            store.report_step(run, step, *event, msg).unwrap();
        }
    }

    #[test]
    fn the_context_merges_the_chain_and_starts_at_the_unfinished_step() {
        use StepEvent::{Done, Fail, Start as Begin};
        let (_dir, mut store) = store();
        let first = new_run(&mut store, "build");
        steps(
            &mut store,
            first.id,
            "Setup",
            &[(Begin, ""), (Done, "made dirs")],
        );
        steps(
            &mut store,
            first.id,
            "Build",
            &[(Begin, ""), (Fail, "linker error")],
        );
        store
            .finish_run(
                first.id,
                RunStatus::Failed,
                Some("step_failed"),
                Some("build broke"),
            )
            .unwrap();
        let first = store.get_run(first.id, true).unwrap().unwrap();
        let second = create(
            &mut store,
            &first,
            "s",
            None,
            None,
            &origin(),
            RunStatus::Running,
        )
        .unwrap();
        steps(
            &mut store,
            second.id,
            "Build",
            &[(Begin, ""), (Done, "built")],
        );
        steps(
            &mut store,
            second.id,
            "Test",
            &[(Begin, ""), (Fail, "flaky"), (Begin, "")],
        );
        store
            .abort_run(second.id, RunStatus::Cancelled, "user_cancelled", None)
            .unwrap();
        let second = store.get_run(second.id, true).unwrap().unwrap();
        let third = create(
            &mut store,
            &second,
            "s",
            None,
            None,
            &origin(),
            RunStatus::Running,
        )
        .unwrap();

        let ctx = context(&store, &third).unwrap().unwrap();
        assert_eq!(
            ctx.earlier.iter().map(|r| r.id).collect::<Vec<_>>(),
            [first.id, second.id]
        );
        let names: Vec<_> = ctx.steps.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Setup", "Build", "Test"]);
        let build = &ctx.steps[1];
        assert_eq!((build.status, build.run), (StepStatus::Done, second.id));
        assert_eq!(
            build.failed_before,
            [(first.id, Some("linker error".into()))]
        );
        let test = &ctx.steps[2];
        assert_eq!(test.status, StepStatus::Failed);
        assert_eq!(test.message.as_deref(), Some("user_cancelled"));
        assert_eq!(test.failed_before, [(second.id, Some("flaky".into()))]);
        assert_eq!(ctx.start, Start::Redo("Test".into()));

        let text = render(&ctx);
        assert!(text.starts_with("## Resuming\n"), "{text}");
        for want in [
            format!(
                "picks up run #{}, which cancelled (user_cancelled)",
                second.id
            ),
            format!("run #{}: failed (step_failed): build broke", first.id),
            format!("- `Build`: done in run #{}: built", second.id),
            format!("  - failed earlier in run #{}: linker error", first.id),
            "Where to start: step `Test`".to_string(),
            "Skip the steps that are done".to_string(),
        ] {
            assert!(text.contains(&want), "missing {want:?} in\n{text}");
        }
        // A run that resumes nothing has no context.
        assert!(context(&store, &first).unwrap().is_none());
    }

    #[test]
    fn where_to_start() {
        let step = |name: &str, status| StepSoFar {
            name: name.into(),
            status,
            message: None,
            run: RunId::new(1),
            failed_before: Vec::new(),
        };
        assert_eq!(start_from(&[]), Start::Beginning);
        assert_eq!(
            start_from(&[step("A", StepStatus::Done), step("B", StepStatus::Done)]),
            Start::After("B".into())
        );
        assert_eq!(
            start_from(&[step("A", StepStatus::Running), step("B", StepStatus::Done)]),
            Start::Redo("A".into())
        );
    }

    #[test]
    fn the_users_start_is_passed_on_as_is() {
        let (dir, mut store) = store();
        let old = ended(&mut store, RunStatus::Failed, dir.path());
        let new = create(
            &mut store,
            &old,
            "s",
            Some("redo the migration, then Test"),
            None,
            &origin(),
            RunStatus::Running,
        )
        .unwrap();
        let ctx = context(&store, &new).unwrap().unwrap();
        assert_eq!(
            ctx.start,
            Start::Given("redo the migration, then Test".into())
        );
        assert!(
            render(&ctx).contains("Where to start: the user said: redo the migration, then Test\n")
        );
    }

    #[test]
    fn worktrees_still_there_are_adopted_and_found_by_name() {
        let (dir, mut store) = store();
        let old = ended(&mut store, RunStatus::Failed, dir.path());
        let repo = dir.path();
        let add = |store: &mut Store, name: &str, worker: Option<&str>, make: bool| {
            let path = repo.join(format!(".tome/worktrees/{}-{name}", old.id));
            if make {
                std::fs::create_dir_all(&path).unwrap();
            }
            store
                .add_worktree(
                    old.id,
                    &NewWorktree {
                        path: &path,
                        repo_path: Some(repo),
                        branch: Some(&format!("tome/{}/{name}", old.id)),
                        base: Some("main"),
                        worker,
                    },
                )
                .unwrap();
        };
        add(&mut store, "api", None, true);
        add(&mut store, "w1", Some("w1"), true);
        add(&mut store, "gone", None, false);
        let new = create(
            &mut store,
            &old,
            "s",
            None,
            None,
            &origin(),
            RunStatus::Running,
        )
        .unwrap();

        let ctx = context(&store, &new).unwrap().unwrap();
        let names = |ws: &[Worktree]| -> Vec<String> {
            ws.iter()
                .map(|w| worktree::name_of(Path::new(&w.path)).unwrap().to_string())
                .collect()
        };
        assert_eq!(names(&ctx.worktrees), ["api", "w1"]);
        assert_eq!(names(&ctx.gone), ["gone"]);

        let found = |name, worker| adopted(&store, new.id, repo, name, worker).unwrap();
        assert!(found("api", false).is_some());
        assert!(
            found("api", true).is_none(),
            "made by worktree create, not a worker"
        );
        assert!(found("w1", true).is_some());
        assert!(found("gone", false).is_none());
        assert!(adopted(&store, new.id, &repo.join("other"), "api", false)
            .unwrap()
            .is_none());
    }

    #[test]
    fn an_orchestrator_hears_how_the_old_workers_ended() {
        use crate::store::NewWorker;
        let (dir, mut store) = store();
        let old = new_run(&mut store, "build");
        store.create_group(old.id, "tests", true).unwrap();
        for (name, group) in [("api", Some("tests")), ("ui", None), ("later", None)] {
            let new = NewWorker {
                name: Some(name),
                kind: "agent",
                group,
                harness: None,
                command: None,
                keep_open: false,
            };
            store.reserve_worker(old.id, &new).unwrap();
        }
        store.start_worker(old.id, "api", "s-api").unwrap();
        store
            .finish_worker(
                old.id,
                "api",
                WorkerStatus::Done,
                None,
                Some("api built"),
                None,
                true,
            )
            .unwrap();
        store.start_worker(old.id, "ui", "s-ui").unwrap();
        store
            .abort_run(old.id, RunStatus::Cancelled, "user_cancelled", None)
            .unwrap();
        let mut old = store.get_run(old.id, true).unwrap().unwrap();
        old.project_path = Some(dir.path().display().to_string());
        let new = create(
            &mut store,
            &old,
            "s",
            None,
            None,
            &origin(),
            RunStatus::Running,
        )
        .unwrap();

        let ctx = context(&store, &new).unwrap().unwrap();
        assert_eq!(ctx.workers.len(), 3);
        let text = render(&ctx);
        for want in [
            format!("Workers of run #{}", old.id),
            "- `api` (agent, group `tests`): done: api built".to_string(),
            "- `ui` (agent): cancelled (user_cancelled), stopped mid-task".to_string(),
            "- `tests`: ".to_string(),
            "- `later` (agent): cancelled (user_cancelled), never started".to_string(),
        ] {
            assert!(text.contains(&want), "missing {want:?} in\n{text}");
        }

        // A single agent has no workers to hear about.
        let mut single = new.clone();
        single.mode = Mode::Single;
        let ctx = context(&store, &single).unwrap().unwrap();
        assert!(ctx.workers.is_empty() && ctx.groups.is_empty());
    }
}
