//! Where a run's sessions go: `layout: tab | split | workspace`.

mod common;

use common::Env;
use serde_json::Value;
use std::fs;
use std::process::Stdio;

fn write_wf(env: &Env, name: &str, defaults: &str) {
    let dir = env.project().join(".tome/workflows");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{name}.md")), format!("---\nname: {name}\n{defaults}---\n## Build\nGo.\n")).unwrap();
}

/// An env with no `TOME_LAYOUT`, so the workflow and config decide.
fn env() -> Env {
    let mut env = Env::new();
    env.vars.retain(|(k, _)| k != "TOME_LAYOUT");
    env
}

fn sessions(env: &Env, run: i64) -> Vec<Value> {
    let (_, shown) = env.json(&["runs", "show", &run.to_string()]);
    shown["sessions"].as_array().unwrap_or_else(|| panic!("{shown}")).clone()
}

/// Spawn a command worker as run `run`'s orchestrator.
fn spawn(env: &Env, run: i64, name: &str, argv: &[&str]) -> Value {
    spawn_with(env, run, name, &[], argv)
}

fn spawn_with(env: &Env, run: i64, name: &str, flags: &[&str], argv: &[&str]) -> Value {
    let mut args = vec!["--json", "worker", "spawn", "--name", name];
    args.extend_from_slice(flags);
    args.push("--");
    args.extend_from_slice(argv);
    let out = env.cmd(&args).env("TOME_RUN_ID", run.to_string()).stdin(Stdio::null()).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_str(stdout.trim()).unwrap()
}

#[test]
fn unknown_layout_in_a_workflow_fails_validation() {
    let env = env();
    write_wf(&env, "bad", "defaults:\n  layout: tabs\n");
    let (code, v) = env.json(&["validate", "bad"]);
    assert_eq!(code, 2, "{v}");
    let err = v.to_string();
    assert!(err.contains("unknown `defaults.layout` `tabs`"), "{err}");
    assert!(err.contains("tab, split, workspace"), "{err}");
}

#[test]
fn unknown_layout_in_the_config_is_refused_with_a_hint() {
    let env = env();
    env.set_config(&format!("{}layout: sideways\n", common::IDLE_CONFIG));
    write_wf(&env, "build", "");
    env.start_daemon();
    let (code, err) = env.json(&["run", "build", "--detach"]);
    assert_eq!(code, 2, "{err}");
    let message = err["error"]["message"].as_str().unwrap();
    assert!(message.contains("unknown session layout `sideways` (from `layout` in the tome config)"), "{err}");
    assert!(err["error"]["hint"].as_str().unwrap().contains("tab, split, workspace"), "{err}");
    let (_, runs) = env.json(&["runs", "list"]);
    assert_eq!(runs["runs"].as_array().unwrap().len(), 0);
}

#[test]
fn the_workflow_layout_overrides_the_config() {
    let env = env();
    env.set_config(&format!("{}layout: sideways\n", common::IDLE_CONFIG));
    write_wf(&env, "build", "defaults:\n  layout: workspace\n");
    env.start_daemon();
    let (code, run) = env.json(&["run", "build", "--detach"]);
    assert_eq!(code, 0, "{run}");
    let s = &sessions(&env, 1)[0];
    assert_eq!(s["layout"], "workspace", "{s}");
    // Under `workspace` a run keeps its own tmux session, as before.
    assert!(env.has_session("tome-1-build"));
}

#[test]
fn the_config_layout_is_recorded_on_every_session() {
    let env = env();
    env.set_config(&format!("{}layout: workspace\n", common::IDLE_CONFIG));
    write_wf(&env, "build", "");
    env.start_daemon();
    env.json(&["run", "build", "--detach"]);
    spawn(&env, 1, "w1", &["sleep", "30"]);
    let all = sessions(&env, 1);
    assert_eq!(all.len(), 2, "{all:?}");
    assert!(all.iter().all(|s| s["layout"] == "workspace"), "{all:?}");
    assert!(env.has_session("tome-1-build-w1"));
}

