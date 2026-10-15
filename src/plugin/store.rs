//! What is installed under `~/.tome/plugins`, and the names the rest of the
//! user's setup already holds.

use super::manifest::{self, Claim, Manifest, Space, Version};
use crate::harness;
use crate::node;
use crate::output::{CliError, CliResult};
use crate::paths;
use crate::workflow::Library;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Kept in each plugin folder, next to the manifest, and never part of it:
/// where the plugin came from and whether it's switched on.
const STATE_FILE: &str = ".tome-install.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstallState {
    /// The path or git URL it was installed from, as given.
    pub source: String,
    /// A git install: the folder is a clone `tome plugin update` can pull.
    #[serde(default)]
    pub git: bool,
    /// The branch, tag or commit asked for with `#ref`.
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
}

fn enabled_by_default() -> bool {
    true
}

impl InstallState {
    pub fn read(dir: &Path) -> Option<InstallState> {
        serde_json::from_slice(&fs::read(dir.join(STATE_FILE)).ok()?).ok()
    }

    pub fn write(&self, dir: &Path) -> CliResult<()> {
        let text =
            serde_json::to_string_pretty(self).map_err(|e| CliError::internal(e.to_string()))?;
        fs::write(dir.join(STATE_FILE), text + "\n").map_err(|e| {
            CliError::internal(format!("writing {}: {e}", dir.join(STATE_FILE).display()))
        })
    }
}

/// One installed plugin folder.
#[derive(Debug, Clone)]
pub struct Installed {
    /// The folder name, which is the name `tome plugin` commands use.
    pub name: String,
    pub dir: PathBuf,
    pub manifest: Result<Manifest, String>,
    pub state: InstallState,
}

impl Installed {
    pub fn load(name: &str) -> Installed {
        let dir = paths::plugins_dir().join(name);
        let manifest = read_manifest(&dir).and_then(|m| {
            if m.name == name {
                Ok(m)
            } else {
                Err(format!(
                    "the manifest's name `{}` doesn't match its folder `{name}`",
                    m.name
                ))
            }
        });
        let state = InstallState::read(&dir).unwrap_or_else(|| InstallState {
            source: dir.display().to_string(),
            git: false,
            reference: None,
            enabled: true,
        });
        Installed {
            name: name.to_string(),
            dir,
            manifest,
            state,
        }
    }

    pub fn source_label(&self) -> String {
        format!("plugin `{}`", self.name)
    }

    pub fn claims(&self) -> Vec<Claim> {
        self.manifest
            .as_ref()
            .map(|m| m.claims(&self.source_label()))
            .unwrap_or_default()
    }
}

pub fn read_manifest(dir: &Path) -> Result<Manifest, String> {
    let path = dir.join("plugin.yaml");
    let text = fs::read_to_string(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    Manifest::parse(&text)
}

/// Every installed plugin, by name. Folders starting with `.` are scratch
/// space from an install in progress.
pub fn installed() -> Vec<Installed> {
    let Ok(read) = fs::read_dir(paths::plugins_dir()) else {
        return Vec::new();
    };
    let mut names: Vec<String> = read
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.'))
        .collect();
    names.sort();
    names.iter().map(|n| Installed::load(n)).collect()
}

impl Installed {
    /// The manifest, if this plugin is switched on, valid and compatible with
    /// this tome: the only plugins that contribute subcommands, workflows,
    /// harnesses, providers or hooks.
    pub fn active(&self) -> Option<&Manifest> {
        if !self.state.enabled {
            return None;
        }
        let m = self.manifest.as_ref().ok()?;
        manifest::compatibility(m, &tome_version()).ok()?;
        Some(m)
    }

    /// The folder the plugin may keep its own state in.
    pub fn data_dir(&self) -> PathBuf {
        data_dir(&self.name)
    }
}

/// `~/.tome/plugins/<name>/data`.
pub fn data_dir(name: &str) -> PathBuf {
    paths::plugins_dir().join(name).join("data")
}

/// `~/.tome/plugins/<name>/plugin.log`.
pub fn log_path(name: &str) -> PathBuf {
    paths::plugins_dir().join(name).join("plugin.log")
}

/// The installed plugins that are switched on, valid and compatible.
pub fn active() -> Vec<Installed> {
    installed()
        .into_iter()
        .filter(|p| p.active().is_some())
        .collect()
}

/// This tome's own version.
pub fn tome_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("Cargo version parses")
}

