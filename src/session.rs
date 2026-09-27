//! Sessions on tmux: named, visible terminals that agents run in. The user can
//! `tmux attach -t <name>` to watch one and type into it.
//!
//! tome uses the default tmux server, or the one named by `TOME_TMUX_SOCKET`
//! (passed as `tmux -L`), which tests use to stay isolated.

use crate::harness::shell_quote;
use crate::output::{CliError, CliResult};
use std::fs;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

pub const BACKEND: &str = "tmux";

/// How long a launched command waits for its output to be captured before
/// starting anyway.
const CAPTURE_WAIT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Default)]
pub struct Tmux {
    /// `tmux -L <socket>`; `None` is the default server.
    pub socket: Option<String>,
}

/// What to run in a new session.
pub struct Launch<'a> {
    pub name: &'a str,
    /// Window title, e.g. `tome: build #42`.
    pub title: &'a str,
    pub cwd: &'a Path,
    pub argv: &'a [String],
    pub env: &'a [(String, String)],
    /// Where to write the launcher script.
    pub script: &'a Path,
    /// Where to append the session's output.
    pub log: &'a Path,
}

impl Tmux {
    pub fn from_env() -> Tmux {
        Tmux { socket: std::env::var("TOME_TMUX_SOCKET").ok().filter(|s| !s.is_empty()) }
    }

    fn cmd(&self) -> Command {
        let mut cmd = Command::new("tmux");
        if let Some(socket) = &self.socket {
            cmd.args(["-L", socket]);
        }
        // Inside tmux, `$TMUX` would point commands at the caller's server.
        cmd.env_remove("TMUX").stdin(Stdio::null());
        cmd
    }

    fn run(&self, args: &[&str]) -> std::io::Result<Output> {
        self.cmd().args(args).output()
    }

    /// Whether tmux is installed.
    pub fn available() -> bool {
        Command::new("tmux").arg("-V").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
    }

    /// Start `launch.argv` in a new detached session with `launch.env` set,
    /// capturing its output to `launch.log`.
    pub fn launch(&self, launch: &Launch) -> CliResult<()> {
        if !Tmux::available() {
            return Err(CliError::internal("tmux isn't installed").with_hint("install tmux to run workflows"));
        }
        if self.is_alive(launch.name) {
            return Err(CliError::internal(format!("tmux session `{}` already exists", launch.name)));
        }
        let ready = launch.script.with_extension("ready");
        let _ = fs::remove_file(&ready);
        fs::write(launch.script, script(launch, &ready))?;

        let cwd = launch.cwd.to_string_lossy();
        let script = launch.script.to_string_lossy();
        let out = self.run(&[
            "new-session", "-d", "-s", launch.name, "-n", launch.title, "-c", &cwd, "-x", "200", "-y", "50", "sh", &script,
        ])?;
        if !out.status.success() {
            return Err(CliError::internal(format!(
                "tmux couldn't start session `{}`: {}",
                launch.name,
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        let window = format!("={}:", launch.name);
        // Keep the title; agents like to set their own.
        let _ = self.run(&["set-option", "-w", "-t", &window, "automatic-rename", "off"]);
        let _ = self.run(&["set-option", "-w", "-t", &window, "allow-rename", "off"]);
        let pipe = format!("cat >> {}", shell_quote(&launch.log.to_string_lossy()));
        let _ = self.run(&["pipe-pane", "-t", &window, "-o", &pipe]);
        // Let the command start now that its output is being captured.
        fs::write(&ready, "")?;
        Ok(())
    }

    /// Whether the session exists and its command is still running.
    pub fn is_alive(&self, name: &str) -> bool {
        match self.run(&["list-panes", "-s", "-t", &format!("={name}"), "-F", "#{pane_dead}"]) {
            // With `remain-on-exit` a finished pane lingers; it's dead then.
            Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).lines().any(|l| l.trim() == "0"),
            _ => false,
        }
    }

    /// Kill a session. Returns whether it existed.
    pub fn kill(&self, name: &str) -> bool {
        self.run(&["kill-session", "-t", &format!("={name}")]).is_ok_and(|o| o.status.success())
    }

    /// Names of all sessions on the server.
    pub fn list(&self) -> Vec<String> {
        match self.run(&["list-sessions", "-F", "#{session_name}"]) {
            Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).lines().map(str::to_string).collect(),
            _ => Vec::new(),
        }
    }

    /// Kill every session whose name starts with `prefix`; returns their names.
    pub fn kill_prefix(&self, prefix: &str) -> Vec<String> {
        self.list().into_iter().filter(|n| n.starts_with(prefix) && self.kill(n)).collect()
    }
}

/// The launcher: set the environment, wait until output capture is attached,
/// then become the command.
fn script(launch: &Launch, ready: &Path) -> String {
    let mut s = String::from("#!/bin/sh\n");
    for (k, v) in launch.env {
        s.push_str(&format!("export {k}={}\n", shell_quote(v)));
    }
    let tries = CAPTURE_WAIT.as_millis() / 50;
    s.push_str(&format!(
        "i=0; while [ ! -e {} ] && [ $i -lt {tries} ]; do sleep 0.05; i=$((i+1)); done\n",
        shell_quote(&ready.to_string_lossy())
    ));
    let argv: Vec<String> = launch.argv.iter().map(|a| shell_quote(a)).collect();
    s.push_str(&format!("exec {}\n", argv.join(" ")));
    s
}

/// A session name for a run: `tome-<id>-<workflow>`. tmux doesn't allow `.`
/// or `:` in names, so anything but letters, digits, `_` and `-` becomes `-`.
pub fn run_session_name(run_id: i64, workflow: &str, role: &str) -> String {
    let slug: String =
        workflow.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '-' }).collect();
    match role {
        "orchestrator" => format!("{}{slug}", run_prefix(run_id)),
        role => format!("{}{slug}-{role}", run_prefix(run_id)),
    }
}

