//! Remote workers (`tome worker spawn --node`), with a fake `ssh`
//! (`TOME_SSH`) that runs the command on this machine as another tome home:
//! `mini` is a second [`Env`] and `down` can't be reached.

mod common;

use common::{eventually, Env};
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Stdio;

struct Nodes {
    here: Env,
    mini: Env,
}

impl Nodes {
    /// `here` runs `build` and knows `mini` and `down`; both have a project
    /// at `~/p`. Both daemons are started.
    fn new() -> Nodes {
        let mut here = Env::new();
        let mini = Env::new();
        for env in [&here, &mini] {
            fs::create_dir_all(env.project().join(".tome/workflows")).unwrap();
        }
        fs::write(
            here.project().join(".tome/workflows/build.md"),
            "---\nname: build\nmode: orchestrated\n---\n## Build\nBuild it.\n",
        )
        .unwrap();
        let mut vars = String::new();
        for (k, v) in &mini.vars {
            vars.push_str(&format!("{k}='{v}' "));
        }
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
        let tome = env!("CARGO_BIN_EXE_tome");
        here.set_config(&format!(
            "{}nodes:\n  mini:\n    ssh: me@mini\n    tome: {tome}\n  down:\n    ssh: down\n    tome: {tome}\n",
            common::IDLE_CONFIG
        ));
        mini.start_daemon();
        here.start_daemon();
        Nodes { here, mini }
    }

    fn start(&self) -> String {
        let (code, run) = self.here.json(&["run", "build", "--detach"]);
        assert_eq!(code, 0, "{run}");
        run["id"].to_string()
    }

    /// A tome command as the run's orchestrator, `--json`.
    fn tome(&self, run: &str, args: &[&str]) -> (i32, Value) {
        let mut full = vec!["--json"];
        full.extend_from_slice(args);
        let out = self
            .here
            .cmd(&full)
            .env("TOME_RUN_ID", run)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        let value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
            panic!(
                "not JSON ({e}): {stdout}\nstderr: {}",
                String::from_utf8_lossy(&out.stderr)
            )
        });
        (out.status.code().unwrap(), value)
    }

    fn ok(&self, run: &str, args: &[&str]) -> Value {
        let (code, v) = self.tome(run, args);
        assert_eq!(code, 0, "tome {args:?}: {v}");
        v
    }

    /// The hosted sessions on mini.
    fn hosted_sessions(&self) -> Vec<String> {
        let out = self.mini.tmux(&["ls", "-F", "#{session_name}"]);
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| l.starts_with("tome-hosted-"))
            .map(str::to_string)
            .collect()
    }
}

#[test]
fn command_workers_run_on_a_node_and_report_back() {
    let n = Nodes::new();
    let run = n.start();
    let w = n.ok(
        &run,
        &[
            "worker",
            "spawn",
            "--node",
            "mini",
            "--name",
            "ok",
            "--",
            "sh",
            "-c",
            "echo hi from mini",
        ],
    );
    assert_eq!(w["node"], "mini", "{w}");
    n.ok(
        &run,
        &[
            "worker",
            "spawn",
            "--node",
            "mini",
            "--name",
            "bad",
            "--",
            "sh",
            "-c",
            "echo boom; exit 7",
        ],
    );
    // A worker's own tome calls go up the link.
    n.ok(
        &run,
        &[
            "worker",
            "spawn",
            "--node",
            "mini",
            "--name",
            "rep",
            "--",
            "sh",
            "-c",
            "tome worker done --summary relayed; sleep 30",
        ],
    );

    let w = n.ok(&run, &["worker", "wait", "ok"]);
    assert_eq!(w["status"], "done", "{w}");
    assert!(
        w["summary"].as_str().unwrap().contains("hi from mini"),
        "{w}"
    );
    let w = n.ok(&run, &["worker", "wait", "bad"]);
    assert_eq!(
        (w["status"].as_str(), w["exit_code"].as_i64()),
        (Some("failed"), Some(7)),
        "{w}"
    );
    assert!(w["summary"].as_str().unwrap().contains("boom"), "{w}");
    let w = n.ok(&run, &["worker", "wait", "rep"]);
    assert_eq!(
        (w["status"].as_str(), w["summary"].as_str()),
        (Some("done"), Some("relayed")),
        "{w}"
    );

    // Ended workers' sessions are closed on the node.
    eventually("hosted sessions closed", || n.hosted_sessions().is_empty());
    let pong = n.mini.rpc_ok("daemon.ping", serde_json::json!({}));
    assert!(
        pong["capabilities"].to_string().contains("hosted_workers"),
        "{pong}"
    );
}

