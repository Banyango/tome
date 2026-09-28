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

/// A cmux env with no `TOME_LAYOUT`, so the config decides.
fn layout_env(layout: &str) -> Option<Env> {
    let mut env = Env::cmux()?;
    env.vars.retain(|(k, _)| k != "TOME_LAYOUT");
    // The orchestrator writes the first line typed into it to `got.txt`.
    let script = env.home().join("reader.sh");
    fs::write(&script, format!("read line; echo \"$line\" > {}; sleep 60\n", env.home().join("got.txt").display())).unwrap();
    env.set_config(&format!("harnesses:\n  reader: [sh, \"{}\"]\nlayout: {layout}\n", script.display()));
    write_wf(&env, "build", "defaults:\n  harness: reader\n");
    Some(env)
}

/// A workspace's panes, left to right: `(pane, [(surface, title)])`.
fn panes_of(workspace: &str) -> Vec<(String, Vec<(String, String)>)> {
    let out = cmux(&["--id-format", "both", "tree", "--workspace", workspace, "--json"]);
    let tree: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    let ws = tree["windows"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|w| w["workspaces"].as_array().into_iter().flatten())
        .find(|w| w["id"] == workspace)
        .cloned()
        .unwrap_or(Value::Null);
    ws["panes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| {
            let surfaces = p["surfaces"].as_array().into_iter().flatten();
            let surfaces = surfaces.map(|s| (s["id"].as_str().unwrap_or("").to_string(), s["title"].as_str().unwrap_or("").to_string()));
            (p["id"].as_str().unwrap_or("").to_string(), surfaces.collect())
        })
        .collect()
}

fn titles(panes: &[(String, Vec<(String, String)>)]) -> Vec<Vec<String>> {
    panes.iter().map(|(_, s)| s.iter().map(|(_, t)| t.clone()).collect()).collect()
}

/// Spawn a command worker as run 1's orchestrator.
fn spawn(env: &Env, name: &str, argv: &[&str]) {
    let mut args = vec!["--json", "worker", "spawn", "--name", name, "--"];
    args.extend_from_slice(argv);
    let out = env.cmd(&args).env("TOME_RUN_ID", "1").stdin(Stdio::null()).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stdout));
}

fn got(env: &Env) -> String {
    fs::read_to_string(env.home().join("got.txt")).unwrap_or_default()
}

#[test]
fn tab_layout_opens_sessions_as_tabs_of_one_split_in_the_tome_workspace() {
    let Some(env) = layout_env("tab") else { return };
    env.start_daemon();
    env.json(&["run", "build", "--detach"]);
    let s = session(&env);
    assert_eq!(s["layout"], "tab", "{s}");
    let ws = workspace(&env);
    let surface = s["pane"].as_str().unwrap().to_string();
    assert_eq!(
        s["attach"],
        format!("cmux select-workspace --workspace {ws} && cmux focus-panel --panel {surface} --workspace {ws}")
    );
    let places: Value = serde_json::from_slice(&fs::read(env.home().join("workspaces.json")).unwrap()).unwrap();
    assert_eq!(places[0]["id"], ws.as_str(), "{places}");

    // The shell on the left, the tabs split on its right.
    eventually("the orchestrator's tab", || titles(&panes_of(&ws)).get(1).is_some_and(|t| t == &["tome: build #1"]));
    spawn(&env, "w1", &["sleep", "60"]);
    let panes = panes_of(&ws);
    assert_eq!(panes.len(), 2, "{panes:?}");
    assert_eq!(titles(&panes)[1], ["tome: build #1", "tome: build #1 / w1"]);

    // The orchestrator is on a terminal of its own, and nudges reach it.
    spawn(&env, "w2", &["true"]);
    eventually("the nudge", || got(&env).contains("[tome] worker w2 done"));
    eventually("w2's tab closed", || titles(&panes_of(&ws))[1].len() == 2);

    // Closing the tabs by hand ends just those sessions, and the split goes.
    let (_, tabs) = panes_of(&ws)[1].clone();
    for (id, _) in &tabs {
        assert!(cmux(&["close-surface", "--workspace", &ws, "--surface", id]).status.success());
    }
    eventually("run to fail", || status(&env)["reason"] == "orchestrator_exited");
    eventually("the split is gone", || panes_of(&ws).len() == 1);

    // The next run brings the split back, in the same workspace.
    env.json(&["run", "build", "--detach"]);
    let (_, shown) = env.json(&["runs", "show", "2"]);
    assert_eq!(shown["sessions"][0]["handle"], ws.as_str(), "{shown}");
    eventually("run 2's tab", || titles(&panes_of(&ws)).get(1).is_some_and(|t| t == &["tome: build #2"]));

    // A restart kills the sessions but leaves the workspace.
    let (_, daemon) = env.json(&["daemon", "status"]);
    unsafe { libc::kill(daemon["pid"].as_i64().unwrap() as i32, libc::SIGKILL) };
    eventually("daemon to die", || env.json(&["daemon", "status"]).0 == 3);
    env.start_daemon();
    eventually("the split is gone again", || panes_of(&ws).len() == 1);
    assert!(cmux_has_workspace(&ws));
}

