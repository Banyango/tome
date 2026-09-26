//! Filesystem locations used by tome. Everything lives under `~/.tome`, which
//! can be overridden with `TOME_HOME` (used by tests and by service units that
//! need a non-default home).

use std::path::PathBuf;

pub fn tome_home() -> PathBuf {
    if let Some(dir) = std::env::var_os("TOME_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(dir);
    }
    user_home().join(".tome")
}

pub fn user_home() -> PathBuf {
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

pub fn socket_path() -> PathBuf {
    tome_home().join("tome.sock")
}

pub fn lock_path() -> PathBuf {
    tome_home().join("daemon.lock")
}

pub fn daemon_log_path() -> PathBuf {
    tome_home().join("daemon.log")
}

pub fn db_path() -> PathBuf {
    tome_home().join("tome.duckdb")
}

pub fn runs_dir() -> PathBuf {
    tome_home().join("runs")
}
