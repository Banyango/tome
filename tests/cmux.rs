//! Runs whose sessions are cmux workspaces. These open (and close) real,
//! unfocused workspaces in the cmux app the tests run in; they're skipped
//! outside cmux or with `TOME_SKIP_CMUX=1`.

mod common;

use common::{cmux, cmux_has_workspace, eventually, Env};
use serde_json::Value;
use std::fs;
use std::process::Stdio;

fn write_wf(env: &Env, name: &str, defaults: &str) {
    let dir = env.project().join(".tome/workflows");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{name}.md")), format!("---\nname: {name}\n{defaults}---\n## Build\nGo.\n")).unwrap();
}

/// The recorded orchestrator session of run 1.
fn session(env: &Env) -> Value {
    let (_, shown) = env.json(&["runs", "show", "1"]);
    let sessions = shown["sessions"].as_array().unwrap_or_else(|| panic!("{shown}"));
    assert_eq!(sessions.len(), 1, "{shown}");
    sessions[0].clone()
}

fn workspace(env: &Env) -> String {
    let s = session(env);
    assert_eq!(s["backend"], "cmux", "{s}");
    assert!(s["pane"].as_str().is_some_and(|p| !p.is_empty()), "no surface: {s}");
    s["handle"].as_str().unwrap_or_else(|| panic!("no handle: {s}")).to_string()
}

fn status(env: &Env) -> Value {
    env.json(&["runs", "show", "1"]).1["run"].clone()
}

const STUB: &str = r#"
if [ -t 0 ] && [ -t 1 ]; then tty=yes; else tty=no; fi
echo "run=$TOME_RUN_ID tty=$tty cwd=$(pwd)" > "$TOME_HOME/seen.txt"
echo "hello from the stub orchestrator"
tome step start Build >/dev/null
tome step done >/dev/null
tome run finish --status succeeded --summary "done in cmux" >/dev/null
"#;

#[test]
fn stub_orchestrator_drives_a_run_in_a_cmux_workspace() {
    let Some(env) = Env::cmux() else { return };
    let script = env.home().join("stub.sh");
    fs::write(&script, STUB).unwrap();
    env.set_config(&format!("harnesses:\n  stub: [sh, \"{}\"]\n", script.display()));
    write_wf(&env, "cmuxstub", "defaults:\n  harness: stub\n");
    env.start_daemon();

    let out = env.cmd(&["--json", "run", "cmuxstub"]).stdin(Stdio::null()).output().unwrap();
    let events = String::from_utf8_lossy(&out.stdout).to_string();
    assert_eq!(out.status.code(), Some(0), "{events}\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(events.contains("done in cmux"), "{events}");

    // The agent ran on a real terminal, in the project, with the run's env.
    let seen = fs::read_to_string(env.home().join("seen.txt")).unwrap();
    let project = env.project().canonicalize().unwrap();
    assert_eq!(seen.trim(), format!("run=1 tty=yes cwd={}", project.display()));

    // Its output was captured, and the workspace closed when it exited.
    let id = workspace(&env);
    let log = fs::read_to_string(env.home().join("runs/1/orchestrator.log")).unwrap();
    assert!(log.contains("hello from the stub orchestrator"), "{log}");
    eventually("workspace to close itself", || !cmux_has_workspace(&id));

    assert_eq!(session(&env)["attach"], format!("cmux select-workspace --workspace {id}"));
}

#[test]
fn cancel_closes_the_workspace() {
    let Some(env) = Env::cmux() else { return };
    write_wf(&env, "cmuxcancel", "");
    env.start_daemon();
    let (code, run) = env.json(&["run", "cmuxcancel", "--detach"]);
    assert_eq!(code, 0, "{run}");
    let id = workspace(&env);
    assert!(cmux_has_workspace(&id));

    let (code, run) = env.json(&["run", "cancel", "1"]);
    assert_eq!(code, 0, "{run}");
    assert!(!cmux_has_workspace(&id));
}

#[test]
fn closing_the_workspace_fails_the_run_and_notifies_in_cmux() {
    let Some(mut env) = Env::cmux() else { return };
    env.vars.retain(|(k, _)| k != "TOME_NOTIFY");
    write_wf(&env, "cmuxclosed", "");
    env.start_daemon();
    env.json(&["run", "cmuxclosed", "--detach"]);
    let id = workspace(&env);

    assert!(cmux(&["close-workspace", "--workspace", &id]).status.success());
    eventually("run to fail", || status(&env)["status"] == "failed");
    assert_eq!(status(&env)["reason"], "orchestrator_exited");

    let title = "tome: cmuxclosed #1 failed";
    let mut posted = None;
    eventually("a cmux notification", || {
        let list = String::from_utf8_lossy(&cmux(&["list-notifications"]).stdout).to_string();
        posted = list.lines().find(|l| l.contains(title)).map(str::to_string);
        posted.is_some()
    });
    let line = posted.unwrap();
    assert!(line.contains("orchestrator_exited"), "{line}");
    // `<n>:<id>|...`: tidy up after ourselves.
    let nid = line.split(['|', ':']).nth(1).unwrap();
    cmux(&["dismiss-notification", "--id", nid]);
}

#[test]
fn daemon_restart_closes_orphaned_workspaces() {
    let Some(env) = Env::cmux() else { return };
    write_wf(&env, "cmuxorphan", "");
    env.start_daemon();
    env.json(&["run", "cmuxorphan", "--detach"]);
    let id = workspace(&env);

    let (_, daemon) = env.json(&["daemon", "status"]);
    unsafe { libc::kill(daemon["pid"].as_i64().unwrap() as i32, libc::SIGKILL) };
    eventually("daemon to die", || env.json(&["daemon", "status"]).0 == 3);
    env.start_daemon();

    assert!(!cmux_has_workspace(&id));
    assert_eq!(status(&env)["reason"], "daemon_restart");
}

#[test]
fn inside_cmux_runs_default_to_cmux() {
    let Some(mut env) = Env::cmux() else { return };
    env.backend = "";
    write_wf(&env, "cmuxauto", "");
    env.start_daemon();
    env.json(&["run", "cmuxauto", "--detach"]);
    assert!(cmux_has_workspace(&workspace(&env)));
}

#[test]
fn workflow_backend_overrides_the_default() {
    let Some(env) = Env::cmux() else { return };
    write_wf(&env, "cmuxtmux", "defaults:\n  backend: tmux\n");
    env.start_daemon();
    env.json(&["run", "cmuxtmux", "--detach"]);
    let s = session(&env);
    assert_eq!(s["backend"], "tmux", "{s}");
    assert!(env.has_session("tome-1-cmuxtmux"));
}
