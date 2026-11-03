//! Hubs: named, machine-local groups of projects with a shared bus, queues,
//! workflows and config, kept under `~/.tome/hubs/<name>/`. A project
//! reaches a hub (or global's bus and queues) only once it's attached.
//!
//! A hub exists while its directory does. Attachments are rows in the
//! daemon's store, never in the repo.

use crate::engine::Engine;
use crate::output::{CliError, CliResult};
use crate::params::{opt_flag, opt_str, req_str};
use crate::paths;
use crate::scope::{HubName, Shared};
use crate::store::Store;
use serde_json::{json, Value};
use std::path::PathBuf;

pub mod cmd;

pub const METHODS: &[&str] = &[
    "hub.create",
    "hub.rm",
    "hub.attach",
    "hub.detach",
    "hub.ls",
    "hub.show",
];

pub fn dir(hub: &HubName) -> PathBuf {
    crate::layers::hub_dir(hub)
}

pub fn exists(hub: &HubName) -> bool {
    dir(hub).is_dir()
}

/// Every hub on this machine, by name.
pub fn all() -> Vec<HubName> {
    let mut out: Vec<HubName> = std::fs::read_dir(paths::hubs_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| HubName::new(&e.file_name().to_string_lossy()).ok())
        .collect();
    out.sort();
    out
}

/// Create a hub's directory (with an empty `workflows/`). Returns whether
/// it was new.
pub fn create_dir(hub: &HubName) -> std::io::Result<bool> {
    if exists(hub) {
        return Ok(false);
    }
    std::fs::create_dir_all(dir(hub).join("workflows"))?;
    Ok(true)
}

/// A hub or global from RPC params: `hub` or `global: true`.
pub fn shared_param(p: &Value) -> CliResult<Shared> {
    bus_param(p)?.ok_or_else(|| CliError::invalid("name a hub, or say global"))
}

/// The same, optional: `None` is the project's own bus or queues.
pub fn bus_param(p: &Value) -> CliResult<Option<Shared>> {
    let global = opt_flag(p, "global", false)?;
    Shared::from_flags(opt_str(p, "hub"), global).map_err(CliError::invalid)
}

fn hub_param(p: &Value) -> CliResult<HubName> {
    HubName::new(req_str(p, "name")?).map_err(CliError::invalid)
}

/// The error for a hub that isn't on this machine.
pub fn missing(hub: &HubName) -> CliError {
    CliError::not_found(format!("there's no hub `{hub}`")).with_hint(format!(
        "create it with `tome hub create {hub}`, or see `tome hub ls`"
    ))
}

/// Refuse a project that isn't attached to `to` (a hub that doesn't exist
/// is reported as such).
pub fn require_attached(store: &Store, project: Option<&str>, to: &Shared) -> CliResult<()> {
    if let Some(h) = to.hub() {
        if !exists(h) {
            return Err(missing(h));
        }
    }
    let attach = match to {
        Shared::Hub(h) => format!("tome hub attach {h}"),
        Shared::Global => "tome hub attach --global".to_string(),
    };
    let Some(project) = project else {
        return Err(CliError::invalid("not inside an attached project")
            .with_hint(format!("run this in a project attached with `{attach}`")));
    };
    if store.is_attached(project, to)? {
        return Ok(());
    }
    Err(
        CliError::invalid(format!("{project} isn't attached to {}", to.label()))
            .with_hint(format!("attach it with `{attach}`")),
    )
}

impl Engine {
    pub(crate) fn dispatch_hubs(&self, method: &str, p: &Value) -> CliResult<Value> {
        match method {
            "hub.create" => self.hub_create(p),
            "hub.rm" => self.hub_rm(p),
            "hub.attach" => self.hub_attach(p, true),
            "hub.detach" => self.hub_attach(p, false),
            "hub.ls" => self.hub_ls(),
            "hub.show" => self.hub_show(p),
            _ => Err(CliError::invalid(format!("unknown method `{method}`"))),
        }
    }

