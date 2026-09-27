//! Triggers, daemon side: the one firing path every event source (cron,
//! file changes, `tome triggers fire`) goes down, and the RPCs behind
//! `tome triggers`.
//!
//! A fire loads the workflow fresh, applies the `to:` rules and muting, and
//! then starts a detached run through the normal concurrency rules. Every
//! fire's outcome is recorded.

use crate::api::{opt_str, req_str};
use crate::arming;
use crate::engine::Engine;
use crate::output::{CliError, CliResult, ErrorKind};
use crate::store::{NewFire, Run, RunStatus};
use crate::{orchestrator, session};
use crate::workflow::{self, FileEvent, Scope, Target, TriggerKind, Workflow};
use chrono::{DateTime, Local, SecondsFormat};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

pub const METHODS: &[&str] = &["triggers.fire", "triggers.ls", "triggers.enable", "project.register"];

/// The queue signals are delivered on, and who they're from.
pub const QUEUE: &str = "events";
const SENDER: &str = "trigger";

/// Fire outcomes, as recorded.
pub mod outcome {
    pub const STARTED: &str = "started";
    pub const SIGNALLED: &str = "signalled";
    pub const NO_TARGET: &str = "no_target";
    pub const MUTED: &str = "muted";
    pub const REJECTED: &str = "rejected";
    pub const ERROR: &str = "error";
}

/// An event that fired a trigger.
#[derive(Debug, Clone, Default)]
pub struct Event {
    /// Changed paths with what happened to them (file triggers).
    pub paths: Vec<(String, FileEvent)>,
    /// When the cron schedule said to fire.
    pub scheduled: Option<DateTime<Local>>,
    /// Sent by `tome triggers fire` rather than a real source.
    pub synthetic: bool,
}

/// One trigger of one workflow, where it's armed.
#[derive(Debug, Clone)]
pub struct FireRequest {
    pub workflow_path: PathBuf,
    /// The project root, for a project workflow.
    pub project: Option<PathBuf>,
    pub index: usize,
    pub event: Event,
    pub dry_run: bool,
}

/// The result of a fire, as returned to `tome triggers fire`.
pub struct Fired {
    pub outcome: &'static str,
    pub message: Option<String>,
    pub runs: Vec<i64>,
    /// Extra detail (resolved params, the run started, ...).
    pub data: Value,
}

fn local_time(t: DateTime<Local>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, false)
}

/// The `{{trigger.*}}` fields of an event.
pub fn fields(kind: &TriggerKind, event: &Event, now: DateTime<Local>) -> Map<String, Value> {
    let (kind_name, what) = match kind {
        TriggerKind::Manual => ("manual", String::new()),
        TriggerKind::Cron { .. } => ("cron", "scheduled".to_string()),
        TriggerKind::File(_) => {
            let mut seen: Vec<&str> = Vec::new();
            for (_, e) in &event.paths {
                if !seen.contains(&e.as_str()) {
                    seen.push(e.as_str());
                }
            }
            ("file", seen.join(", "))
        }
    };
    let paths: Vec<Value> = event.paths.iter().map(|(p, e)| json!({ "path": p, "event": e.as_str() })).collect();
    let mut m = Map::new();
    m.insert("kind".into(), json!(kind_name));
    m.insert("event".into(), json!(what));
    m.insert("paths".into(), Value::Array(paths));
    m.insert("time".into(), json!(local_time(now)));
    m.insert("scheduled".into(), json!(event.scheduled.map(local_time).unwrap_or_default()));
    m
}

/// Load a workflow as it's armed: a project workflow, or a global one
/// (which needs absolute file globs).
pub fn load(path: &Path, project: Option<&Path>) -> Result<Workflow, workflow::Invalid> {
    let scope = if project.is_some() { Scope::Project } else { Scope::Global };
    workflow::load(path).and_then(|wf| workflow::check_scope(wf, scope))
}

pub fn first_errors(inv: &workflow::Invalid) -> String {
    let n = inv.errors.len();
    let first = inv.errors.first().map(|d| format!("line {}: {}", d.line, d.message)).unwrap_or_default();
    if n > 1 {
        format!("workflow is invalid: {first} (and {} more)", n - 1)
    } else {
        format!("workflow is invalid: {first}")
    }
}

/// Whether two paths name the same file.
pub fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

impl Engine {
    pub(crate) fn dispatch_triggers(&self, method: &str, p: &Value) -> CliResult<Value> {
        match method {
            "triggers.fire" => self.fire_rpc(p),
            "triggers.ls" => self.ls_rpc(),
            "triggers.enable" => {
                let path = req_str(p, "project")?;
                let enabled = p["enabled"].as_bool().ok_or_else(|| CliError::invalid("`enabled` must be a boolean"))?;
                Ok(json!(self.with_store(|store| store.set_project_enabled(path, enabled))?))
            }
            "project.register" => {
                let path = req_str(p, "path")?;
                let new = self.with_store(|store| store.register_project(path))?;
                if new {
                    eprintln!("tome daemon: registered project {path}");
                }
                Ok(json!({ "path": path, "new": new }))
            }
            _ => Err(CliError::invalid(format!("unknown method `{method}`"))),
        }
    }

