//! Harness lookup across the user's own definitions and the enabled plugins'.
//!
//! [`crate::harness`] knows how to parse and run a harness; this layer adds the
//! plugins' harnesses on top, so it sits above both `harness` and the plugin
//! store. A name defined in more than one place is a clash.

use super::store;
use crate::config;
use crate::harness::{self, Candidate, Def, Harness};
use crate::output::{CliError, CliResult};
use std::collections::BTreeMap;
use std::path::Path;

/// Every definition of every harness name: the user's, and those of the
/// enabled plugins. A name with more than one is a clash.
pub fn candidates(project: Option<&Path>) -> CliResult<BTreeMap<String, Vec<Candidate>>> {
    let mut out: BTreeMap<String, Vec<Candidate>> = harness::user_candidates(project)?
        .into_iter()
        .map(|(name, c)| (name, vec![c]))
        .collect();
    for plugin in store::active() {
        let Some(m) = plugin.active() else { continue };
        for (name, def) in &m.harnesses {
            out.entry(name.clone()).or_default().push(Candidate {
                source: plugin.source_label(),
                plugin: Some(plugin.name.clone()),
                def: def.clone(),
            });
        }
    }
    Ok(out)
}

/// The error for a name defined in more than one place.
pub fn clash_error(name: &str, found: &[Candidate]) -> CliError {
    CliError::invalid(format!(
        "harness name `{name}` is defined more than once: {}",
        found
            .iter()
            .map(|c| c.source.as_str())
            .collect::<Vec<_>>()
            .join(" and ")
    ))
    .with_hint("rename one, or disable the plugin with `tome plugin disable <name>`")
}

/// Every known harness for a project: the user's and the plugins'. A name
/// defined twice is left out; [`resolve`] reports it.
pub fn all(project: Option<&Path>) -> CliResult<BTreeMap<String, Def>> {
    Ok(candidates(project)?
        .into_iter()
        .filter_map(|(name, mut found)| (found.len() == 1).then(|| (name, found.remove(0).def)))
        .collect())
}

/// Look a harness up by name, in the config that applies to `project`.
pub fn resolve(name: &str, project: Option<&Path>) -> CliResult<Harness> {
    let mut found = candidates(project)?;
    match found.remove(name) {
        Some(mut list) if list.len() > 1 => Err(clash_error(name, &list)),
        Some(mut list) => {
            let def = list.remove(0).def;
            Ok(Harness {
                name: name.to_string(),
                template: def.template,
                model_flag: def.model_flag,
            })
        }
        None => Err(CliError::invalid(format!("unknown harness `{name}`")).with_hint(format!(
            "known harnesses: {}; define others under `harnesses:` in {} or the project's .tome/config.yaml",
            found.keys().cloned().collect::<Vec<_>>().join(", "),
            config::global_path().display()
        ))),
    }
}
