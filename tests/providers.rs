//! Node providers: `tome run <workflow> --on <provider>` makes a machine with
//! a plugin, runs there, keeps a record and releases it. The "machine" is
//! `mini`, a second tome home reached through a fake `ssh` (see nodes.rs).

mod common;

use common::{eventually, Env};
use serde_json::{json, Value};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const PROVIDER: &str = r#"#!/usr/bin/env python3
import sys, json, os
data = os.environ["TOME_PLUGIN_DATA"]

def knob(name, default=""):
    try:
        return open(os.path.join(data, name)).read().strip()
    except OSError:
        return default

def send(o):
    sys.stdout.write(json.dumps(o) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    m = json.loads(line)
    method, id_ = m.get("method"), m.get("id")
    with open(os.path.join(data, "calls.jsonl"), "a") as f:
        f.write(json.dumps({"method": method, "params": m.get("params")}) + "\n")
    if id_ is None:
        continue
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": id_, "result": {"protocol": 1}})
    elif method == "provide":
        mode = knob("provide_mode", "ok")
        if mode == "hang":
            continue
        if mode == "fail":
            send({"jsonrpc": "2.0", "id": id_, "error": {"code": -32000, "message": "no capacity"}})
            continue
        send({"jsonrpc": "2.0", "method": "progress", "params": {"id": id_, "message": "booting"}})
        send({"jsonrpc": "2.0", "id": id_, "result": {
            "id": "i-1", "ssh": knob("ssh", "me@mini"), "tome": knob("tome")}})
    elif method == "release":
        if knob("release_fail"):
            send({"jsonrpc": "2.0", "id": id_, "error": {"code": -32000, "message": "api down"}})
        else:
            send({"jsonrpc": "2.0", "id": id_, "result": {}})
    else:
        send({"jsonrpc": "2.0", "id": id_, "result": {}})
"#;

struct Rig {
    here: Env,
    mini: Env,
}

fn write_wf(env: &Env, name: &str, extra: &str) {
    let dir = env.project().join(".tome/workflows");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join(format!("{name}.md")),
        format!("---\nname: {name}\nmode: single\n{extra}---\n## Go\nGo.\n"),
    )
    .unwrap();
}

impl Rig {
    fn new(provide_hook: &str) -> Rig {
        let mut here = Env::new();
        let mini = Env::new();
        for env in [&here, &mini] {
            fs::create_dir_all(env.project().join(".tome/workflows")).unwrap();
        }
        let vars: String = mini
            .vars
            .iter()
            .map(|(k, v)| format!("{k}='{v}' "))
            .collect();
        let script = format!(
            r#"#!/bin/sh
while [ $# -gt 0 ]; do
  case "$1" in
    -o) shift 2 ;;
    -t|-tt) shift ;;
    *) break ;;
  esac
done
dest="$1"; shift
case "$dest" in
  down) echo "ssh: connect to host down port 22: Connection refused" >&2; exit 255 ;;
  me@mini) ;;
  *) echo "ssh: Could not resolve hostname $dest" >&2; exit 255 ;;
