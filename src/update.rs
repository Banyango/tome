//! New-release notices. The daemon asks GitHub for the latest release about
//! once a day and saves the answer in `~/.tome/update.json`; CLI commands read
//! that file and print one line to stderr when a newer version is out. The CLI
//! never goes to the network itself, so commands stay fast and work offline.
//!
//! Off with `TOME_NO_UPDATE_CHECK=1`, and whenever `CI` is set.

use std::io::IsTerminal;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::paths;
use crate::stop::Stop;

const LATEST_URL: &str = "https://api.github.com/repos/banyango/tome/releases/latest";
const INSTALL: &str =
    "curl -fsSL https://raw.githubusercontent.com/banyango/tome/main/install.sh | sh";
const EVERY: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Serialize, Deserialize)]
struct Saved {
    latest: String,
    checked_at: u64,
}

pub fn disabled() -> bool {
    let set = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty() && v != "0");
    set("TOME_NO_UPDATE_CHECK") || set("CI")
}

/// The daemon's loop: check when the saved answer is a day old, until shutdown.
pub fn check_loop(stop: &Stop) {
    if disabled() {
        return;
    }
    let file = paths::update_path();
    loop {
        let age = read(&file)
            .map(|s| now().saturating_sub(s.checked_at))
            .unwrap_or(u64::MAX);
        let wait = match EVERY.as_secs().checked_sub(age) {
            Some(left) if left > 0 => Duration::from_secs(left),
            _ => match fetch_latest() {
                Some(latest) => {
                    write(&file, &latest);
                    EVERY
                }
                // Offline or rate-limited: try again in an hour, not a day.
                None => Duration::from_secs(3600),
            },
        };
        if !stop.sleep(wait) {
            return;
        }
    }
}

/// The one-line notice for this binary, if a newer release has been seen.
pub fn notice() -> Option<String> {
    let saved = read(&paths::update_path())?;
    let current = env!("CARGO_PKG_VERSION");
    newer(&saved.latest, current).then(|| {
        format!(
            "tome {} is available (you have {current}). Update: {INSTALL}",
            saved.latest
        )
    })
}

/// Print the notice to stderr, for people at a terminal only.
pub fn print_notice() {
    if disabled() || !std::io::stderr().is_terminal() {
        return;
    }
    if let Some(line) = notice() {
        eprintln!("\n{line}");
    }
}

fn fetch_latest() -> Option<String> {
    let out = Command::new("curl")
        .args([
            "-fsSL",
            "--max-time",
            "10",
            "-H",
            "Accept: application/vnd.github+json",
        ])
        .args([
            "-A",
            concat!("tome/", env!("CARGO_PKG_VERSION")),
            LATEST_URL,
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let body: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let tag = body.get("tag_name")?.as_str()?;
    parse(tag).map(|_| tag.trim_start_matches('v').to_string())
}

fn read(file: &Path) -> Option<Saved> {
    serde_json::from_slice(&std::fs::read(file).ok()?).ok()
}

fn write(file: &Path, latest: &str) {
    let saved = Saved {
        latest: latest.to_string(),
        checked_at: now(),
    };
    let tmp = file.with_extension("json.tmp");
    let ok = serde_json::to_vec(&saved)
        .ok()
        .and_then(|bytes| std::fs::write(&tmp, bytes).ok())
        .and_then(|()| std::fs::rename(&tmp, file).ok());
    if ok.is_none() {
        eprintln!("tome daemon: couldn't save {}", file.display());
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `major.minor.patch`, with an optional leading `v`. Pre-releases don't
/// count: they're never offered as updates.
fn parse(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.trim().trim_start_matches('v').split('.');
    let mut next = || parts.next()?.parse().ok();
    let v = (next()?, next()?, next()?);
    parts.next().is_none().then_some(v)
}

fn newer(latest: &str, current: &str) -> bool {
    match (parse(latest), parse(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions_numerically() {
        assert!(newer("0.10.0", "0.9.9"));
        assert!(newer("v1.0.0", "0.99.0"));
        assert!(!newer("0.9.0", "0.9.0"));
        assert!(!newer("0.8.5", "0.9.0"));
    }

    #[test]
    fn ignores_versions_it_cannot_read() {
        assert!(!newer("1.0.0-rc.1", "0.9.0"));
        assert!(!newer("garbage", "0.9.0"));
        assert!(!newer("1.0", "0.9.0"));
    }

    #[test]
    fn round_trips_the_saved_answer() {
        let dir = std::env::temp_dir().join(format!("tome-update-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("update.json");
        write(&file, "9.9.9");
        let saved = read(&file).unwrap();
        assert_eq!(saved.latest, "9.9.9");
        assert!(now() - saved.checked_at < 5);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