#[test]
fn bad_nodes_spawn_nothing() {
    let n = Nodes::new();
    let run = n.start();
    let (code, err) = n.tome(&run, &["worker", "spawn", "--node", "nope", "--", "true"]);
    assert_eq!(code, 2, "{err}");
    let (code, err) = n.tome(&run, &["worker", "spawn", "--node", "down", "--", "true"]);
    assert_eq!(code, 3, "{err}");
    assert!(err.to_string().contains("can't take workers"), "{err}");
    let status = n.ok(&run, &["worker", "status"]);
    assert_eq!(
        status["workers"].as_array().map(Vec::len),
        Some(0),
        "{status}"
    );
}

#[test]
fn cancelling_the_run_closes_hosted_sessions() {
    let n = Nodes::new();
    let run = n.start();
    n.ok(
        &run,
        &[
            "worker", "spawn", "--node", "mini", "--name", "s", "--", "sleep", "30",
        ],
    );
    eventually("the hosted session starts", || {
        n.hosted_sessions().len() == 1
    });
    let (code, out) = n.here.json(&["run", "cancel", &run]);
    assert_eq!(code, 0, "{out}");
    eventually("the hosted session closes", || {
        n.hosted_sessions().is_empty()
    });
}

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Make `here`'s project a git repo with one commit, and mini's a clone.
fn git_projects(n: &Nodes) {
    let here = n.here.project();
    git(&here, &["init", "-q", "-b", "main"]);
    git(&here, &["add", "-A"]);
    git(&here, &["commit", "-qm", "init"]);
    let mini = n.mini.project();
    fs::remove_dir_all(&mini).unwrap();
    git(
        n.mini.dir.path(),
        &[
            "clone",
            "-q",
            &here.to_string_lossy(),
            &mini.to_string_lossy(),
        ],
    );
}

#[test]
fn remote_workers_get_a_worktree_and_their_branch_comes_back() {
    let n = Nodes::new();
    git_projects(&n);
    // Uncommitted changes stay here.
    fs::write(n.here.project().join("dirty.txt"), "x").unwrap();
    let run = n.start();
    let w = n.ok(
        &run,
        &[
            "worker", "spawn", "--node", "mini", "--name", "edit", "--", "sh", "-c",
            "test ! -e dirty.txt && echo hi > made.txt && git add made.txt && git -c user.name=t -c user.email=t@t commit -qm made",
        ],
    );
    assert!(w["worktree"].as_str().unwrap().starts_with("mini:"), "{w}");
    let w = n.ok(&run, &["worker", "wait", "edit"]);
    assert_eq!(w["status"], "done", "{w}");
    let branch = format!("tome/{run}/edit");
    let here = n.here.project();
    eventually("the branch is fetched back", || {
        std::process::Command::new("git")
            .arg("-C")
            .arg(&here)
            .args(["cat-file", "-e", &format!("{branch}:made.txt")])
            .status()
            .is_ok_and(|s| s.success())
    });
    let shown = n.here.json(&["runs", "show", &run]).1;
    assert!(shown.to_string().contains("uncommitted changes"), "{shown}");
}

#[test]
fn a_node_copy_that_isnt_a_repo_spawns_nothing() {
    let n = Nodes::new();
    let here = n.here.project();
    git(&here, &["init", "-q", "-b", "main"]);
    git(&here, &["add", "-A"]);
    git(&here, &["commit", "-qm", "init"]);
    let run = n.start();
    let (code, err) = n.tome(&run, &["worker", "spawn", "--node", "mini", "--", "true"]);
    assert_eq!(code, 4, "{err}");
    assert!(err.to_string().contains("nodes.mini.projects"), "{err}");
}

