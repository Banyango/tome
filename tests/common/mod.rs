#![allow(dead_code)]

use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// An isolated tome home (and project dir) for one test. Stops any daemon it
/// started when dropped.
///
/// Sessions go to a private tmux server, or with [`Env::cmux`] to the cmux
/// app the tests run in; either way desktop notifications are off unless a
/// test sets `TOME_NOTIFY` in `vars`. The start handshake is off too (the
/// idle harness never calls tome) unless a test sets `TOME_START_TIMEOUT`.
pub struct Env {
    pub dir: tempfile::TempDir,
    /// `TOME_BACKEND` for tome commands; empty leaves it unset (auto).
    pub backend: &'static str,
    /// Extra environment for every tome command (and so the daemon).
    pub vars: Vec<(String, String)>,
}

impl Env {
    pub fn new() -> Env {
        Env::with_backend("tmux")
    }

    /// An env whose sessions are cmux workspaces, or `None` (with a note)
    /// when cmux isn't reachable or `TOME_SKIP_CMUX` is set.
    pub fn cmux() -> Option<Env> {
        if std::env::var_os("TOME_SKIP_CMUX").is_some() {
            eprintln!("skipping: TOME_SKIP_CMUX is set");
            return None;
        }
        if !cmux(&["ping"]).status.success() {
            eprintln!("skipping: cmux isn't reachable (run the tests from a cmux terminal)");
            return None;
        }
        Some(Env::with_backend("cmux"))
    }

    fn with_backend(backend: &'static str) -> Env {
        // Keep paths short: Unix socket paths are limited to ~104 bytes.
        let dir = tempfile::Builder::new().prefix("tm").tempdir_in("/tmp").unwrap();
        let vars = vec![("TOME_NOTIFY".into(), "off".into()), ("TOME_START_TIMEOUT".into(), "off".into())];
        let env = Env { dir, backend, vars };
        std::fs::create_dir_all(env.project()).unwrap();
        std::fs::create_dir_all(env.home()).unwrap();
        // Never launch a real agent: the default harness idles until killed.
        env.set_config(IDLE_CONFIG);
        env
    }

    /// Set an environment variable for every tome command (and the daemon),
    /// replacing any earlier value.
    pub fn set_var(&mut self, key: &str, value: &str) {
        self.vars.retain(|(k, _)| k != key);
        self.vars.push((key.into(), value.into()));
    }

    /// Replace `~/.tome/config.yaml`.
    pub fn set_config(&self, yaml: &str) {
        std::fs::write(self.home().join("config.yaml"), yaml).unwrap();
    }

    /// This env's private tmux server (`tmux -L <name>`).
    pub fn tmux_socket(&self) -> String {
        format!("tome-{}", self.dir.path().file_name().unwrap().to_string_lossy())
    }

    pub fn tmux(&self, args: &[&str]) -> Output {
        Command::new("tmux").args(["-L", &self.tmux_socket()]).args(args).env_remove("TMUX").output().unwrap()
    }

    /// The cmux workspace ids recorded for this env's runs.
    pub fn cmux_workspaces(&self) -> Vec<String> {
        // Lenient: this runs in `drop`, where a panic would abort.
        let json = |args: &[&str]| -> Value {
            let mut full = vec!["--json"];
            full.extend_from_slice(args);
            serde_json::from_slice(&self.run(&full).stdout).unwrap_or(Value::Null)
        };
        let runs = json(&["runs", "list"]);
        let ids: Vec<i64> = runs["runs"].as_array().into_iter().flatten().filter_map(|r| r["id"].as_i64()).collect();
        ids.iter()
            .flat_map(|id| {
                let shown = json(&["runs", "show", &id.to_string()]);
                let handles: Vec<String> = shown["sessions"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|s| s["handle"].as_str().map(str::to_string))
                    .collect();
                handles
            })
            .collect()
    }

    /// Whether a tmux session exists on this env's server.
    pub fn has_session(&self, name: &str) -> bool {
        self.tmux(&["has-session", "-t", &format!("={name}")]).status.success()
    }

    pub fn home(&self) -> PathBuf {
        self.dir.path().join("h")
    }

    pub fn project(&self) -> PathBuf {
        self.dir.path().join("p")
    }

    pub fn cmd(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_tome"));
        cmd.args(args)
            .env("TOME_HOME", self.home())
            .env("HOME", self.dir.path())
            .env_remove("TOME_OUTPUT")
            .env_remove("TOME_RUN_ID")
            .env_remove("TMUX")
            .env("TOME_TMUX_SOCKET", self.tmux_socket())
            .env_remove("TOME_BACKEND")
            .envs(Some(("TOME_BACKEND", self.backend)).filter(|(_, b)| !b.is_empty()))
            .envs(self.vars.iter().map(|(k, v)| (k, v)))
            .current_dir(self.project());
        cmd
    }

    pub fn run(&self, args: &[&str]) -> Output {
        self.cmd(args).output().unwrap()
    }

    /// Run with `--json` and parse stdout.
    pub fn json(&self, args: &[&str]) -> (i32, Value) {
        let mut full = vec!["--json"];
        full.extend_from_slice(args);
        let out = self.run(&full);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
            panic!(
                "stdout is not JSON ({e}): {stdout}\nstderr: {}",
                String::from_utf8_lossy(&out.stderr)
            )
        });
        (out.status.code().unwrap(), value)
    }

    pub fn start_daemon(&self) {
        let (code, v) = self.json(&["daemon", "start"]);
        assert_eq!(code, 0, "daemon start failed: {v}");
    }

    pub fn socket(&self) -> PathBuf {
        self.home().join("tome.sock")
    }

    /// Send one raw JSON-RPC request to the daemon and return the response.
    pub fn rpc(&self, method: &str, params: Value) -> Value {
        rpc_at(&self.socket(), method, params)
    }

    pub fn rpc_ok(&self, method: &str, params: Value) -> Value {
        let resp = self.rpc(method, params);
        assert!(resp.get("error").is_none(), "rpc {method} failed: {resp}");
        resp["result"].clone()
    }
}

pub fn rpc_at(socket: &Path, method: &str, params: Value) -> Value {
    let mut stream = UnixStream::connect(socket).unwrap();
    let req = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    writeln!(stream, "{req}").unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

/// Overrides the `claude` preset, which every workflow without a harness uses.
pub const IDLE_CONFIG: &str = "harnesses:\n  claude: [sleep, \"600\"]\n";

/// Poll until `f` holds; panics after 10s.
pub fn eventually(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !f() {
        assert!(std::time::Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        // cmux is the user's app: close whatever this env opened there.
        if self.backend != "tmux" {
            for id in self.cmux_workspaces() {
                cmux(&["close-workspace", "--workspace", &id]);
            }
        }
        if self.socket().exists() {
            let _ = self.run(&["daemon", "stop"]);
        }
        let _ = self.tmux(&["kill-server"]);
    }
}

/// Run the cmux CLI.
pub fn cmux(args: &[&str]) -> Output {
    Command::new("cmux").args(args).env("CMUX_QUIET", "1").output().unwrap()
}

/// Whether a cmux workspace with this id is open.
pub fn cmux_has_workspace(id: &str) -> bool {
    let out = cmux(&["tree", "--all", "--json"]);
    let tree: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    tree["windows"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|w| w["workspaces"].as_array().into_iter().flatten())
        .any(|w| w["id"] == id)
}