    /// `triggers.ls`: each registered project's armed triggers (and the
    /// global ones) with their last fire, plus workflows whose triggers
    /// can't be armed.
    fn ls_rpc(&self) -> CliResult<Value> {
        self.live_projects();
        let projects = self.with_store(|store| store.projects())?;
        let roots: Vec<PathBuf> = projects.iter().map(|p| PathBuf::from(&p.path)).collect();
        let scan = arming::scan(&roots);
        let fires = self.with_store(|store| store.last_fires())?;
        let last = |path: &Path, project: Option<&Path>, index: i64| {
            let (path, project) = (path.display().to_string(), project.map(|p| p.display().to_string()));
            fires.iter().find(|f| f.workflow_path == path && f.project_path == project && f.trigger_index == index).cloned()
        };
        let section = |project: Option<&Path>| {
            let triggers: Vec<Value> = scan
                .armed
                .iter()
                .filter(|a| a.project.as_deref() == project)
                .map(|a| {
                    json!({
                        "workflow": a.name,
                        "workflow_path": a.workflow_path,
                        "index": a.index,
                        "kind": a.trigger.kind_name(),
                        "trigger": a.trigger.describe(),
                        "to": a.trigger.effective_target().as_str(),
                        "last": last(&a.workflow_path, project, a.index as i64),
                    })
                })
                .collect();
            let errors: Vec<Value> = scan
                .broken
                .iter()
                .filter(|b| b.project.as_deref() == project)
                .map(|b| {
                    json!({
                        "workflow": b.name,
                        "workflow_path": b.workflow_path,
                        "message": b.message,
                        "last": last(&b.workflow_path, project, arming::WHOLE),
                    })
                })
                .collect();
            (triggers, errors)
        };
        let projects: Vec<Value> = projects
            .iter()
            .map(|p| {
                let (triggers, errors) = section(Some(Path::new(&p.path)));
                json!({ "path": p.path, "enabled": p.enabled, "triggers": triggers, "errors": errors })
            })
            .collect();
        let (triggers, errors) = section(None);
        Ok(json!({ "projects": projects, "global": { "triggers": triggers, "errors": errors } }))
    }

    /// Queued and running runs of the workflow at `path`.
    pub(crate) fn active_runs(&self, name: &str, path: &Path) -> CliResult<Vec<Run>> {
        let runs = self.with_store(|store| store.in_progress_runs().map_err(crate::api::internal))?;
        Ok(runs
            .into_iter()
            .filter(|r| r.workflow_name == name && r.workflow_path.as_deref().is_some_and(|p| same_file(Path::new(p), path)))
            .collect())
    }