#[test]
fn agent_workers_run_with_the_nodes_harness() {
    let n = Nodes::new();
    // Only mini knows `far`.
    n.mini.set_config(&format!(
        "{}  far: [sh, \"-c\", \"grep -q fix-it {{{{prompt_file}}}} && tome worker done --summary from-far; sleep 30\"]\n",
        common::IDLE_CONFIG
    ));
    let run = n.start();
    n.ok(
        &run,
        &[
            "worker",
            "spawn",
            "--node",
            "mini",
            "--name",
            "a",
            "--harness",
            "far",
            "--prompt",
            "fix-it",
        ],
    );
    let w = n.ok(&run, &["worker", "wait", "a"]);
    assert_eq!(
        (
            w["status"].as_str(),
            w["summary"].as_str(),
            w["kind"].as_str()
        ),
        (Some("done"), Some("from-far"), Some("agent")),
        "{w}"
    );

    let (code, err) = n.tome(
        &run,
        &[
            "worker",
            "spawn",
            "--node",
            "mini",
            "--harness",
            "nope",
            "--prompt",
            "x",
        ],
    );
    assert_eq!(code, 2, "{err}");
    assert!(err.to_string().contains("on this node"), "{err}");
}

#[test]
fn workflows_set_a_default_node_per_worker() {
    let n = Nodes::new();
    fs::write(
        n.here.project().join(".tome/workflows/build.md"),
        "---\nname: build\nmode: orchestrated\ndefaults:\n  layout:\n    workers:\n      node: mini\n      rules:\n        - match: \"here-*\"\n          node: local\n---\n## Build\nBuild it.\n",
    )
    .unwrap();
    let run = n.start();
    let w = n.ok(&run, &["worker", "spawn", "--name", "far", "--", "true"]);
    assert_eq!(w["node"], "mini", "{w}");
    let w = n.ok(&run, &["worker", "spawn", "--name", "here-1", "--", "true"]);
    assert!(w["node"].is_null(), "{w}");
    let w = n.ok(
        &run,
        &[
            "worker", "spawn", "--name", "x", "--node", "local", "--", "true",
        ],
    );
    assert!(w["node"].is_null(), "{w}");

    fs::write(
        n.here.project().join(".tome/workflows/odd.md"),
        "---\nname: odd\nmode: orchestrated\ndefaults:\n  layout:\n    workers:\n      node: nowhere\n---\n## Go\nGo.\n",
    )
    .unwrap();
    let (_, out) = n.here.json(&["validate", "odd"]);
    assert!(out.to_string().contains("node `nowhere`"), "{out}");
}

#[test]
fn a_lost_link_fails_the_nodes_workers() {
    let n = Nodes::new();
    let run = n.start();
    n.ok(
        &run,
        &[
            "worker", "spawn", "--node", "mini", "--name", "s", "--", "sleep", "30",
        ],
    );
    let (code, out) = n.mini.json(&["daemon", "stop"]);
    assert_eq!(code, 0, "{out}");
    let w = n.ok(&run, &["worker", "wait", "s"]);
    assert_eq!(
        (
            w["status"].as_str(),
            w["exit_reason"].as_str().or(w["reason"].as_str())
        ),
        (Some("failed"), Some("node_unreachable")),
        "{w}"
    );
}

#[test]
fn gc_on_the_node_removes_hosted_workers_and_their_branches() {
    let n = Nodes::new();
    git_projects(&n);
    let run = n.start();
    n.ok(
        &run,
        &[
            "worker", "spawn", "--node", "mini", "--name", "g", "--", "true",
        ],
    );
    let w = n.ok(&run, &["worker", "wait", "g"]);
    assert_eq!(w["status"], "done", "{w}");
    let branch = format!("tome/{run}/g");
    let mini = n.mini.project();
    eventually("the hosted session is closed", || {
        n.hosted_sessions().is_empty()
    });
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let (code, out) = n.mini.json(&["gc", "--older-than", "1s"]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(out["hosted"].as_array().map(Vec::len), Some(1), "{out}");
    let branches = git(&mini, &["branch", "--list", &branch]);
    assert!(branches.is_empty(), "{branches}");
    assert!(!git(&mini, &["worktree", "list"]).contains("hosted-"));
}
