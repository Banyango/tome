//! Plugins: folders in `~/.tome/plugins/<name>/` with a `plugin.yaml`
//! manifest and their own executables, installed only by the user.
//!
//! [`manifest`] holds the deterministic rules (parsing, compatibility, name
//! conflicts); [`store`] reads what's installed and the names already taken.

pub mod gate;
pub mod host;
pub mod manifest;
pub mod store;
