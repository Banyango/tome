//! tome's config files: the global `~/.tome/config.yaml` and a project's
//! `.tome/config.yaml` (the nearest one above the run's project directory,
//! found the same way as project workflows).
//!
//! Both take the same keys. The project file wins per top-level key, and per
//! entry for the named maps (`harnesses`, `layout_presets`): an entry with
//! the same name in the project file replaces the global one entirely.

use crate::output::{CliError, CliResult};
use crate::paths;
use serde_yaml::{Mapping, Value as Yaml};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Every top-level key either file accepts.
pub const KEYS: &[&str] = &[
    "backend",
    "herdr",
    "harnesses",
    "layout",
    "layout_presets",
    "nodes",
    "bus",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Global,
    Project,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Global => "global",
            Scope::Project => "project",
        }
    }
}

/// One config file that exists.
#[derive(Debug, Clone)]
pub struct File {
    pub scope: Scope,
    pub path: PathBuf,
    doc: Mapping,
}

impl File {
    /// How error messages name the file.
    pub fn label(&self) -> String {
        match self.scope {
            Scope::Global => "the tome config".to_string(),
            Scope::Project => format!("the project config ({})", self.path.display()),
        }
    }

    /// A top-level value in this file.
    pub fn get(&self, key: &str) -> Option<&Yaml> {
        self.doc.get(key).filter(|v| !v.is_null())
    }

    /// An error about this file's contents.
    pub fn error(&self, e: impl std::fmt::Display) -> CliError {
        CliError::invalid(format!("{}: {e}", self.path.display()))
    }
}

/// The config files in effect for a project (or for none).
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Global first, then the project's.
    files: Vec<File>,
}

pub fn global_path() -> PathBuf {
    paths::tome_home().join("config.yaml")
}

/// The project config that applies in `dir`: the nearest `.tome/config.yaml`
/// above it that isn't the tome home's own.
pub fn project_path(dir: &Path) -> Option<PathBuf> {
    let home = paths::tome_home();
    dir.ancestors().find_map(|d| {
        let tome = d.join(".tome");
        let candidate = tome.join("config.yaml");
        (candidate.is_file() && !crate::workflow::same_path(&tome, &home)).then_some(candidate)
    })
}

impl Config {
    /// Read the global config and, with a project directory, its project
    /// config. Either may be missing; an unreadable or malformed one, or one
    /// with a key tome doesn't know, is an error.
    pub fn load(project: Option<&Path>) -> CliResult<Config> {
        let mut files = Vec::new();
        if let Some(f) = read(Scope::Global, global_path())? {
            files.push(f);
        }
        if let Some(f) = project
            .and_then(project_path)
            .map(|p| read(Scope::Project, p))
            .transpose()?
            .flatten()
        {
            files.push(f);
        }
        Ok(Config { files })
    }

    pub fn files(&self) -> &[File] {
        &self.files
    }

    /// A top-level value and the file it came from, the project's first.
    pub fn get(&self, key: &str) -> Option<(&Yaml, &File)> {
        self.files.iter().rev().find_map(|f| match f.doc.get(key) {
            None | Some(Yaml::Null) => None,
            Some(v) => Some((v, f)),
        })
    }

    /// A top-level string setting, e.g. `backend: cmux`.
    pub fn str(&self, key: &str) -> CliResult<Option<(String, &File)>> {
        match self.get(key) {
            None => Ok(None),
            Some((Yaml::String(s), f)) => Ok(Some((s.clone(), f))),
            Some((_, f)) => Err(f.error(format!("`{key}` must be a string"))),
        }
    }

    /// A named map (`harnesses`, `layout_presets`), merged per entry: the
    /// project's entries replace the global ones with the same name.
    pub fn entries(&self, key: &str) -> CliResult<BTreeMap<String, (Yaml, &File)>> {
        let mut out = BTreeMap::new();
        for f in &self.files {
            let map = match f.doc.get(key) {
                None | Some(Yaml::Null) => continue,
                Some(Yaml::Mapping(m)) => m,
                Some(_) => {
                    return Err(f.error(format!("`{key}` must be a mapping of name to entry")))
                }
            };
            for (name, value) in map {
                let name = name
                    .as_str()
                    .ok_or_else(|| f.error(format!("`{key}` names must be strings")))?;
                out.insert(name.to_string(), (value.clone(), f));
            }
        }
        Ok(out)
    }

