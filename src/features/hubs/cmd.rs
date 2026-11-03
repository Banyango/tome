//! `tome hub create|rm|attach|detach|ls|show`: the client side of hubs.

use crate::features::nodes as node;
use crate::output::{table, CliError, CliResult, Report};
use crate::scope::{HubName, Shared};
use crate::{paths, rpc};
use serde_json::{json, Value};
use std::path::Path;

fn call(method: &str, params: Value) -> CliResult<Value> {
    rpc::call(&paths::socket_path(), method, params)
}

fn project(cwd: &Path) -> CliResult<String> {
    node::project(cwd)?
        .map(|p| p.display().to_string())
        .ok_or_else(|| {
            CliError::invalid("not inside a project").with_hint(
                "attachments belong to a project: run this in one (a directory with .tome/)",
            )
        })
}

fn name(hub: &str) -> CliResult<HubName> {
    HubName::new(hub).map_err(CliError::invalid)
}

/// `--hub`-style params for a hub name or `--global`.
fn target(hub: Option<&str>, global: bool) -> CliResult<Shared> {
    Shared::from_flags(hub, global)
        .map_err(CliError::invalid)?
        .ok_or_else(|| {
            CliError::invalid("name a hub, or pass --global").with_hint("see `tome hub ls`")
        })
}

fn scope_params(to: &Shared) -> Value {
    match to {
        Shared::Hub(h) => json!({ "hub": h.as_str() }),
        Shared::Global => json!({ "global": true }),
    }
}

pub fn create(hub: &str) -> CliResult<Report> {
    let hub = name(hub)?;
    let out = call("hub.create", json!({ "name": hub.as_str() }))?;
    let human = format!(
        "created hub {hub}{} at {}\nattach a project with `tome hub attach {hub}`",
        node::on_suffix(),
        out["path"].as_str().unwrap_or("")
    );
    Ok(Report::new(out, human))
}

pub fn rm(hub: &str, force: bool) -> CliResult<Report> {
    let hub = name(hub)?;
    let out = call("hub.rm", json!({ "name": hub.as_str(), "force": force }))?;
    let detached = out["detached"].as_array().map_or(0, Vec::len);
    let note = if detached > 0 {
        format!(" (detached {detached} project(s))")
    } else {
        String::new()
    };
    Ok(Report::new(
        out,
        format!("removed hub {hub}{note}{}", node::on_suffix()),
    ))
}

pub fn attach(cwd: &Path, hub: Option<&str>, global: bool, on: bool) -> CliResult<Report> {
    let to = target(hub, global)?;
    let project = project(cwd)?;
    let method = if on { "hub.attach" } else { "hub.detach" };
    let mut params = scope_params(&to);
    params["project_path"] = json!(project);
    let out = call(method, params)?;
    let changed = out["changed"] == true;
    let human = match (on, changed) {
        (true, true) => format!("attached {project} to {}", to.label()),
        (true, false) => format!("{project} was already attached to {}", to.label()),
        (false, true) => {
            let mut s = format!("detached {project} from {}", to.label());
            for (k, what) in [
                ("dropped", "pending deliveries dropped"),
                ("released", "claimed messages returned"),
            ] {
                let n = out[k].as_u64().unwrap_or(0);
                if n > 0 {
                    s += &format!("\n{n} {what}");
                }
            }
            s
        }
        (false, false) => format!("{project} wasn't attached to {}", to.label()),
    };
    Ok(Report::new(out, human + &node::on_suffix()))
}

pub fn ls() -> CliResult<Report> {
    let out = call("hub.ls", json!({}))?;
    let rows: Vec<Vec<String>> = out["hubs"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|h| {
            let projects = h["projects"].as_array().map_or(0, Vec::len);
            vec![
                h["name"].as_str().unwrap_or("").to_string(),
                projects.to_string(),
            ]
        })
        .collect();
    let human = table(&["HUB", "PROJECTS"], rows);
    Ok(Report::new(out, human))
}

/// `tome hub show [<name> | --global]`: a hub, or with neither, what the
/// current project is attached to.
pub fn show(cwd: &Path, hub: Option<&str>, global: bool) -> CliResult<Report> {
    if hub.is_none() && !global {
        let project = project(cwd)?;
        let out = call("hub.show", json!({ "project_path": project }))?;
        let attached: Vec<&str> = out["attached"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        let human = if attached.is_empty() {
            format!("{project} isn't attached to any hub\nattach it with `tome hub attach <name>`")
        } else {
            format!("{project} is attached to: {}", attached.join(", "))
        };
        return Ok(Report::new(out, human));
    }
    let params = match target(hub, global)? {
        Shared::Hub(h) => json!({ "name": h.as_str() }),
        Shared::Global => json!({ "global": true }),
    };
    let out = call("hub.show", params)?;
    let list = |key: &str| -> String {
        let items: Vec<&str> = out[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        if items.is_empty() {
            "(none)".to_string()
        } else {
            items.join("\n  ")
        }
    };
    let human = format!(
        "{}\npath: {}\nprojects:\n  {}\nworkflows:\n  {}",
        out["name"].as_str().unwrap_or(""),
        out["path"].as_str().unwrap_or(""),
        list("projects"),
        list("workflows"),
    );
    Ok(Report::new(out, human))
}