/// A `reader` harness whose orchestrator writes the first line typed into
/// it to `got.txt` in the tome home, then idles.
fn reader(env: &Env, layout: &str) {
    let script = env.home().join("reader.sh");
    fs::write(&script, format!("read line; echo \"$line\" > {}; sleep 30\n", env.home().join("got.txt").display())).unwrap();
    env.set_config(&format!("{}  reader: [sh, \"{}\"]\nlayout: {layout}\n", common::IDLE_CONFIG, script.display()));
    write_wf(env, "build", "defaults:\n  harness: reader\n");
}

fn got(env: &Env) -> String {
    fs::read_to_string(env.home().join("got.txt")).unwrap_or_default()
}

#[test]
fn sessions_are_tracked_by_their_own_pane() {
    let env = env();
    reader(&env, "workspace");
    env.start_daemon();
    env.json(&["run", "build", "--detach"]);
    let s = &sessions(&env, 1)[0];
    let pane = s["pane"].as_str().unwrap_or_else(|| panic!("no pane: {s}"));
    assert!(pane.starts_with('%'), "{s}");

    // The user opens another window in the session: nudges still reach the
    // orchestrator's own pane.
    assert!(env.tmux(&["new-window", "-t", "=tome-1-build:"]).status.success());
    spawn(&env, 1, "w1", &["true"]);
    common::eventually("the nudge", || got(&env).contains("[tome] worker w1 done"));
}

fn lines(out: std::process::Output) -> Vec<String> {
    String::from_utf8_lossy(&out.stdout).lines().map(str::to_string).collect()
}

/// The project's tome session's windows: `(pane, name)`.
fn windows(env: &Env) -> Vec<(String, String)> {
    let out = env.tmux(&["list-windows", "-t", "=p-orchestrator", "-F", "#{pane_id}\t#{window_name}"]);
    lines(out).iter().filter_map(|l| l.split_once('\t')).map(|(p, n)| (p.to_string(), n.to_string())).collect()
}

/// The panes of the project's tome session's first window: `(pane, title)`.
fn panes(env: &Env) -> Vec<(String, String)> {
    let out = env.tmux(&["list-panes", "-t", "=p-orchestrator:^", "-F", "#{pane_id}\t#{pane_title}"]);
    lines(out).iter().filter_map(|l| l.split_once('\t')).map(|(p, n)| (p.to_string(), n.to_string())).collect()
}

fn named(all: &[(String, String)], name: &str) -> bool {
    all.iter().any(|(_, n)| n == name)
}

/// `tome worker wait <name>` as run `run`'s orchestrator.
fn wait(env: &Env, run: i64, name: &str) -> Value {
    let out = env.cmd(&["--json", "worker", "wait", name]).env("TOME_RUN_ID", run.to_string()).output().unwrap();
    serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap()
}

fn status(env: &Env, run: i64) -> Value {
    env.json(&["runs", "show", &run.to_string()]).1["run"].clone()
}

#[test]
fn tab_is_the_default_and_puts_sessions_in_windows_of_the_project_session() {
    let env = env();
    write_wf(&env, "build", "");
    env.start_daemon();
    env.json(&["run", "build", "--detach"]);
    let orch = &sessions(&env, 1)[0];
    assert_eq!(orch["layout"], "tab", "{orch}");
    assert!(env.has_session("p-orchestrator"));
    assert!(!env.has_session("tome-1-build"), "no session of its own");
    let pane = orch["pane"].as_str().unwrap();
    let attach = orch["attach"].as_str().unwrap();
    assert!(attach.contains(&format!(r"select-window -t {pane} \; select-pane -t {pane} \; attach -t")), "{attach}");
    common::eventually("the orchestrator's window", || named(&windows(&env), "tome: build #1"));

    // Workers, and a second run, share the one session.
    spawn(&env, 1, "w1", &["sleep", "30"]);
    env.json(&["run", "build", "--detach"]);
    let all = windows(&env);
    assert!(named(&all, "tome: build #1 / w1") && named(&all, "tome: build #2"), "{all:?}");
    assert_eq!(all.len(), 4, "the shell and three sessions: {all:?}");

    // A finished worker's window closes, unless it's kept open.
    spawn(&env, 1, "w2", &["true"]);
    spawn_with(&env, 1, "w3", &["--keep-open"], &["true"]);
    assert_eq!(wait(&env, 1, "w2")["status"], "done");
    assert_eq!(wait(&env, 1, "w3")["status"], "done");
    common::eventually("w2's window closed", || !named(&windows(&env), "tome: build #1 / w2"));
    assert!(named(&windows(&env), "tome: build #1 / w3"), "kept open");

    // Closing one window ends just that session.
    let (w1, _) = windows(&env).into_iter().find(|(_, n)| n == "tome: build #1 / w1").unwrap();
    assert!(env.tmux(&["kill-window", "-t", &w1]).status.success());
    let w = wait(&env, 1, "w1");
    assert_eq!((w["status"].as_str(), w["reason"].as_str()), (Some("failed"), Some("worker_exited")), "{w}");
    let (orch2, _) = windows(&env).into_iter().find(|(_, n)| n == "tome: build #2").unwrap();
    assert!(env.tmux(&["kill-window", "-t", &orch2]).status.success());
    common::eventually("run 2 failed", || status(&env, 2)["reason"] == "orchestrator_exited");
    assert_eq!(status(&env, 1)["status"], "running");

    // Cancelling the run closes its windows; the tome session stays.
    assert_eq!(env.json(&["run", "cancel", "1"]).0, 0);
    common::eventually("run 1's windows closed", || windows(&env).iter().all(|(_, n)| !n.starts_with("tome: build #1")));
    assert_eq!(windows(&env).len(), 1, "{:?}", windows(&env));
}

