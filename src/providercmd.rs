//! `tome run <workflow> --on <provider>`: make a machine with a plugin, run
//! the workflow there as on any node, and release the machine afterwards.
//!
//! The plugin lives in the daemon, so `provide` and `release` go through it
//! (`provider.provide`, which streams the plugin's progress, and
//! `provider.release`). This side registers the machine as an ephemeral
//! node, checks it once, runs the workflow, keeps a local record of the
//! outcome and releases the machine, whatever happened.

use crate::config::Config;
use crate::node::{self, Node};
use crate::nodecmd;
use crate::output::{exit, CliError, CliResult, ErrorKind, Report};
use crate::paths;
use crate::plugin::store;
use crate::provider::{self, Cause, Opts, Provided, Stub};
use crate::rpc;
use crate::runcmd;
use crate::workflow::Library;
use serde_json::{json, Value};
use serde_yaml::Value as Yaml;
use std::cell::Cell;
use std::path::Path;

/// The plugin that provides `name`, if an active one does.
pub fn provider_plugin(name: &str) -> Option<String> {
    store::active()
        .into_iter()
        .find(|p| {
            p.active()
                .is_some_and(|m| m.providers.iter().any(|n| n == name))
        })
        .map(|p| p.name)
}

pub fn is_provider(name: &str) -> bool {
    provider_plugin(name).is_some()
}

/// What `tome run` was asked on a provider.
pub struct Job<'a> {
    pub provider: &'a str,
    pub cwd: &'a Path,
    pub workflow: &'a str,
    /// `--provider-opt` arguments.
    pub opt_args: &'a [String],
    pub keep: bool,
    /// `--detach` or `--view`: tome returns while the run goes on.
    pub detached: bool,
}

/// Make the machine, run `start` on it and release it. Exit `3` with the
/// check's fix hint if the machine doesn't come up or doesn't pass its check.
pub fn run(job: &Job, start: impl FnOnce() -> CliResult<Report>) -> CliResult<Report> {
    if job.detached && !job.keep {
        return Err(CliError::invalid(
            "--detach and --view return while the run goes on, so the machine couldn't be released after it",
        )
        .with_hint("add --keep to leave the machine for you to release, or leave out --detach"));
    }
    let opts = options(job)?;
    let provider = job.provider;

    runcmd::install_signal_handlers();
    let mut client = rpc::Client::local(&paths::socket_path())?;
    eprintln!("provider {provider}: asking for a machine");
    let interrupted = Cell::new(false);
    let reply = client.stream(
        "provider.provide",
        json!({
            "provider": provider,
            "opts": opts,
            "workflow": job.workflow,
            "project": job.cwd,
        }),
        |event| {
            if let Some(message) = event["message"].as_str() {
                eprintln!("provider {provider}: {message}");
            }
        },
        || {
            if runcmd::interrupted() {
                interrupted.set(true);
                return Err(CliError::internal("interrupted"));
            }
            Ok(())
        },
    );
    drop(client);
    let reply = match reply {
        Ok(reply) => reply,
        Err(e) if interrupted.get() => {
            release(job, &opts, None, Cause::Interrupted, None);
            let _ = e;
            return Ok(Report::new(
                json!({ "provider": provider, "cancelled": true }),
                format!("cancelled while {provider} was making a machine"),
            )
            .with_exit(exit::CANCELLED));
        }
        Err(e) => {
            // The plugin never started making one: nothing to clean up.
            if !matches!(e.kind, ErrorKind::NotFound | ErrorKind::Invalid) {
                release(job, &opts, None, Cause::ProvideFailed, None);
            }
            return Err(e);
        }
    };
    let provided = match Provided::from_reply(&reply) {
        Ok(p) => p,
        Err(why) => {
            release(job, &opts, reply["id"].as_str(), Cause::ProvideFailed, None);
            return Err(CliError::new(
                ErrorKind::Unreachable,
                format!("provider `{provider}` returned a machine tome can't use: {why}"),
            )
            .with_hint(
                "the plugin's `provide` must return {\"id\", \"ssh\"} (see the plugins guide)",
            ));
        }
    };
    let machine = provided.node(provider);
    if let Err(e) = provider::register(&machine) {
        eprintln!("warning: couldn't register node {}: {e}", machine.name);
    }

    if let Err(e) = check(&machine, job.cwd) {
        release(job, &opts, Some(&provided.id), Cause::CheckFailed, None);
        provider::unregister(&machine.name);
        return Err(e);
    }
    eprintln!(
        "provider {provider}: node {} is ready ({})",
        machine.name, machine.ssh
    );

    node::set_target(machine.clone());
    let result = start();

    if job.keep {
        eprintln!(
            "kept {} (--keep): `tome runs ls --on {}` shows its runs, and `tome node rm {}` forgets it. \
             The machine is yours to release.",
            machine.name, machine.name, machine.name
        );
        return result;
    }

    let run_id = result.as_ref().ok().and_then(|r| r.data["id"].as_i64());
    let recorded = record(provider, &provided, &machine, run_id);
    release(
        job,
        &opts,
        Some(&provided.id),
        Cause::Finished,
        Some((&recorded, &machine)),
    );
    provider::unregister(&machine.name);
    result
}

