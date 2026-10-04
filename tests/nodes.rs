//! Remote nodes, with a fake `ssh` (`TOME_SSH`) that runs the command on
//! this machine as another tome home: `mini` is a second [`Env`], `down`
//! can't be reached and `locked` refuses the key.

mod common;

use common::{eventually, Env};
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Output;

struct Nodes {
    here: Env,
    mini: Env,
}

fn write_wf(env: &Env, name: &str, frontmatter: &str) {
    let dir = env.project().join(".tome/workflows");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join(format!("{name}.md")),
        format!("---\nname: {name}\n{frontmatter}---\n## Go\nGo.\n"),
    )
    .unwrap();
}

impl Nodes {
    /// `here` knows `mini`, `down` and `locked`; both have a project at
    /// `~/p`.
    fn new() -> Nodes {
        let mut here = Env::new();
        let mini = Env::new();
        for env in [&here, &mini] {
            fs::create_dir_all(env.project().join(".tome/workflows")).unwrap();
        }
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
  locked) echo "me@locked: Permission denied (publickey)." >&2; exit 255 ;;
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
            "{}nodes:\n  mini:\n    ssh: me@mini\n    tome: {tome}\n  down:\n    ssh: down\n    tome: {tome}\n  locked:\n    ssh: locked\n    tome: {tome}\n",
            common::IDLE_CONFIG
        ));
        Nodes { here, mini }
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn a_node_is_checked_listed_and_reached() {
    let n = Nodes::new();
    let out = n.here.run(&["node", "check", "mini"]);
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("✓"), "{text}");
    assert!(!text.contains("✗"), "{text}");

    // An unreachable node: `ls` still exits 0.
    let (code, v) = n.here.json(&["node", "ls"]);
    assert_eq!(code, 0, "{v}");
    let reach = |name: &str| -> Value {
        v["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["node"] == name)
            .unwrap_or_else(|| panic!("{name} in {v}"))
            .clone()
    };
    assert_eq!(reach("mini")["reachable"], true, "{v}");
    assert_eq!(reach("down")["reachable"], false, "{v}");

    // Daemon commands go to the node; its daemon was started on demand.
    let out = n.here.run(&["daemon", "status", "--on", "mini"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("on mini"), "{}", stdout(&out));
    assert_eq!(n.mini.json(&["daemon", "status"]).0, 0);
}

#[test]
fn an_unreachable_node_exits_3_with_a_hint() {
    let n = Nodes::new();
    let out = n.here.run(&["runs", "list", "--on", "down"]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("tome node check down"),
        "{}",
        stderr(&out)
    );

    let out = n.here.run(&["node", "check", "locked"]);
    assert_eq!(out.status.code(), Some(3), "{}", stdout(&out));
    assert!(stdout(&out).contains("✗"), "{}", stdout(&out));
    assert!(stdout(&out).contains("fix:"), "{}", stdout(&out));
}

#[test]
fn local_commands_and_unknown_nodes_refuse_on() {
    let n = Nodes::new();
    let out = n.here.run(&["validate", "--on", "mini"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("ssh me@mini tome"),
        "{}",
        stderr(&out)
    );

    let out = n.here.run(&["ready", "--on", "mini"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));

    let out = n.here.run(&["runs", "list", "--on", "nope"]);
    assert_ne!(out.status.code(), Some(0), "{}", stderr(&out));

    // A run reference that disagrees with --on.
    let out = n.here.run(&["runs", "show", "down:3", "--on", "mini"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));

    // TOME_NODE alone leaves local-only commands here.
    let out = n
        .here
        .cmd(&["validate"])
        .env("TOME_NODE", "mini")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
}

#[test]
fn a_run_starts_on_a_node_by_workflow_name() {
    let n = Nodes::new();
    write_wf(&n.mini, "build", "mode: single\n");
    let out = n.here.run(&["run", "build", "--detach", "--on", "mini"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let err = stderr(&out);
    let node_path = n.mini.project().canonicalize().unwrap();
    assert!(
        err.contains(&format!(
            "{}",
            node_path.join(".tome/workflows/build.md").display()
        )),
        "the path the node used: {err}"
    );

    // It's the node's run, reachable as mini:<id>.
    let (code, runs) = n.mini.json(&["runs", "list"]);
    assert_eq!(code, 0);
    let id = runs["runs"][0]["id"].as_i64().unwrap();
    let (code, shown) = n.here.json(&["runs", "show", &format!("mini:{id}")]);
    assert_eq!(code, 0, "{shown}");
    assert_eq!(shown["run"]["workflow_name"], "build");

    // Listing everywhere: a NODE column, an unreachable node only warns.
    let out = n.here.run(&["runs", "list", "--nodes"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(stdout(&out).contains("NODE"), "{}", stdout(&out));
    assert!(stdout(&out).contains("mini"), "{}", stdout(&out));
    assert!(stderr(&out).contains("down"), "{}", stderr(&out));

    let out = n.here.run(&["run", "cancel", &format!("mini:{id}")]);
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn a_remote_run_refuses_paths_and_names_missing_workflows() {
    let n = Nodes::new();
    let out = n
        .here
        .run(&["run", ".tome/workflows/build.md", "--on", "mini"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));

    // Here but not on the node: a hint to commit and pull.
    write_wf(&n.here, "deploy", "mode: single\n");
    let out = n.here.run(&["run", "deploy", "--detach", "--on", "mini"]);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    assert!(stderr(&out).contains("on mini"), "{}", stderr(&out));
}

#[test]
fn a_project_missing_on_the_node_exits_4() {
    let n = Nodes::new();
    fs::remove_dir_all(n.mini.project().join(".tome")).unwrap();
    let out = n.here.run(&["events", "ls", "--on", "mini"]);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("nodes.mini.projects"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn publishing_on_a_node_names_this_machine_as_the_sender() {
    let n = Nodes::new();
    write_wf(
        &n.mini,
        "deploy",
        "mode: single\ntriggers:\n  - on: build.done\n",
    );
    n.mini.start_daemon();
    assert_eq!(n.mini.json(&["triggers", "disable"]).0, 0);

    let (code, v) = n
        .here
        .json(&["publish", "build.done", "hi", "--on", "mini"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["delivered_to"][0], "deploy", "{v}");

    let (code, shown) = n.mini.json(&["events", "show", "build.done"]);
    assert_eq!(code, 0, "{shown}");
    let sender = shown["events"][0]["sender"].as_str().unwrap();
    assert!(
        sender.starts_with("node ") && sender.ends_with(" (user)"),
        "{sender}"
    );
}

#[test]
fn events_are_forwarded_to_nodes_and_wait_for_unreachable_ones() {
    let n = Nodes::new();
    write_wf(
        &n.mini,
        "deploy",
        "mode: single\ntriggers:\n  - on: build.done\n",
    );
    n.mini.start_daemon();
    assert_eq!(n.mini.json(&["triggers", "disable"]).0, 0);
    fs::write(
        n.here.project().join(".tome/config.yaml"),
        "bus:\n  forward:\n    - topic: build.*\n      to: [mini, down]\n    - topic: build.done\n      to: ghost\n",
    )
    .unwrap();

    // validate warns about the node this machine doesn't know.
    let out = n.here.run(&["validate"]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert!(stdout(&out).contains("ghost"), "{}", stdout(&out));

    let out = n
        .here
        .cmd(&["daemon", "start"])
        .env("TOME_TRIGGER_TICK_MS", "100")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let (code, v) = n.here.json(&["publish", "build.done", "{\"ok\":true}"]);
    assert_eq!(code, 0, "{v}");
    let mut to: Vec<&str> = v["delivered_to"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.as_str().unwrap())
        .collect();
    to.sort();
    assert_eq!(to, ["→ down", "→ ghost", "→ mini"], "{v}");

    let state = |wf: &str| -> String {
        let (_, shown) = n.here.json(&["events", "show", "build.done", "--all"]);
        shown["events"][0]["deliveries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["workflow"] == wf)
            .map(|d| d["state"].as_str().unwrap().to_string())
            .unwrap_or_default()
    };
    eventually("the delivery to mini is sent", || state("→ mini") == "done");
    assert_eq!(state("→ ghost"), "failed", "an unknown node fails at once");
    assert_eq!(state("→ down"), "pending", "an unreachable node waits");

    let (code, shown) = n.mini.json(&["events", "show", "build.done"]);
    assert_eq!(code, 0, "{shown}");
    let e = &shown["events"][0];
    assert!(e["sender"].as_str().unwrap().ends_with(" (user)"), "{e}");
    assert_eq!(e["deliveries"][0]["workflow"], "deploy", "{e}");

    // A forward delivery is named by its node.
    let (code, v) = n.here.json(&[
        "events",
        "remove",
        &v["event"]["id"].to_string(),
        "--workflow",
        "down",
    ]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(state("→ down"), "dropped");
}
