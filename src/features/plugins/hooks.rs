//! The daemon's calls to plugin hooks about runs: the blocking
//! `run.before_start`, and the `run.started` and `run.finished` notifications.
//!
//! None of these is made while the store lock is held, and the notifications
//! never wait for the plugin.

use crate::api::StartRequest;
use crate::engine::Engine;
use crate::features::plugins::gate::{parse_reply, Gate, Verdict};
use crate::features::plugins::host::HOOK_TIMEOUT;
use crate::features::plugins::manifest::{Hook, OnError};
use crate::ids::RunId;
use crate::output::{CliError, CliResult};
use crate::store::{Run, RunStatus};
use crate::workflow::{Invalid, Workflow};
use serde_json::{json, Map, Value};
use std::path::Path;

const BEFORE_START: &str = "run.before_start";

impl Engine {
    /// Ask the `run.before_start` plugins about a new run. Returns the
    /// request as they leave it, or an error if one rejects it (or fails and
    /// its manifest doesn't say `on_error: allow`).
    pub(crate) fn before_start(
        &self,
        wf: &Workflow,
        req: &StartRequest,
    ) -> CliResult<StartRequest> {
        let hooks = self.plugins.declaring(BEFORE_START);
        if hooks.is_empty() {
            return Ok(req.clone());
        }
        let params = wf
            .resolve_params(&req.params, true)
            .map_err(Invalid::into_cli_error)?;
        let gate = self.consult(
            wf.name(),
            Some(wf.path()),
            req.project_path.as_deref(),
            params,
            req.cause.as_ref(),
            None,
            &hooks,
        )?;
        let mut out = req.clone();
        let changed = gate.overrides();
        out.params
            .retain(|(key, _)| !changed.iter().any(|(k, _)| k == key));
        out.params.extend(changed);
        out.record.extend(gate.record());
        // A plugin that set a param the workflow doesn't have is refused here
        // rather than as a bad `--param` the user never typed.
        if !gate.changes.is_empty() {
            wf.resolve_params(&out.params, true).map_err(|e| {
                CliError::conflict(format!(
                    "a `{BEFORE_START}` plugin set an invalid param: {}",
                    e.into_cli_error().message
                ))
            })?;
        }
        Ok(out)
    }

    /// The same question for a resumed run. The plugins may allow or reject
    /// it; changes they ask for aren't applied to a resumed run.
    pub(crate) fn before_resume(&self, old: &Run) -> CliResult<()> {
        let hooks = self.plugins.declaring(BEFORE_START);
        if hooks.is_empty() {
            return Ok(());
        }
        let params = old.params.as_object().cloned().unwrap_or_default();
        self.consult(
            &old.workflow_name,
            old.workflow_path.as_deref().map(Path::new),
            old.project_path.as_deref().map(Path::new),
            params,
            old.trigger.as_ref(),
            Some(old.id),
            &hooks,
        )
        .map(|_| ())
    }

    #[allow(clippy::too_many_arguments)]
    fn consult(
        &self,
        workflow: &str,
        path: Option<&Path>,
        project: Option<&Path>,
        params: Map<String, Value>,
        cause: Option<&Value>,
        resumes: Option<RunId>,
        hooks: &[(String, Hook)],
    ) -> CliResult<Gate> {
        let mut gate = Gate::new(params);
        for (plugin, hook) in hooks {
            let payload = json!({
                "workflow": workflow,
                "workflow_path": path,
                "project": project,
                "params": gate.params,
                "env": gate.env,
                "cause": cause,
                "resumes": resumes,
                "node": crate::features::nodes::host_name(),
            });
            let timeout = hook.timeout.unwrap_or(HOOK_TIMEOUT);
            let verdict = self
                .plugins
                .call(plugin, BEFORE_START, payload, timeout, |_| {})
                .map_err(|e| e.to_string())
                .and_then(|reply| parse_reply(&reply));
            match verdict {
                Ok(Verdict::Allow) => {}
                Ok(Verdict::Reject(reason)) => {
                    return Err(CliError::conflict(format!(
                        "the run of `{workflow}` was rejected by plugin `{plugin}`: {reason}"
                    )));
                }
                Ok(Verdict::Modify { params, env }) => {
                    if resumes.is_none() {
                        gate.apply(plugin, params, env);
                    }
                }
                Err(why) if hook.on_error == OnError::Allow => {
                    eprintln!(
                        "tome daemon: plugin {plugin}'s `{BEFORE_START}` failed ({why}); allowing the run"
                    );
                }
                Err(why) => {
                    return Err(CliError::conflict(format!(
                        "plugin `{plugin}` could not approve the run of `{workflow}`: {why}"
                    ))
                    .with_hint(format!(
                        "see {}; `tome plugin check {plugin}` tests it, and `on_error: allow` on the hook lets runs through when it fails",
                        crate::features::plugins::store::log_path(plugin).display()
                    )));
                }
            }
        }
        Ok(gate)
    }

