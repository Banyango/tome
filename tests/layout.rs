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
    let mut args = vec!["--json", "worker", "spawn", "--name", name, "--"];
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
