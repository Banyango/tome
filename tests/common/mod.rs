#![allow(dead_code)]

use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// An isolated tome home (and project dir) for one test. Stops any daemon it
/// started when dropped.
pub struct Env {
    pub dir: tempfile::TempDir,
}

impl Env {
    pub fn new() -> Env {
        // Keep paths short: Unix socket paths are limited to ~104 bytes.
        let dir = tempfile::Builder::new().prefix("tm").tempdir_in("/tmp").unwrap();
        let env = Env { dir };
        std::fs::create_dir_all(env.project()).unwrap();
        std::fs::create_dir_all(env.home()).unwrap();
        // Never launch a real agent: the default harness idles until killed.
        env.set_config(IDLE_CONFIG);
        env
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
        if self.socket().exists() {
            let _ = self.run(&["daemon", "stop"]);
        }
        let _ = self.tmux(&["kill-server"]);
    }
}