    /// `triggers.fire {workflow_path, project_path?, index?, paths?, dry_run?}`:
    /// a synthetic event down the real firing path.
    fn fire_rpc(&self, p: &Value) -> CliResult<Value> {
        let workflow_path = PathBuf::from(req_str(p, "workflow_path")?);
        let project = opt_str(p, "project_path").map(PathBuf::from);
        let paths: Vec<String> =
            p["paths"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_string)).collect();
        let index = match p.get("index").and_then(Value::as_u64) {
            Some(i) => i as usize,
            None => {
                // The first real trigger, so `tome triggers fire <wf>` does the obvious thing.
                match load(&workflow_path, project.as_deref()) {
                    Ok(wf) => wf.frontmatter.triggers.iter().position(|t| !matches!(t.kind, TriggerKind::Manual)).unwrap_or(0),
                    Err(_) => 0,
                }
            }
        };
        let event_for = |kind: Option<&TriggerKind>| {
            let on = match kind {
                Some(TriggerKind::File(f)) if !f.on.contains(&FileEvent::Modified) => FileEvent::Created,
                _ => FileEvent::Modified,
            };
            Event {
                paths: paths.iter().map(|p| (p.clone(), on)).collect(),
                scheduled: matches!(kind, Some(TriggerKind::Cron { .. })).then(Local::now),
                synthetic: true,
            }
        };
        let kind = load(&workflow_path, project.as_deref()).ok().and_then(|wf| wf.frontmatter.triggers.get(index).map(|t| t.kind.clone()));
        let req = FireRequest {
            workflow_path,
            project,
            index,
            event: event_for(kind.as_ref()),
            dry_run: p["dry_run"] == true,
        };
        let fired = self.fire(&req);
        let mut data = json!({
            "outcome": fired.outcome,
            "message": fired.message,
            "run_ids": fired.runs,
            "dry_run": req.dry_run,
            "index": req.index,
        });
        if let (Value::Object(extra), Value::Object(out)) = (&fired.data, &mut data) {
            out.extend(extra.clone());
        }
        Ok(data)
    }

    /// Fire one trigger: the single path every event source uses. The
    /// outcome is recorded unless it's a dry run.
    pub fn fire(&self, req: &FireRequest) -> Fired {
        let wf = load(&req.workflow_path, req.project.as_deref());
        let name = match &wf {
            Ok(wf) => wf.name().to_string(),
            Err(inv) => inv.name.clone().unwrap_or_else(|| req.workflow_path.display().to_string()),
        };
        let trigger_desc = wf
            .as_ref()
            .ok()
            .and_then(|wf| wf.frontmatter.triggers.get(req.index))
            .map(|t| t.describe())
            .unwrap_or_else(|| format!("trigger {}", req.index));
        let fired = match wf {
            Ok(wf) => self.fire_workflow(&wf, req),
            Err(inv) => Fired { outcome: outcome::ERROR, message: Some(first_errors(&inv)), runs: Vec::new(), data: json!({}) },
        };
        if !req.dry_run {
            let recorded = self.with_store(|store| {
                store.record_fire(&NewFire {
                    workflow_path: &req.workflow_path.display().to_string(),
                    workflow_name: &name,
                    project_path: req.project.as_ref().map(|p| p.display().to_string()).as_deref(),
                    trigger_index: req.index as i64,
                    trigger: &trigger_desc,
                    outcome: fired.outcome,
                    message: fired.message.as_deref(),
                    run_ids: &fired.runs,
                })
            });
            if let Err(e) = recorded {
                eprintln!("tome daemon: recording a trigger fire failed: {e}");
            }
            eprintln!(
                "tome daemon: trigger {trigger_desc} of {name} fired: {}{}",
                fired.outcome,
                fired.message.as_deref().map(|m| format!(" ({m})")).unwrap_or_default()
            );
            // A rejection under `on_conflict: reject` is configured
            // behaviour: recorded, not notified.
            if fired.outcome == outcome::ERROR {
                self.trigger_failed(&name, fired.message.as_deref().unwrap_or("unknown error"));
            }
        }
        fired
    }

    fn fire_workflow(&self, wf: &Workflow, req: &FireRequest) -> Fired {
        let err = |outcome: &'static str, message: String| Fired { outcome, message: Some(message), runs: Vec::new(), data: json!({}) };
        let Some(trigger) = wf.frontmatter.triggers.get(req.index) else {
            let n = wf.frontmatter.triggers.len();
            return err(outcome::ERROR, format!("workflow `{}` has no trigger at index {} ({n} trigger{})", wf.name(), req.index, if n == 1 { "" } else { "s" }));
        };
        let active = match self.active_runs(wf.name(), &wf.path) {
            Ok(runs) => runs,
            Err(e) => return err(outcome::ERROR, e.message),
        };
        let active_ids: Vec<i64> = active.iter().map(|r| r.id).collect();
        if matches!(trigger.kind, TriggerKind::File(_)) && !active.is_empty() {
            return Fired {
                outcome: outcome::MUTED,
                message: Some(format!("muted while run {} is active", join_ids(&active_ids))),
                runs: Vec::new(),
                data: json!({ "active": active_ids }),
            };
        }
        match trigger.effective_target() {
            Target::Running if active.is_empty() => {
                Fired { outcome: outcome::NO_TARGET, message: Some("no active run to signal".into()), runs: Vec::new(), data: json!({}) }
            }
            Target::Running | Target::RunningOrNew if !active.is_empty() => self.signal(trigger, req, &active),
            _ => self.fire_new(wf, trigger, req),
        }
    }

    /// Deliver a fire to active runs: JSON onto each run's `events` queue, a
    /// line in its event stream, and a nudge typed into its orchestrator's
    /// pane (dropped if the pane is gone).
    fn signal(&self, trigger: &workflow::Trigger, req: &FireRequest, active: &[Run]) -> Fired {
        let fields = fields(&trigger.kind, &req.event, Local::now());
        let ids: Vec<i64> = active.iter().map(|r| r.id).collect();
        if req.dry_run {
            return Fired {
                outcome: outcome::SIGNALLED,
                message: Some(format!("would signal run {}", join_ids(&ids))),
                runs: Vec::new(),
                data: json!({ "action": "signal", "runs": ids, "trigger": fields }),
            };
        }
        let mut body = json!({
            "type": "trigger",
            "trigger": trigger.describe(),
            "index": req.index,
            "synthetic": req.event.synthetic,
            "params": trigger.params,
        });
        if let Value::Object(b) = &mut body {
            b.extend(fields.clone());
        }
        let body = body.to_string();
        let what = describe_event(trigger, &fields);
        let mut signalled = Vec::new();
        let mut errors = Vec::new();
        for run in active {
            let pushed = self.with_store(|store| {
                store.push_message(run.id, QUEUE, &body, SENDER)?;
                store.worker_event(run.id, None, None, "trigger", Some(&what))
            });
            match pushed {
                Ok(()) => signalled.push(run.id),
                Err(e) => errors.push(format!("run {}: {}", run.id, e.message)),
            }
            if run.status == RunStatus::Running {
                let nudge = format!("[tome] trigger {} fired. Details: tome queue pull {QUEUE}", trigger.kind_name());
                for s in self.recorded_sessions(run.id).iter().filter(|s| s.role == orchestrator::ROLE) {
                    session::send_line(s, &nudge);
                }
            }
        }
        if signalled.is_empty() {
            return Fired { outcome: outcome::ERROR, message: Some(errors.join("; ")), runs: Vec::new(), data: json!({}) };
        }
        let mut message = format!("signalled run {}", join_ids(&signalled));
        if !errors.is_empty() {
            message.push_str(&format!(" (failed: {})", errors.join("; ")));
        }
        Fired { outcome: outcome::SIGNALLED, message: Some(message), runs: signalled, data: json!({ "action": "signal" }) }
    }

    /// Start a detached run for a fired trigger.
    fn fire_new(&self, wf: &Workflow, trigger: &workflow::Trigger, req: &FireRequest) -> Fired {
        let now = Local::now();
        let fields = fields(&trigger.kind, &req.event, now);
        let params: Vec<String> = trigger.params.iter().map(|(k, v)| format!("{k}={}", text(v))).collect();
        let cause = json!({
            "kind": trigger.kind_name(),
            "trigger": trigger.describe(),
            "index": req.index,
            "project": req.project,
            "synthetic": req.event.synthetic,
            "event": fields,
        });
        if req.dry_run {
            let overrides = workflow::parse_param_args(&params).unwrap_or_default();
            return match wf.resolve_params(&overrides, true) {
                Ok(resolved) => Fired {
                    outcome: outcome::STARTED,
                    message: Some(format!("would start a new run of {}", wf.name())),
                    runs: Vec::new(),
                    data: json!({ "action": "start", "params": resolved, "trigger": fields }),
                },
                Err(inv) => Fired { outcome: outcome::ERROR, message: Some(first_errors(&inv)), runs: Vec::new(), data: json!({}) },
            };
        }
        let p = json!({
            "workflow_path": wf.path,
            "source": wf.source,
            "project_path": req.project,
            "params": params,
            "trigger": fields,
            "cause": cause,
        });
        match self.start(&p) {
            Ok(run) => Fired {
                outcome: outcome::STARTED,
                message: Some(format!("run {} {}", run.id, run.status.as_str())),
                runs: vec![run.id],
                data: json!({ "action": "start", "run": run }),
            },
            Err(e) if e.kind == ErrorKind::Conflict => {
                Fired { outcome: outcome::REJECTED, message: Some(e.message), runs: Vec::new(), data: json!({}) }
            }
            Err(e) => Fired { outcome: outcome::ERROR, message: Some(e.message), runs: Vec::new(), data: json!({}) },
        }
    }
}