esac
cd '{home}' || exit 1
exec env -u TOME_RUN_ID -u TOME_WORKER_ID -u TOME_NODE HOME='{home}' TOME_HOME='{tome}' TOME_TMUX_SOCKET='{tmux}' {vars} sh -c "$*"
"#,
            home = mini.dir.path().display(),
            tome = mini.home().display(),
            tmux = mini.tmux_socket(),
        );
        let ssh = here.dir.path().join("ssh");
        fs::write(&ssh, script).unwrap();
        fs::set_permissions(&ssh, fs::Permissions::from_mode(0o755)).unwrap();
        here.set_var("TOME_SSH", &ssh.display().to_string());
        here.set_config(&format!(
            "{}defaults:\n  provider_opts:\n    disk: 40\n    size: tiny\n",
            common::IDLE_CONFIG
        ));
        let rig = Rig { here, mini };
        rig.install(provide_hook);
        rig.knob("tome", env!("CARGO_BIN_EXE_tome"));
        write_wf(&rig.mini, "build", "");
        write_wf(
            &rig.here,
            "build",
            "defaults:\n  provider_opts:\n    size: large\n",
        );
        rig.here.start_daemon();
        rig.mini.start_daemon();
        eventually("the provider plugin to run", || rig.state() == "running");
        rig
    }

    fn install(&self, provide_hook: &str) {
        let src = self.here.dir.path().join("src/fakevm");
        fs::create_dir_all(src.join("bin")).unwrap();
        fs::write(
            src.join("plugin.yaml"),
            format!(
                "name: fakevm\nprotocol: 1\ncommand: bin/vm\nproviders: [fake-vm]\nhooks:\n  provide:{provide_hook}\n  release:\n"
            ),
        )
        .unwrap();
        fs::write(src.join("bin/vm"), PROVIDER).unwrap();
        fs::set_permissions(src.join("bin/vm"), fs::Permissions::from_mode(0o755)).unwrap();
        let (code, out) = self
            .here
            .json(&["plugin", "install", src.to_str().unwrap()]);
        assert_eq!(code, 0, "{out}");
    }

    fn state(&self) -> String {
        let (_, out) = self.here.json(&["plugin", "ls"]);
        out["plugins"][0]["state"]
            .as_str()
            .unwrap_or("")
            .to_string()
    }

    fn knob(&self, name: &str, value: &str) {
        let dir = self.here.home().join("plugins/fakevm/data");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(name), value).unwrap();
    }

    fn calls(&self) -> Vec<Value> {
        fs::read_to_string(self.here.home().join("plugins/fakevm/data/calls.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn called(&self, method: &str) -> Vec<Value> {
        self.calls()
            .into_iter()
            .filter(|c| c["method"] == method)
            .collect()
    }

    /// The newest run on the machine, once there is one.
    fn mini_run(&self) -> Option<i64> {
        let (_, v) = self.mini.json(&["runs", "list"]);
        v["runs"].as_array()?.first()?["id"].as_i64()
    }

    fn run_cmd(&self, args: &[&str]) -> Command {
        let mut full = vec!["run", "build", "--on", "fake-vm"];
        full.extend_from_slice(args);
        let mut cmd = self.here.cmd(&full);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        cmd
    }
}

fn released_dir(rig: &Rig) -> PathBuf {
    rig.here.home().join("released")
}

#[test]
fn a_run_on_a_provider_provides_runs_records_and_releases() {
    let rig = Rig::new("");
    let child = rig
        .run_cmd(&[
            "--provider-opt",
            "region=eu",
            "--provider-opt",
            "size=small",
        ])
        .spawn()
        .unwrap();
    eventually("the run on the machine", || rig.mini_run().is_some());
    let id = rig.mini_run().unwrap();
    // Registered while it lives.
    assert!(rig.here.home().join("ephemeral/fake-vm-i-1.json").is_file());
    assert!(rig.called("release").is_empty(), "not released mid-run");
    let out = rig.mini.run(&[
        "run",
        "finish",
        "--status",
        "succeeded",
        "--summary",
        "all green",
        "--run",
        &id.to_string(),
    ]);
    assert!(out.status.success());
    let out = child.wait_with_output().unwrap();
    let (stdout, stderr) = (
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert_eq!(out.status.code(), Some(0), "{stdout}\n{stderr}");
    assert!(stderr.contains("booting"), "progress is shown: {stderr}");
    assert!(stdout.contains("on fake-vm-i-1"), "{stdout}");

    // Options: command line over workflow over config.
    let provide = &rig.called("provide")[0]["params"];
    assert_eq!(
        provide["opts"],
        json!({"disk": "40", "region": "eu", "size": "small"})
    );
    assert_eq!(provide["provider"], "fake-vm");

    // Released once, after the run, with where the logs are.
    let release = rig.called("release");
    assert_eq!(release.len(), 1, "{release:?}\n{stderr}");
    let p = &release[0]["params"];
    assert_eq!(p["id"], "i-1");
    assert_eq!(p["cause"], "finished");
    assert_eq!(p["run"]["status"], "succeeded");
    assert_eq!(p["run"]["summary"], "all green");
    assert_eq!(p["logs"]["ssh"], "me@mini");
    let stub_path = p["logs"]["stub"].as_str().unwrap();
    assert!(fs::metadata(stub_path).is_ok(), "{p}");
    assert!(!rig.here.home().join("ephemeral/fake-vm-i-1.json").exists());

    // The record outlives the machine.
    assert_eq!(fs::read_dir(released_dir(&rig)).unwrap().count(), 1);
    let (code, v) = rig.here.json(&["runs", "list"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["released"][0]["provider"], "fake-vm");
    assert_eq!(v["released"][0]["status"], "succeeded");
    assert_eq!(v["released"][0]["summary"], "all green");
    let human = String::from_utf8_lossy(&rig.here.run(&["runs", "list"]).stdout).into_owned();
    assert!(human.contains("fake-vm:i-1 (released)"), "{human}");
}

#[test]
fn a_provide_that_times_out_is_released_and_exits_3() {
    let rig = Rig::new(" { timeout: 1s }");
    rig.knob("provide_mode", "hang");
    let out = rig.run_cmd(&[]).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{stderr}");
    assert!(stderr.contains("within 1s"), "{stderr}");
    eventually("release", || !rig.called("release").is_empty());
    let release = &rig.called("release")[0]["params"];
    assert_eq!(release["cause"], "provide_failed");
    assert!(release["id"].is_null());
    assert!(!released_dir(&rig).exists(), "no run, no record");
}

#[test]
fn a_machine_that_fails_its_check_is_released_with_the_fix_hint() {
    let rig = Rig::new("");
    rig.knob("ssh", "down");
    let out = rig.run_cmd(&[]).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{stderr}");
    assert!(stderr.contains("failed its check at `ssh`"), "{stderr}");
    assert!(
        stderr.contains("hint:") || stderr.contains("fix"),
        "{stderr}"
    );
    eventually("release", || !rig.called("release").is_empty());
    let release = &rig.called("release")[0]["params"];
    assert_eq!(release["cause"], "check_failed");
    assert_eq!(release["id"], "i-1");
    assert!(!rig.here.home().join("ephemeral/fake-vm-i-1.json").exists());
}

#[test]
fn a_failing_provide_is_exit_3_and_a_failing_release_is_notified_not_retried() {
    let rig = Rig::new("");
    rig.knob("provide_mode", "fail");
    let out = rig.run_cmd(&[]).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{stderr}");
    assert!(stderr.contains("no capacity"), "{stderr}");

    // A release that fails: said once, never retried, and the run's own
    // outcome is still the exit code.
    rig.knob("provide_mode", "ok");
    rig.knob("release_fail", "1");
    let before = rig.called("release").len();
    let child = rig.run_cmd(&[]).spawn().unwrap();
    eventually("the run on the machine", || rig.mini_run().is_some());
    let id = rig.mini_run().unwrap();
    rig.mini.run(&[
        "run",
        "finish",
        "--status",
        "failed",
        "--run",
        &id.to_string(),
    ]);
    let out = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("couldn't release fake-vm:i-1"), "{stderr}");
    assert!(stderr.contains("api down"), "{stderr}\n{:?}", rig.calls());
    assert_eq!(rig.called("release").len(), before + 1, "released once");
    // The record still says how it went.
    let (_, v) = rig.here.json(&["runs", "list", "--status", "failed"]);
    assert_eq!(v["released"][0]["status"], "failed", "{v}");
}

#[test]
fn keep_skips_release_and_leaves_an_ephemeral_node() {
    let rig = Rig::new("");
    let out = rig.run_cmd(&["--detach"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2), "--detach needs --keep");

    let out = rig.run_cmd(&["--detach", "--keep"]).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(stderr.contains("kept fake-vm-i-1"), "{stderr}");
    assert!(rig.called("release").is_empty());
    assert!(rig.mini_run().is_some());

    // It is a node now: reachable by name, listed, removable.
    let (code, v) = rig.here.json(&["runs", "list", "--on", "fake-vm-i-1"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["runs"].as_array().unwrap().len(), 1);
    let (_, v) = rig.here.json(&["node", "ls"]);
    assert!(v["nodes"].to_string().contains("fake-vm-i-1"), "{v}");
    let (code, _) = rig.here.json(&["node", "rm", "fake-vm-i-1"]);
    assert_eq!(code, 0);
    assert!(!rig.here.home().join("ephemeral/fake-vm-i-1.json").exists());
}

#[test]
fn provider_names_are_refused_where_they_would_clash() {
    let rig = Rig::new("");
    let (code, v) = rig.here.json(&["node", "add", "fake-vm", "me@x"]);
    assert_ne!(code, 0, "{v}");
    assert!(
        v["error"]
            .to_string()
            .contains("provider declared by plugin `fakevm`"),
        "{v}"
    );

    let (code, v) = rig.here.json(&["runs", "list", "--on", "fake-vm"]);
    assert_eq!(code, 2, "{v}");
    assert!(v["error"].to_string().contains("is a provider"), "{v}");

    let out = rig.here.run(&["run", "build", "--provider-opt", "a=b"]);
    assert_eq!(out.status.code(), Some(2));
    let out = rig
        .here
        .run(&["run", "build", "--on", "fake-vm", "--provider-opt", "nope"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