/// The names the user's own setup holds: tome's built-in commands, configured
/// nodes, global workflows and harnesses. Plugins add their own on top.
pub fn user_claims(builtin_commands: &[String]) -> CliResult<Vec<Claim>> {
    let mut out: Vec<Claim> = builtin_commands
        .iter()
        .map(|c| Claim {
            space: Space::Command,
            name: c.clone(),
            source: "a built-in command".into(),
        })
        .collect();
    for name in node::all()?.into_keys() {
        out.push(Claim {
            space: Space::Provider,
            source: format!("configured node `{name}`"),
            name,
        });
    }
    // `local` is `--on`'s word for this machine.
    out.push(Claim {
        space: Space::Provider,
        name: node::LOCAL.into(),
        source: "the reserved name `local`".into(),
    });
    let library = Library {
        global_dir: paths::tome_home().join("workflows"),
        project_dir: None,
        plugin_dirs: Vec::new(),
    };
    for entry in library.entries() {
        let name = entry.name().map(str::to_string).or_else(|| {
            entry
                .path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
        });
        if let Some(name) = name {
            out.push(Claim {
                space: Space::Workflow,
                name,
                source: format!("your workflow {}", entry.path.display()),
            });
        }
    }
    for name in harness::user(None)?.into_keys() {
        let source = if name == harness::DEFAULT {
            "the built-in harness".to_string()
        } else {
            format!("your harness in {}", crate::config::global_path().display())
        };
        out.push(Claim {
            space: Space::Harness,
            name,
            source,
        });
    }
    Ok(out)
}

/// The names held by everything but the plugin `except`: the user's setup and
/// every other installed plugin.
pub fn claims_except(builtin_commands: &[String], except: Option<&str>) -> CliResult<Vec<Claim>> {
    let mut out = user_claims(builtin_commands)?;
    for plugin in installed() {
        if Some(plugin.name.as_str()) != except {
            out.extend(plugin.claims());
        }
    }
    Ok(out)
}

/// Workflow and harness names that an enabled plugin shares with the user's
/// own setup (as seen from `cwd`) or with another enabled plugin. Install
/// refuses these, but they can appear afterwards: the user adds a workflow, a
/// plugin updates.
pub fn collisions(cwd: &Path) -> CliResult<Vec<manifest::Conflict>> {
    let wanted = |c: &Claim| matches!(c.space, Space::Workflow | Space::Harness);
    let mut user: Vec<Claim> = Library::discover(cwd)
        .entries()
        .into_iter()
        .filter(|e| e.plugin.is_none())
        .filter_map(|e| {
            Some(Claim {
                space: Space::Workflow,
                name: e.name()?.to_string(),
                source: e.source_label(),
            })
        })
        .collect();
    for (name, c) in harness::user_sources(Some(cwd)).unwrap_or_default() {
        user.push(Claim {
            space: Space::Harness,
            name,
            source: c,
        });
    }
    let plugins = active();
    let claims: Vec<Vec<Claim>> = plugins
        .iter()
        .map(|p| p.claims().into_iter().filter(wanted).collect())
        .collect();
    let mut out = Vec::new();
    for (i, mine) in claims.iter().enumerate() {
        let mut others = user.clone();
        others.extend(claims[i + 1..].iter().flatten().cloned());
        out.extend(conflicts(mine, &others));
    }
    Ok(out)
}

pub use manifest::conflicts;
