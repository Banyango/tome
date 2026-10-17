//! The workflow library as seen with plugins: [`Library`] plus the workflows
//! the enabled plugins list in their manifests.

use super::store;
use crate::workflow::{Library, PluginWorkflows};
use std::path::Path;

/// The directories visible from `cwd`, with the enabled plugins' workflows.
pub fn discover(cwd: &Path) -> Library {
    let mut library = Library::discover_user(cwd);
    library.plugin_dirs = store::active()
        .into_iter()
        .filter_map(|p| {
            let workflows = store::read_manifest(&p.dir).ok()?.workflows;
            Some(PluginWorkflows {
                plugin: p.name,
                dir: p.dir,
                workflows,
            })
        })
        .collect();
    library
}