/// `--provider-opt` over the workflow's `defaults.provider_opts` over the
/// config's.
fn options(job: &Job) -> CliResult<Opts> {
    let cli = provider::parse_opts(job.opt_args).map_err(CliError::invalid)?;
    let config = config_opts(job.cwd)?;
    let workflow = Library::discover(job.cwd)
        .find(job.workflow)
        .map(|wf| wf.frontmatter().defaults.provider_opts.clone())
        .unwrap_or_default();
    Ok(provider::merge_opts(&[config, workflow, cli]))
}

/// `defaults.provider_opts` in the project's config, else the global one.
fn config_opts(cwd: &Path) -> CliResult<Opts> {
    let cfg = Config::load(Some(cwd))?;
    let Some((defaults, file)) = cfg.get("defaults") else {
        return Ok(Opts::new());
    };
    let Yaml::Mapping(map) = defaults else {
        return Err(file.error("`defaults` must be a mapping"));
    };
    match map.get("provider_opts") {
        None => Ok(Opts::new()),
        Some(v) => provider::opts_from_yaml(v).map_err(|e| file.error(format!("defaults: {e}"))),
    }
}

/// `node check`'s steps, once. The first failure is the error, with its fix.
fn check(machine: &Node, cwd: &Path) -> CliResult<()> {
    let steps = nodecmd::check_steps(machine, Some(cwd));
    let Some(failed) = steps.iter().find(|s| s.error.is_some()) else {
        return Ok(());
    };
    let e = failed.error.as_ref().expect("found by is_some");
    let mut err = CliError::new(
        ErrorKind::Unreachable,
        format!(
            "the machine {} failed its check at `{}`: {}",
            machine.name, failed.name, e.message
        ),
    );
    if let Some(hint) = &e.hint {
        err = err.with_hint(hint.clone());
    }
    Err(err)
}

/// A stub of the run on the machine, saved here before it goes away.
struct Recorded {
    stub: Stub,
    /// The run's log files on the machine.
    node_logs: Vec<String>,
    path: Option<String>,
}

fn record(provider: &str, provided: &Provided, machine: &Node, run_id: Option<i64>) -> Recorded {
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let (stub, node_logs) = match fetch(machine, run_id) {
        Ok((show, logs)) => {
            let node_logs = logs["logs"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|l| l["path"].as_str().map(str::to_string))
                .collect();
            (
                Stub::from_node(provider, provided, &show, Some(&logs), &now),
                node_logs,
            )
        }
        Err(why) => {
            eprintln!(
                "warning: couldn't read the run on {} before release: {why}",
                machine.name
            );
            (
                Stub::unavailable(provider, provided, &why, &now),
                Vec::new(),
            )
        }
    };
    let path = match provider::save_stub(&stub) {
        Ok(p) => Some(p.display().to_string()),
        Err(e) => {
            eprintln!("warning: couldn't save the record of the run: {e}");
            None
        }
    };
    Recorded {
        stub,
        node_logs,
        path,
    }
}

/// The run's `runs.show` and `runs.logs` (tail) from the machine.
fn fetch(machine: &Node, run_id: Option<i64>) -> Result<(Value, Value), String> {
    let mut client = rpc::Client::to_node(machine).map_err(|e| e.message)?;
    let id = match run_id {
        Some(id) => id,
        // The attached run failed before saying its id: the machine was made
        // for this run, so its newest run is it.
        None => client
            .call("runs.list", json!({ "limit": 1 }))
            .map_err(|e| e.message)?["runs"][0]["id"]
            .as_i64()
            .ok_or("the machine has no run")?,
    };
    let show = client
        .call("runs.show", json!({ "id": id, "snapshot": false }))
        .map_err(|e| e.message)?;
    let logs = client
        .call(
            "runs.logs",
            json!({ "id": id, "tail": provider::TAIL_LINES }),
        )
        .unwrap_or_else(|_| json!({ "logs": [] }));
    Ok((show, logs))
}

/// Tell the plugin to release the machine. A failure is notified, once.
fn release(
    job: &Job,
    opts: &Opts,
    id: Option<&str>,
    cause: Cause,
    recorded: Option<(&Recorded, &Node)>,
) {
    let params = json!({
        "provider": job.provider,
        "id": id,
        "cause": cause.as_str(),
        "opts": opts,
        "run": recorded.map(|(r, _)| r.stub.summary_json()),
        // Where the run's logs are: files on the machine (reach it with
        // `ssh`), and the local record, which holds their tail.
        "logs": recorded.map(|(r, machine)| json!({
            "node": machine.name,
            "ssh": machine.ssh,
            "paths": r.node_logs,
            "stub": r.path,
        })),
    });
    // The plugin lives in this machine's daemon, not on the run's node.
    let sent = rpc::Client::local(&paths::socket_path())
        .and_then(|mut client| client.call("provider.release", params));
    if let Err(e) = sent {
        let what = id.map_or_else(
            || format!("a machine from {}", job.provider),
            |id| format!("{}:{id}", job.provider),
        );
        let message = format!("couldn't release {what}: {}", e.message);
        eprintln!("warning: {message}; it is not retried");
        let _ = crate::plugincmd::notify("tome: release failed", &message);
    }
}
