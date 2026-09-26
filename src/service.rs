//! `tome daemon install|uninstall`: register the daemon as an OS service so
//! it's always available. A launchd agent on macOS, a systemd `--user` unit
//! on Linux.
//!
//! Both units restart the daemon only when it exits unsuccessfully, so
//! `tome daemon stop` (a clean exit) stops it, while a crash brings it back
//! (and crash recovery then fails the runs that were in progress).

use crate::lifecycle;
use crate::output::{CliError, CliResult, Report};
use crate::paths;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const LAUNCHD_LABEL: &str = "dev.tome.daemon";
pub const SYSTEMD_UNIT: &str = "tome.service";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Launchd,
    Systemd,
}

impl Platform {
    pub fn current() -> CliResult<Platform> {
        if cfg!(target_os = "macos") {
            Ok(Platform::Launchd)
        } else if cfg!(target_os = "linux") {
            Ok(Platform::Systemd)
        } else {
            Err(CliError::invalid("service install is supported on macOS (launchd) and Linux (systemd) only"))
        }
    }

    fn name(self) -> &'static str {
        match self {
            Platform::Launchd => "launchd",
            Platform::Systemd => "systemd",
        }
    }

    pub fn unit_path(self) -> PathBuf {
        match self {
            Platform::Launchd => paths::user_home().join("Library/LaunchAgents").join(format!("{LAUNCHD_LABEL}.plist")),
            Platform::Systemd => {
                let config = std::env::var_os("XDG_CONFIG_HOME")
                    .filter(|v| !v.is_empty())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| paths::user_home().join(".config"));
                config.join("systemd/user").join(SYSTEMD_UNIT)
            }
        }
    }
}

/// What goes into the unit file.
pub struct UnitSpec {
    pub exe: PathBuf,
    /// Set when `TOME_HOME` is overridden, so the service uses the same home.
    pub tome_home: Option<PathBuf>,
    /// The installing shell's PATH, so the daemon can find multiplexers and
    /// agent harnesses (launchd/systemd start services with a minimal PATH).
    pub path_env: Option<String>,
    pub log: PathBuf,
}

impl UnitSpec {
    fn from_env() -> CliResult<UnitSpec> {
        let exe = std::env::current_exe()?;
        let exe = exe.canonicalize().unwrap_or(exe);
        Ok(UnitSpec {
            exe,
            tome_home: std::env::var_os("TOME_HOME").filter(|v| !v.is_empty()).map(PathBuf::from),
            path_env: std::env::var("PATH").ok().filter(|v| !v.is_empty()),
            log: paths::daemon_log_path(),
        })
    }

    fn env_pairs(&self) -> Vec<(&'static str, String)> {
        let mut env = Vec::new();
        if let Some(p) = &self.path_env {
            env.push(("PATH", p.clone()));
        }
        if let Some(h) = &self.tome_home {
            env.push(("TOME_HOME", h.display().to_string()));
        }
        env
    }

    pub fn render(&self, platform: Platform) -> String {
        match platform {
            Platform::Launchd => self.launchd_plist(),
            Platform::Systemd => self.systemd_unit(),
        }
    }