/// Every session of a run starts with this.
pub fn run_prefix(run_id: i64) -> String {
    format!("tome-{run_id}-")
}

/// Poll until `f` holds or `limit` passes.
pub fn wait_until(limit: Duration, mut f: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + limit;
    loop {
        if f() {
            return true;
        }
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A private tmux server, killed on drop.
    struct Server(Tmux);

    impl Server {
        fn new(tag: &str) -> Option<Server> {
            if !Tmux::available() {
                eprintln!("skipping: tmux isn't installed");
                return None;
            }
            Some(Server(Tmux { socket: Some(format!("tome-test-{tag}-{}", std::process::id())) }))
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            let _ = self.0.run(&["kill-server"]);
        }
    }

    fn launch(tmux: &Tmux, dir: &Path, name: &str, argv: &[&str]) {
        let argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
        let env = vec![("TOME_RUN_ID".to_string(), "7".to_string()), ("GREETING".to_string(), "it's me".to_string())];
        tmux.launch(&Launch {
            name,
            title: "tome: test #7",
            cwd: dir,
            argv: &argv,
            env: &env,
            script: &dir.join(format!("{name}.sh")),
            log: &dir.join(format!("{name}.log")),
        })
        .unwrap();
    }

    #[test]
    fn launched_command_gets_env_cwd_and_its_output_is_captured() {
        let Some(server) = Server::new("env") else { return };
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path().canonicalize().unwrap();
        launch(&server.0, &dir, "tome-7-env", &["sh", "-c", "echo \"run=$TOME_RUN_ID $GREETING in $(pwd)\"; sleep 0.3"]);
        let log = dir.join("tome-7-env.log");
        let want = format!("run=7 it's me in {}", dir.display());
        assert!(
            wait_until(Duration::from_secs(5), || fs::read_to_string(&log).is_ok_and(|s| s.contains(&want))),
            "log: {:?}",
            fs::read_to_string(&log)
        );
        // Exits on its own and the session goes away.
        assert!(wait_until(Duration::from_secs(5), || !server.0.is_alive("tome-7-env")));
    }

    #[test]
    fn sessions_can_be_listed_and_killed_by_run() {
        let Some(server) = Server::new("kill") else { return };
        let tmux = &server.0;
        let dir = tempfile::tempdir().unwrap();
        launch(tmux, dir.path(), "tome-7-build", &["sleep", "30"]);
        launch(tmux, dir.path(), "tome-7-build-worker", &["sleep", "30"]);
        launch(tmux, dir.path(), "tome-70-build", &["sleep", "30"]);
        assert!(tmux.is_alive("tome-7-build"));
        assert!(!tmux.is_alive("tome-7-buil"), "names match exactly");

        let mut killed = tmux.kill_prefix(&run_prefix(7));
        killed.sort();
        assert_eq!(killed, ["tome-7-build", "tome-7-build-worker"]);
        assert!(!tmux.is_alive("tome-7-build"));
        assert!(tmux.is_alive("tome-70-build"));
        assert!(tmux.kill("tome-70-build"));
        assert!(!tmux.kill("tome-70-build"));
    }

    #[test]
    fn dead_pane_with_remain_on_exit_is_not_alive() {
        let Some(server) = Server::new("remain") else { return };
        let tmux = &server.0;
        let dir = tempfile::tempdir().unwrap();
        launch(tmux, dir.path(), "tome-8-keep", &["sleep", "30"]);
        tmux.run(&["set-option", "-g", "remain-on-exit", "on"]).unwrap();
        tmux.run(&["set-option", "-t", "=tome-8-keep", "remain-on-exit", "on"]).unwrap();
        assert!(tmux.is_alive("tome-8-keep"));
        tmux.run(&["send-keys", "-t", "=tome-8-keep:", "C-c"]).unwrap();
        assert!(wait_until(Duration::from_secs(5), || !tmux.is_alive("tome-8-keep")));
        assert!(tmux.list().contains(&"tome-8-keep".to_string()), "pane lingers");
    }

    #[test]
    fn run_session_names_are_tmux_safe() {
        assert_eq!(run_session_name(42, "review.loop:v2", "orchestrator"), "tome-42-review-loop-v2");
        assert_eq!(run_session_name(42, "build", "worker"), "tome-42-build-worker");
        assert!(run_session_name(42, "build", "orchestrator").starts_with(&run_prefix(42)));
    }
}