    /// A config with just this text as its global file (for tests).
    #[cfg(test)]
    pub fn parse(scope: Scope, text: &str) -> CliResult<Config> {
        let path = PathBuf::from(format!("{}.yaml", scope.as_str()));
        Ok(Config {
            files: vec![parse(scope, path, text)?],
        })
    }
}

fn read(scope: Scope, path: PathBuf) -> CliResult<Option<File>> {
    match std::fs::read_to_string(&path) {
        Ok(text) => parse(scope, path, &text).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(CliError::internal(format!(
            "reading {}: {e}",
            path.display()
        ))),
    }
}

fn parse(scope: Scope, path: PathBuf, text: &str) -> CliResult<File> {
    let bad = |e: String| CliError::invalid(format!("{}: {e}", path.display()));
    let doc = match serde_yaml::from_str(text).map_err(|e| bad(e.to_string()))? {
        Yaml::Null => Mapping::new(),
        Yaml::Mapping(m) => m,
        _ => return Err(bad("expected a mapping at the top level".into())),
    };
    for key in doc.keys() {
        let key = key.as_str().unwrap_or("?");
        if !KEYS.contains(&key) {
            return Err(bad(format!("unknown key `{key}`"))
                .with_hint(format!("known keys: {}", KEYS.join(", "))));
        }
        // Hosts and SSH access are the user's; forwarding rules are the repo's.
        match (key, scope) {
            ("herdr", Scope::Project) => {
                return Err(bad("`herdr` settings go in the global config".into())
                    .with_hint(format!("move it to {}", global_path().display())))
            }
            ("herdr", Scope::Global) => {
                let map = doc
                    .get("herdr")
                    .and_then(Yaml::as_mapping)
                    .ok_or_else(|| bad("`herdr` must be a mapping".into()))?;
                for (key, value) in map {
                    if key.as_str() != Some("session") {
                        return Err(
                            bad("unknown herdr setting".into()).with_hint("known setting: session")
                        );
                    }
                    if !value.is_string() {
                        return Err(bad("`herdr.session` must be a string".into()));
                    }
                }
            }
            ("nodes", Scope::Project) => {
                return Err(bad("`nodes` goes in the global config".into())
                    .with_hint(format!("move it to {}", global_path().display())))
            }
            ("bus", Scope::Global) => {
                return Err(bad("`bus` goes in a project's .tome/config.yaml".into()))
            }
            _ => {}
        }
    }
    Ok(File { scope, path, doc })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_keys_are_refused() {
        let err = Config::parse(Scope::Project, "backend: tmux\nlayuot: split\n").unwrap_err();
        assert!(
            err.message.contains("unknown key `layuot`"),
            "{}",
            err.message
        );
        assert!(err.hint.unwrap().contains("layout_presets"));
    }

    #[test]
    fn project_wins_per_key_and_per_entry() {
        let global = parse(
            Scope::Global,
            "g.yaml".into(),
            "backend: tmux\nlayout: split\nharnesses:\n  a: x\n  b: y\n",
        )
        .unwrap();
        let project = parse(
            Scope::Project,
            "p.yaml".into(),
            "layout: tab\nharnesses:\n  b: z\n",
        )
        .unwrap();
        let cfg = Config {
            files: vec![global, project],
        };
        assert_eq!(cfg.str("backend").unwrap().unwrap().0, "tmux");
        let (layout, from) = cfg.str("layout").unwrap().unwrap();
        assert_eq!((layout.as_str(), from.scope), ("tab", Scope::Project));
        let h = cfg.entries("harnesses").unwrap();
        assert_eq!(h["a"].0, Yaml::from("x"));
        assert_eq!(
            (h["b"].0.clone(), h["b"].1.scope),
            (Yaml::from("z"), Scope::Project)
        );
    }
}