#[test]
fn split_layout_opens_each_session_in_its_own_split_to_the_right() {
    let Some(env) = layout_env("split") else { return };
    env.start_daemon();
    env.json(&["run", "build", "--detach"]);
    assert_eq!(session(&env)["layout"], "split");
    let ws = workspace(&env);
    spawn(&env, "w1", &["sleep", "60"]);
    spawn(&env, "w2", &["sleep", "60"]);
    let panes = panes_of(&ws);
    let titles: Vec<String> = titles(&panes).into_iter().skip(1).flatten().collect();
    assert_eq!(titles, ["tome: build #1", "tome: build #1 / w1", "tome: build #1 / w2"], "{panes:?}");

    // Nudges reach the orchestrator's pane.
    spawn(&env, "w3", &["true"]);
    eventually("the nudge", || got(&env).contains("[tome] worker w3 done"));
    eventually("w3's pane closed", || panes_of(&ws).len() == 4);

    assert_eq!(env.json(&["run", "cancel", "1"]).0, 0);
    eventually("only the shell is left", || panes_of(&ws).len() == 1);
    assert!(cmux_has_workspace(&ws));
}

fn spawn_with(env: &Env, name: &str, flags: &[&str], argv: &[&str]) {
    let mut args = vec!["--json", "worker", "spawn", "--name", name];
    args.extend_from_slice(flags);
    args.push("--");
    args.extend_from_slice(argv);
    let out = env.cmd(&args).env("TOME_RUN_ID", "1").stdin(Stdio::null()).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stdout));
}

fn worker_session(env: &Env, name: &str) -> Value {
    let (_, shown) = env.json(&["runs", "show", "1"]);
    shown["sessions"].as_array().unwrap().iter().find(|s| s["name"] == format!("tome-1-build-{name}")).cloned().unwrap()
}

#[test]
fn a_tab_with_from_joins_the_anchor_pane_and_size_is_best_effort() {
    let Some(env) = layout_env("split") else { return };
    env.start_daemon();
    env.json(&["run", "build", "--detach"]);
    let ws = workspace(&env);
    let orch = session(&env)["pane"].as_str().unwrap().to_string();
    // A tab next to the orchestrator: in its pane.
    spawn_with(&env, "t", &["--layout", "tab", "--from", "orchestrator"], &["sleep", "60"]);
    let panes = panes_of(&ws);
    let with_orch = panes.iter().find(|(_, s)| s.iter().any(|(id, _)| *id == orch)).unwrap();
    assert!(with_orch.1.iter().any(|(_, t)| t == "tome: build #1 / t"), "{panes:?}");

    // A sized split below: sized, or skipped with a warning when cmux has
    // no geometry for a workspace that hasn't been shown.
    spawn_with(&env, "d", &["--direction", "down", "--size", "10"], &["sleep", "60"]);
    let d = worker_session(&env, "d");
    assert_eq!(d["placement"]["direction"], "down", "{d}");
    assert_eq!(panes_of(&ws).len(), 3, "{:?}", panes_of(&ws));
    let warnings = d["placement"]["warnings"].to_string();
    assert!(d["placement"]["warnings"].is_null() || warnings.contains("split.size 10"), "{d}");
    assert_eq!(env.json(&["run", "cancel", "1"]).0, 0);
}