#[test]
fn split_puts_each_session_in_its_own_pane() {
    let env = env();
    env.set_config(&format!("{}layout: split\n", common::IDLE_CONFIG));
    write_wf(&env, "build", "");
    env.start_daemon();
    env.json(&["run", "build", "--detach"]);
    assert_eq!(sessions(&env, 1)[0]["layout"], "split");
    spawn(&env, 1, "w1", &["sleep", "30"]);
    spawn(&env, 1, "w2", &["sleep", "30"]);
    let all = panes(&env);
    assert_eq!(all.len(), 4, "the shell and three sessions, side by side: {all:?}");
    assert!(named(&all, "tome: build #1") && named(&all, "tome: build #1 / w2"), "{all:?}");
    assert_eq!(windows(&env).len(), 1);
    // The newest is on the right.
    let right = lines(env.tmux(&["list-panes", "-t", "=p-orchestrator:^", "-F", "#{pane_right} #{pane_title}"]));
    let rightmost = right.iter().max_by_key(|l| l.split(' ').next().unwrap().parse::<i64>().unwrap()).unwrap();
    assert!(rightmost.ends_with("tome: build #1 / w2"), "{right:?}");

    // Closing one pane ends just that session.
    let (w1, _) = all.iter().find(|(_, n)| n == "tome: build #1 / w1").unwrap();
    assert!(env.tmux(&["kill-pane", "-t", w1]).status.success());
    let w = wait(&env, 1, "w1");
    assert_eq!((w["status"].as_str(), w["reason"].as_str()), (Some("failed"), Some("worker_exited")), "{w}");

    assert_eq!(env.json(&["run", "cancel", "1"]).0, 0);
    common::eventually("only the shell is left", || panes(&env).len() == 1);
}

#[test]
fn a_restart_kills_the_sessions_but_leaves_the_tome_session() {
    let env = env();
    write_wf(&env, "build", "");
    env.start_daemon();
    env.json(&["run", "build", "--detach"]);
    spawn(&env, 1, "w1", &["sleep", "30"]);
    assert_eq!(windows(&env).len(), 3);

    let pid = env.rpc_ok("daemon.status", serde_json::json!({}))["pid"].as_i64().unwrap();
    unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    common::eventually("the daemon died", || unsafe { libc::kill(pid as i32, 0) } != 0);
    env.start_daemon();
    assert_eq!(status(&env, 1)["reason"], "daemon_restart");
    common::eventually("the sessions are gone", || windows(&env).len() == 1);
    assert!(env.has_session("p-orchestrator"));

    // The next run finds it again.
    env.json(&["run", "build", "--detach"]);
    common::eventually("run 2's window", || named(&windows(&env), "tome: build #2"));
    let out = env.tmux(&["list-sessions", "-F", "#{session_name}"]);
    assert_eq!(lines(out), ["p-orchestrator"]);
}
