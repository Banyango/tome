//! The one loader for hub layers: which hubs a project is attached to, and
//! where their workflows and config are. Workflow and config resolution
//! walk these between global and the project.
//!
//! Attachments are owned by the daemon's store, which mirrors them into
//! `~/.tome/attachments.json` on every change so this loader works without
//! the daemon (`tome validate`) and inside it. Hub files are read from
//! `~/.tome/hubs/<name>/` for now; a remote owner would fetch them here.

use crate::paths;
use crate::scope::{HubName, Shared};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// An attached hub that exists on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layer {
    pub hub: HubName,
    pub dir: PathBuf,
}

impl Layer {
    pub fn workflows_dir(&self) -> PathBuf {
        self.dir.join("workflows")
    }

    pub fn config_path(&self) -> PathBuf {
        self.dir.join("config.yaml")
    }
}

pub fn hub_dir(hub: &HubName) -> PathBuf {
    paths::hubs_dir().join(hub.as_str())
}

/// What one project is attached to, as mirrored.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Mirrored {
    #[serde(default)]
    pub hubs: Vec<String>,
    #[serde(default)]
    pub global: bool,
}

fn mirror_path() -> PathBuf {
    paths::tome_home().join("attachments.json")
}

/// Replace the mirror with `attachments` (project path to what it's
/// attached to, in attach order). Best effort: a failed write is reported,
/// and the next change writes it again.
pub fn write_mirror(attachments: &BTreeMap<String, Vec<Shared>>) {
    let out: BTreeMap<&str, Mirrored> = attachments
        .iter()
        .map(|(project, to)| {
            let m = Mirrored {
                hubs: to
                    .iter()
                    .filter_map(|s| s.hub().map(|h| h.to_string()))
                    .collect(),
                global: to.contains(&Shared::Global),
            };
            (project.as_str(), m)
        })
        .collect();
    let path = mirror_path();
    let text = serde_json::to_string_pretty(&out).unwrap_or_default();
    let tmp = path.with_extension("json.tmp");
    if let Err(e) = std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, &path)) {
        eprintln!("tome daemon: writing {}: {e}", path.display());
    }
}

/// The hubs `project` (a project root) is attached to that exist here, in
/// attach order.
pub fn hubs(project: &Path) -> Vec<Layer> {
    let Ok(text) = std::fs::read_to_string(mirror_path()) else {
        return Vec::new();
    };
    let Ok(all) = serde_json::from_str::<BTreeMap<String, Mirrored>>(&text) else {
        return Vec::new();
    };
    let Some(m) = all
        .iter()
        .find(|(p, _)| paths::same_path(Path::new(p), project))
        .map(|(_, m)| m)
    else {
        return Vec::new();
    };
    m.hubs
        .iter()
        .filter_map(|h| HubName::new(h).ok())
        .map(|hub| Layer {
            dir: hub_dir(&hub),
            hub,
        })
        .filter(|l| l.dir.is_dir())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirror_round_trips() {
        let m: Mirrored = serde_json::from_str(r#"{"hubs":["a","b"]}"#).unwrap();
        assert_eq!((m.hubs.len(), m.global), (2, false));
    }
}
