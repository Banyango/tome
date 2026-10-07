//! `tome run resume <id>`: start a new run that picks up a failed or
//! cancelled one. The new run records the old one as `resumed_from` and
//! uses its workflow snapshot as stored, so params and `{{run.id}}` keep the
//! old run's values. Nothing about the old run changes.

use crate::api::{self, internal, Origin};
use crate::ids::RunId;
use crate::output::{CliError, CliResult};
use crate::placement::Settings;
use crate::store::{NewRun, Run, RunStatus, Session, Store};
use crate::triggers;
use crate::workflow::{self, Invalid};
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
}

impl ResumeRequest {
    /// `run.resume {id, placement?, cmux_caller?, cancel_on_disconnect?}`
    pub fn from_json(p: &Value) -> CliResult<ResumeRequest> {
        Ok(ResumeRequest {
            id: api::req_id(p)?,
            placement: api::opt_placement(p)?,
            cmux_caller: api::opt_object(p, "cmux_caller")?.map(|o| Value::Object(o.clone())),
            cancel_on_disconnect: api::opt_flag(p, "cancel_on_disconnect", true)?,
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
/// project and mode. `flags` are its placement flags.
pub fn create(
    store: &mut Store,
    old: &Run,
    snapshot: &str,
    flags: Option<&Settings>,
    origin: &Origin,
    status: RunStatus,
) -> CliResult<Run> {
    let params: Map<String, Value> = old.params.as_object().cloned().unwrap_or_default();
    let placement = api::placement_state(flags, Some(origin));
    store
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
        .map_err(internal)
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
        let second = create(&mut store, &first, "s", None, &origin(), RunStatus::Running).unwrap();
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
        let new = create(&mut store, &old, &snap, None, &origin(), RunStatus::Queued).unwrap();
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
}
