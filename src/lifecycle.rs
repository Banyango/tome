//! `tome daemon start|stop|status`.

use crate::output::{exit, CliError, CliResult, ErrorKind, Report};
use crate::paths;
use crate::rpc;
use crate::service;
use serde_json::{json, Value};
use std::fs::OpenOptions;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const START_TIMEOUT: Duration = Duration::from_secs(15);
const STOP_TIMEOUT: Duration = Duration::from_secs(10);

/// Ask a running daemon for its status; `Ok(None)` if it isn't running.
pub fn probe() -> CliResult<Option<Value>> {
    match rpc::call(&paths::socket_path(), "daemon.status", json!({})) {
        Ok(status) => Ok(Some(status)),
        Err(e) if e.kind == ErrorKind::DaemonNotRunning => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn status() -> CliResult<Report> {
    match probe()? {
        Some(status) => {
            let human = format!(
                "tome daemon is running (pid {}, version {}, up {}s)\nsocket: {}",
                status["pid"],
                status["version"].as_str().unwrap_or("?"),
                status["uptime_secs"],
                status["socket"].as_str().unwrap_or("?"),
            );
            Ok(Report::new(status, human))
        }
        None => Ok(Report::new(
            json!({ "running": false, "socket": paths::socket_path() }),
            "tome daemon is not running\nhint: start it with `tome daemon start`, or register it as a service with `tome daemon install`",
        )
        .with_exit(exit::DAEMON_UNAVAILABLE)),
    }
}

pub fn start() -> CliResult<Report> {
    if let Some(status) = probe()? {
        let human = format!("tome daemon is already running (pid {})", status["pid"]);
        return Ok(Report::new(
            json!({ "started": false, "already_running": true, "status": status }),
            human,
        ));
    }

    // With a service installed, let the service manager own the process so
    // it keeps supervising it.
    let via = match service::installed() {
        Some((platform, unit)) => {
            service::start_installed(platform, &unit)?;
            "service"
        }
        None => {
            spawn_detached()?;
            "process"
        }
    };
    let status = wait_until_up()?;
    let human = format!("tome daemon started (pid {})", status["pid"]);
    Ok(Report::new(
        json!({ "started": true, "already_running": false, "via": via, "status": status }),
        human,
    ))
}

/// Start `tome daemon run` in its own process group, detached from this
/// terminal, with output appended to `~/.tome/daemon.log`.
fn spawn_detached() -> CliResult<()> {
    let home = paths::tome_home();
    std::fs::create_dir_all(&home)?;
    let log_path = paths::daemon_log_path();
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let exe = std::env::current_exe()?;
    Command::new(exe)
        .args(["daemon", "run"])
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .process_group(0)
        .spawn()
        .map_err(|e| CliError::internal(format!("failed to spawn daemon: {e}")))?;
    Ok(())
}

pub fn wait_until_up() -> CliResult<Value> {
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        if let Some(status) = probe()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            return Err(
                CliError::internal("the daemon did not come up in time").with_hint(format!(
                    "check {} for errors",
                    paths::daemon_log_path().display()
                )),
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Stopping is the same with or without a service: the daemon exits cleanly
/// on `daemon.shutdown`, and both service units only restart it after a
/// failure.
pub fn stop() -> CliResult<Report> {
    let Some(status) = probe()? else {
        return Ok(Report::new(
            json!({ "stopped": false, "was_running": false }),
            "tome daemon is not running",
        ));
    };
    match rpc::call(&paths::socket_path(), "daemon.shutdown", json!({})) {
        Ok(_) => {}
        // The daemon may exit before we read the reply; that still counts.
        Err(e) if e.kind == ErrorKind::Internal => {}
        Err(e) => return Err(e),
    }

    let deadline = Instant::now() + STOP_TIMEOUT;
    // A probe that lands while the daemon is exiting sees its connection
    // dropped mid-request; that means "still stopping", not a failure.
    while !matches!(probe(), Ok(None)) {
        if Instant::now() >= deadline {
            return Err(CliError::internal("the daemon did not stop in time"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let human = format!("tome daemon stopped (was pid {})", status["pid"]);
    Ok(Report::new(
        json!({ "stopped": true, "was_running": true, "pid": status["pid"] }),
        human,
    ))
}