/// `file specs/**/*.md fired (specs/a.md (modified))`, for event streams.
fn describe_event(trigger: &workflow::Trigger, fields: &Map<String, Value>) -> String {
    let paths = workflow::trigger_text(fields.get("paths").unwrap_or(&Value::Null));
    if paths.is_empty() {
        format!("{} fired", trigger.describe())
    } else {
        format!("{} fired ({paths})", trigger.describe())
    }
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

pub fn join_ids(ids: &[i64]) -> String {
    ids.iter().map(i64::to_string).collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_fields() {
        let now = Local::now();
        let ev = Event { paths: vec![("a.md".into(), FileEvent::Created), ("b.md".into(), FileEvent::Modified)], ..Default::default() };
        let wf = workflow::parse(Path::new("w.md"), "---\nname: w\ntriggers:\n  - file: \"*.md\"\n---\n").unwrap();
        let f = fields(&wf.frontmatter.triggers[0].kind, &ev, now);
        assert_eq!(f["kind"], "file");
        assert_eq!(f["event"], "created, modified");
        assert_eq!(f["paths"][1]["path"], "b.md");
        assert_eq!(f["scheduled"], "");
        let f = fields(&TriggerKind::Manual, &Event::default(), now);
        assert_eq!((f["kind"].as_str(), f["event"].as_str()), (Some("manual"), Some("")));
    }
}