    /// Tell the `run.started` plugins.
    pub(crate) fn plugins_started(&self, run: &Run) {
        self.plugins.fire(
            "run.started",
            json!({
                "run_id": run.id,
                "workflow": run.workflow_name,
                "project": run.project_path,
                "params": run.params,
                "cause": run.trigger,
                "node": crate::features::nodes::host_name(),
            }),
        );
    }

    /// Tell the `run.finished` plugins how the run ended.
    pub(crate) fn plugins_finished(&self, run: &Run) {
        let status = match run.status {
            RunStatus::Succeeded => "succeeded",
            RunStatus::Failed => "failed",
            RunStatus::Cancelled => "cancelled",
            _ => return,
        };
        self.plugins.fire(
            "run.finished",
            json!({
                "run_id": run.id,
                "workflow": run.workflow_name,
                "project": run.project_path,
                "status": status,
                "reason": run.reason,
                "summary": run.summary,
                "node": crate::features::nodes::host_name(),
            }),
        );
    }
}

// --- providers ----------------------------------------------------------

use crate::engine::Sink;
use crate::features::plugins::host::{CallError, ProviderHooks};
use crate::output::ErrorKind;
use std::time::Duration;

/// `45s`, `10m`.
fn span(d: Duration) -> String {
    let secs = d.as_secs();
    if secs >= 60 && secs % 60 == 0 {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

impl Engine {
    fn provider_of(&self, p: &Value) -> CliResult<(String, ProviderHooks)> {
        let name = p["provider"]
            .as_str()
            .filter(|n| !n.is_empty())
            .ok_or_else(|| CliError::invalid("missing `provider`"))?;
        let hooks = self.plugins.provider(name).ok_or_else(|| {
            CliError::not_found(format!("no running plugin provides `{name}`"))
                .with_hint("check it with `tome plugin ls`, then `tome plugin check <name>`")
        })?;
        Ok((name.to_string(), hooks))
    }

    /// `provider.provide {provider, opts, workflow, project}`: ask the
    /// plugin for a machine and wait until it is ready. The plugin's
    /// progress goes to the caller as `{type: "provider", message}` events.
    pub(crate) fn provide(&self, p: &Value, sink: &mut dyn Sink) -> CliResult<Value> {
        let (name, hooks) = self.provider_of(p)?;
        let request = json!({
            "provider": name,
            "opts": p.get("opts").cloned().unwrap_or_else(|| json!({})),
            "workflow": p.get("workflow"),
            "project": p.get("project"),
        });
        self.plugins
            .call(
                &hooks.plugin,
                "provide",
                request,
                hooks.provide,
                |message| {
                    // A caller that went away still gets its machine released.
                    let _ = sink.send(&json!({
                        "type": "provider",
                        "provider": name,
                        "message": message,
                    }));
                },
            )
            .map_err(|e| {
                let why = match &e {
                    CallError::Timeout(_) => format!(
                        "provider `{name}` didn't have a machine ready within {}",
                        span(hooks.provide)
                    ),
                    other => format!(
                        "provider `{name}` (plugin `{}`) failed: {other}",
                        hooks.plugin
                    ),
                };
                CliError::new(ErrorKind::Unreachable, why)
            })
    }

    /// `provider.release {provider, id, cause, opts, node, run, logs}`: tell
    /// the plugin the machine isn't needed any more. Not retried.
    pub(crate) fn release(&self, p: &Value) -> CliResult<Value> {
        let (name, hooks) = self.provider_of(p)?;
        self.plugins
            .call(&hooks.plugin, "release", p.clone(), hooks.release, |_| {})
            .map_err(|e| {
                CliError::new(
                    ErrorKind::Unreachable,
                    format!(
                        "provider `{name}` (plugin `{}`) failed to release: {e}",
                        hooks.plugin
                    ),
                )
            })
    }
}