    /// `hub.create {name}`
    fn hub_create(&self, p: &Value) -> CliResult<Value> {
        let hub = hub_param(p)?;
        if !create_dir(&hub)? {
            return Err(
                CliError::conflict(format!("hub `{hub}` already exists")).with_hint(format!(
                    "attach a project to it with `tome hub attach {hub}`"
                )),
            );
        }
        Ok(json!({ "hub": hub.as_str(), "path": dir(&hub), "created": true }))
    }

    /// `hub.rm {name}`: refused while any project is attached.
    fn hub_rm(&self, p: &Value) -> CliResult<Value> {
        let hub = hub_param(p)?;
        if !exists(&hub) {
            return Err(missing(&hub));
        }
        let shared = Shared::Hub(hub.clone());
        let attached = self.with_store(|store| store.attached_projects(&shared))?;
        if !attached.is_empty() {
            return Err(CliError::conflict(format!(
                "hub `{hub}` has attached projects: {}",
                attached.join(", ")
            ))
            .with_hint(format!(
                "detach them first (`tome hub detach {hub}` in each)"
            )));
        }
        std::fs::remove_dir_all(dir(&hub))?;
        Ok(json!({ "hub": hub.as_str(), "removed": true }))
    }

    /// `hub.attach|hub.detach {project_path, hub? | global?}`
    fn hub_attach(&self, p: &Value, attach: bool) -> CliResult<Value> {
        let project = req_str(p, "project_path")?;
        let to = shared_param(p)?;
        if let Some(h) = to.hub() {
            if attach && !exists(h) {
                return Err(missing(h));
            }
        }
        let changed = self.with_store(|store| {
            let changed = if attach {
                store.register_project(project)?;
                store.attach(project, &to)
            } else {
                store.detach(project, &to)
            }?;
            store.mirror_attachments()?;
            Ok(changed)
        })?;
        Ok(json!({
            "project": project,
            "scope": to.label(),
            "hub": to.hub().map(HubName::as_str),
            "global": to == Shared::Global,
            "attached": attach,
            "changed": changed,
        }))
    }

    /// `hub.ls`: every hub, and global, with their attached projects.
    fn hub_ls(&self) -> CliResult<Value> {
        let mut scopes: Vec<Shared> = all().into_iter().map(Shared::Hub).collect();
        scopes.push(Shared::Global);
        let rows = scopes
            .iter()
            .map(|s| {
                let projects = self.with_store(|store| store.attached_projects(s))?;
                Ok(json!({
                    "name": s.hub().map_or("global", HubName::as_str),
                    "global": *s == Shared::Global,
                    "projects": projects,
                }))
            })
            .collect::<CliResult<Vec<Value>>>()?;
        Ok(json!({ "hubs": rows }))
    }

    /// `hub.show {name? | global?, project_path?}`: one hub (or global),
    /// or with only `project_path`, what that project is attached to.
    fn hub_show(&self, p: &Value) -> CliResult<Value> {
        if opt_str(p, "name").is_none() && !opt_flag(p, "global", false)? {
            let project = req_str(p, "project_path")?;
            let to = self.with_store(|store| store.attachments_of(project))?;
            let labels: Vec<String> = to.iter().map(Shared::label).collect();
            return Ok(json!({ "project": project, "attached": labels }));
        }
        let shared = match opt_str(p, "name") {
            Some(_) => Shared::Hub(hub_param(p)?),
            None => Shared::Global,
        };
        let path = match &shared {
            Shared::Hub(h) if !exists(h) => return Err(missing(h)),
            Shared::Hub(h) => dir(h),
            Shared::Global => paths::tome_home(),
        };
        let projects = self.with_store(|store| store.attached_projects(&shared))?;
        let mut workflows: Vec<String> = std::fs::read_dir(path.join("workflows"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "md"))
            .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .collect();
        workflows.sort();
        Ok(json!({
            "name": shared.hub().map_or("global", HubName::as_str),
            "global": shared == Shared::Global,
            "path": path,
            "projects": projects,
            "workflows": workflows,
        }))
    }
}
