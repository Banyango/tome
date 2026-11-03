//! Where a bus or queue lives: a project's own, a hub's, or global's. Every
//! hub and global operation names its scope with this type, and only the
//! store turns it into a stored key, so hub and global state can later be
//! owned by another machine without changing callers (see
//! `docs/modules/bus.md`).

use crate::text::is_identifier;
use std::fmt;
use std::path::{Path, PathBuf};

/// The name `global` is taken by the global scope.
pub const GLOBAL: &str = "global";

/// A hub's name: an identifier other than `global`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HubName(String);

impl HubName {
    pub fn new(name: &str) -> Result<HubName, String> {
        if name == GLOBAL {
            return Err("a hub can't be named `global`: that's the global scope".into());
        }
        if !is_identifier(name) || name.len() > 64 {
            return Err(format!(
                "invalid hub name `{name}`: use a letter or `_`, then letters, digits, `_` or `-` (at most 64)"
            ));
        }
        Ok(HubName(name.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for HubName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A scope a project can attach to: a hub or global.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Shared {
    Hub(HubName),
    Global,
}

const HUB_PREFIX: &str = "hub:";
const GLOBAL_KEY: &str = "global:";

impl Shared {
    /// From `--hub <name>` / `--global` (or `hub:` / `global: true`).
    pub fn from_flags(hub: Option<&str>, global: bool) -> Result<Option<Shared>, String> {
        match (hub, global) {
            (Some(_), true) => Err("give --hub or --global, not both".into()),
            (Some(h), false) => HubName::new(h).map(|h| Some(Shared::Hub(h))),
            (None, true) => Ok(Some(Shared::Global)),
            (None, false) => Ok(None),
        }
    }

    /// The key stored in `project_path` columns and `attachments.scope`.
    /// Never a valid absolute path, so it can't clash with a project's.
    pub fn key(&self) -> String {
        match self {
            Shared::Hub(h) => format!("{HUB_PREFIX}{h}"),
            Shared::Global => GLOBAL_KEY.to_string(),
        }
    }

    pub fn from_key(key: &str) -> Option<Shared> {
        if key == GLOBAL_KEY {
            return Some(Shared::Global);
        }
        key.strip_prefix(HUB_PREFIX)
            .and_then(|h| HubName::new(h).ok())
            .map(Shared::Hub)
    }

    pub fn hub(&self) -> Option<&HubName> {
        match self {
            Shared::Hub(h) => Some(h),
            Shared::Global => None,
        }
    }

    /// How it's shown: `hub release`, `global`.
    pub fn label(&self) -> String {
        match self {
            Shared::Hub(h) => format!("hub {h}"),
            Shared::Global => GLOBAL.to_string(),
        }
    }
}

/// Which bus or queue set an operation acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    Project(PathBuf),
    Shared(Shared),
}

impl Scope {
    pub fn key(&self) -> String {
        match self {
            Scope::Project(p) => p.display().to_string(),
            Scope::Shared(s) => s.key(),
        }
    }

    pub fn project(&self) -> Option<&Path> {
        match self {
            Scope::Project(p) => Some(p),
            Scope::Shared(_) => None,
        }
    }
}

/// Which daemon owns a scope's state. Always this machine for now; a remote
/// owner would be a node the façade sends the scope's calls to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Owner {
    Local,
}

pub fn owner(_scope: &Scope) -> Owner {
    Owner::Local
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hub_names() {
        assert!(HubName::new("release").is_ok());
        assert!(HubName::new("global").unwrap_err().contains("global scope"));
        assert!(HubName::new("1x").is_err());
        assert!(HubName::new("a/b").is_err());
        assert!(HubName::new(&"a".repeat(65)).is_err());
    }

    #[test]
    fn keys_round_trip_and_never_look_like_paths() {
        let hub = Shared::Hub(HubName::new("release").unwrap());
        for s in [hub.clone(), Shared::Global] {
            assert_eq!(Shared::from_key(&s.key()), Some(s.clone()));
            assert!(!s.key().starts_with('/'));
        }
        assert_eq!(Shared::from_key("/p"), None);
        assert_eq!(
            Shared::from_flags(Some("release"), false).unwrap(),
            Some(hub)
        );
        assert_eq!(
            Shared::from_flags(None, true).unwrap(),
            Some(Shared::Global)
        );
        assert!(Shared::from_flags(Some("a"), true).is_err());
        assert_eq!(Shared::from_flags(None, false).unwrap(), None);
    }
}