    pub fn launchd_plist(&self) -> String {
        let env: String = self
            .env_pairs()
            .iter()
            .map(|(k, v)| format!("        <key>{k}</key>\n        <string>{}</string>\n", xml_escape(v)))
            .collect();
        let env_block = if env.is_empty() {
            String::new()
        } else {
            format!("    <key>EnvironmentVariables</key>\n    <dict>\n{env}    </dict>\n")
        };
        let log = xml_escape(&self.log.display().to_string());
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LAUNCHD_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>daemon</string>
        <string>run</string>
    </array>
{env_block}    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
    <key>ProcessType</key>
    <string>Background</string>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
</dict>
</plist>
"#,
            exe = xml_escape(&self.exe.display().to_string()),
        )
    }

    pub fn systemd_unit(&self) -> String {
        let env: String = self
            .env_pairs()
            .iter()
            .map(|(k, v)| format!("Environment=\"{k}={}\"\n", v.replace('\\', "\\\\").replace('"', "\\\"")))
            .collect();
        format!(
            "[Unit]
Description=tome agentic workflow daemon

[Service]
Type=simple
ExecStart=\"{exe}\" daemon run
{env}Restart=on-failure
RestartSec=2

[Install]
WantedBy=default.target
",
            exe = self.exe.display(),
        )
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The installed unit file, if any.
pub fn installed() -> Option<(Platform, PathBuf)> {
    let platform = Platform::current().ok()?;
    let path = platform.unit_path();
    path.exists().then_some((platform, path))
}

pub fn install(print_only: bool, no_start: bool) -> CliResult<Report> {
    let platform = Platform::current()?;
    let spec = UnitSpec::from_env()?;
    let unit_path = platform.unit_path();
    let content = spec.render(platform);

    if print_only {
        let data = json!({ "platform": platform.name(), "path": unit_path, "content": content, "installed": false });
        return Ok(Report::new(data, content));
    }

    // A daemon started by hand would hold the lock and make the service's
    // copy fail; stop it so the service owns the daemon from now on.
    let was_running = lifecycle::probe()?.is_some();
    if was_running {
        lifecycle::stop()?;
    }

    if let Some(dir) = unit_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::create_dir_all(paths::tome_home())?;
    std::fs::write(&unit_path, &content)?;

    let mut started = false;
    if !no_start {
        match platform {
            Platform::Launchd => {
                let target = launchd_domain();
                // Reload if an older copy is already loaded.
                let _ = run_quiet("launchctl", &["bootout", &format!("{target}/{LAUNCHD_LABEL}")]);
                run_checked("launchctl", &["bootstrap", &target, &unit_path.display().to_string()])?;
            }
            Platform::Systemd => {
                run_checked("systemctl", &["--user", "daemon-reload"])?;
                run_checked("systemctl", &["--user", "enable", "--now", SYSTEMD_UNIT])?;
            }
        }
        lifecycle::wait_until_up()?;
        started = true;
    }

    let human = format!(
        "installed {} service at {}{}",
        platform.name(),
        unit_path.display(),
        if started { "\ntome daemon is running" } else { "" }
    );
    let data = json!({
        "platform": platform.name(),
        "path": unit_path,
        "installed": true,
        "started": started,
        "restarted_existing_daemon": was_running,
    });
    Ok(Report::new(data, human))
}

pub fn uninstall() -> CliResult<Report> {
    let platform = Platform::current()?;
    let unit_path = platform.unit_path();
    if !unit_path.exists() {
        let data = json!({ "platform": platform.name(), "path": unit_path, "removed": false });
        return Ok(Report::new(data, "tome service is not installed"));
    }
    match platform {
        Platform::Launchd => {
            let _ = run_quiet("launchctl", &["bootout", &format!("{}/{LAUNCHD_LABEL}", launchd_domain())]);
        }
        Platform::Systemd => {
            let _ = run_quiet("systemctl", &["--user", "disable", "--now", SYSTEMD_UNIT]);
        }
    }
    std::fs::remove_file(&unit_path)?;
    if platform == Platform::Systemd {
        let _ = run_quiet("systemctl", &["--user", "daemon-reload"]);
    }
    let data = json!({ "platform": platform.name(), "path": unit_path, "removed": true });
    Ok(Report::new(data, format!("removed {} service {}", platform.name(), unit_path.display())))
}

/// Start the installed service (used by `tome daemon start`).
pub fn start_installed(platform: Platform, unit_path: &Path) -> CliResult<()> {
    match platform {
        Platform::Launchd => {
            let target = launchd_domain();
            let service = format!("{target}/{LAUNCHD_LABEL}");
            // kickstart fails if the agent isn't loaded (e.g. after bootout).
            if run_quiet("launchctl", &["kickstart", &service]).is_err() {
                run_checked("launchctl", &["bootstrap", &target, &unit_path.display().to_string()])?;
            }
        }
        Platform::Systemd => run_checked("systemctl", &["--user", "start", SYSTEMD_UNIT])?,
    }
    Ok(())
}

fn launchd_domain() -> String {
    // SAFETY: getuid has no preconditions.
    format!("gui/{}", unsafe { libc::getuid() })
}

fn run_quiet(cmd: &str, args: &[&str]) -> Result<(), String> {
    let out = Command::new(cmd).args(args).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn run_checked(cmd: &str, args: &[&str]) -> CliResult<()> {
    run_quiet(cmd, args).map_err(|e| CliError::internal(format!("`{cmd} {}` failed: {e}", args.join(" "))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> UnitSpec {
        UnitSpec {
            exe: PathBuf::from("/opt/tome & co/bin/tome"),
            tome_home: Some(PathBuf::from("/data/tome")),
            path_env: Some("/usr/local/bin:/usr/bin".into()),
            log: PathBuf::from("/data/tome/daemon.log"),
        }
    }

    #[test]
    fn launchd_plist_runs_daemon_and_restarts_only_on_failure() {
        let p = spec().launchd_plist();
        assert!(p.contains("<string>dev.tome.daemon</string>"));
        assert!(p.contains("<string>/opt/tome &amp; co/bin/tome</string>\n        <string>daemon</string>\n        <string>run</string>"));
        assert!(p.contains("<key>SuccessfulExit</key>\n        <false/>"));
        assert!(p.contains("<key>TOME_HOME</key>\n        <string>/data/tome</string>"));
        assert!(p.contains("<key>PATH</key>"));
        assert!(p.contains("<key>RunAtLoad</key>"));
    }

    #[test]
    fn systemd_unit_runs_daemon_and_restarts_on_failure() {
        let u = spec().systemd_unit();
        assert!(u.contains("ExecStart=\"/opt/tome & co/bin/tome\" daemon run\n"));
        assert!(u.contains("Restart=on-failure"));
        assert!(u.contains("Environment=\"TOME_HOME=/data/tome\""));
        assert!(u.contains("WantedBy=default.target"));
    }

    #[test]
    fn env_block_omitted_when_empty() {
        let s = UnitSpec { tome_home: None, path_env: None, ..spec() };
        assert!(!s.launchd_plist().contains("EnvironmentVariables"));
        assert!(!s.systemd_unit().contains("Environment="));
    }
}
