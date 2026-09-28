mod common;

use common::{eventually, Env};
use serde_json::Value;
use std::fs;

fn write_wf(env: &Env, name: &str, src: &str) {
    let dir = env.project().join(".tome/workflows");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{name}.md")), src).unwrap();
}

/// A scripted agent harness: `sh <script> <prompt_file>`, next to the idle
/// `claude` preset.
fn stub_harness(env: &Env, name: &str, script: &str) {
    let path = env.home().join(format!("{name}.sh"));
    fs::write(&path, script).unwrap();
    env.set_config(&format!(
        "{}  {name}: [sh, \"{}\", \"{{{{prompt_file}}}}\"]\n",
        common::IDLE_CONFIG,
        path.display()
    ));
}

/// An env whose agents must start within `timeout`.
fn env_with_timeout(timeout: &str) -> Env {
    let mut env = Env::new();
    env.set_var("TOME_START_TIMEOUT", timeout);
    env
}

fn show(env: &Env, id: &str) -> Value {
    let (code, shown) = env.json(&["runs", "show", id]);
    assert_eq!(code, 0, "{shown}");
    shown
}

/// The run's handshake states, in order, as `<agent> <state>`.
fn states(env: &Env, id: &str) -> Vec<String> {
    show(env, id)["handshake"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| format!("{} {}", h["worker"].as_str().unwrap_or("orchestrator"), h["state"].as_str().unwrap()))
        .collect()
}

fn status(env: &Env, id: &str) -> String {
    show(env, id)["run"]["status"].as_str().unwrap().to_string()
}

#[test]
fn a_ready_orchestrator_is_left_alone() {
    let env = env_with_timeout("300ms");
    stub_harness(
        &env,
        "stub",
        "grep -q 'run `tome ready`' \"$1\" || exit 3\ntome ready > \"$TOME_HOME/ready.json\"\nexec sleep 600\n",
    );
    write_wf(&env, "build", "---\nname: build\ndefaults:\n  harness: stub\n---\n## Build\nGo.\n");
    env.start_daemon();
    env.json(&["run", "build", "--detach"]);

    eventually("the ready event", || states(&env, "1") == ["orchestrator waiting", "orchestrator ready"]);
    let ready: Value = serde_json::from_str(&fs::read_to_string(env.home().join("ready.json")).unwrap()).unwrap();
    assert_eq!((ready["role"].as_str(), ready["ready"].as_bool()), (Some("orchestrator"), Some(true)), "{ready}");

    // Well past two timeouts: never nudged or failed.
    std::thread::sleep(std::time::Duration::from_millis(1200));
    assert_eq!(status(&env, "1"), "running");
    assert_eq!(states(&env, "1"), ["orchestrator waiting", "orchestrator ready"]);

    // Shown by `tome runs show`.
    let out = env.run(&["runs", "show", "1"]);
    let human = String::from_utf8_lossy(&out.stdout);
    assert!(human.contains("start:") && human.contains("orchestrator  ready"), "{human}");
}

#[test]
fn a_silent_orchestrator_is_nudged_then_failed() {
    let env = env_with_timeout("500ms");
    write_wf(&env, "build", "---\nname: build\n---\n## Build\nGo.\n");
    env.start_daemon();
    env.json(&["run", "build", "--detach"]);
    assert!(env.has_session("tome-1-build"));

    // The nudge is typed into its pane.
    eventually("the nudge", || states(&env, "1") == ["orchestrator waiting", "orchestrator nudged"]);
    let prompt_file = env.home().join("runs/1/orchestrator-prompt.md");
    let pane = String::from_utf8_lossy(&env.tmux(&["capture-pane", "-p", "-t", "=tome-1-build:"]).stdout).into_owned();
    assert!(pane.contains(&format!("tome: read {}", prompt_file.display())), "{pane}");
    assert_eq!(status(&env, "1"), "running");

    // Then, still silent, it fails.
    eventually("the run to fail", || status(&env, "1") == "failed");
    let shown = show(&env, "1");
    assert_eq!(shown["run"]["reason"], "orchestrator_no_start");
    assert_eq!(states(&env, "1"), ["orchestrator waiting", "orchestrator nudged", "orchestrator no_start"]);
    assert!(!env.has_session("tome-1-build"), "its session is killed");
    assert!(env.home().join("runs/1/orchestrator.log").exists(), "its log is kept");
    eventually("notify in the daemon log", || {
        fs::read_to_string(env.home().join("daemon.log"))
            .is_ok_and(|log| log.contains("notify: run 1 (build) failed: orchestrator_no_start"))
    });

    // A late call is refused.
    let out = env.cmd(&["--json", "ready"]).env("TOME_RUN_ID", "1").output().unwrap();
    assert_eq!(out.status.code(), Some(2), "{}", String::from_utf8_lossy(&out.stdout));
    assert!(String::from_utf8_lossy(&out.stdout).contains("isn't running (failed)"));
}

#[test]
fn a_nudged_orchestrator_that_answers_carries_on() {
    let env = env_with_timeout("400ms");
    // Only acts on what's typed into its pane; any tome call counts.
    stub_harness(
        &env,
        "late",
        "read -r line\necho \"$line\" > \"$TOME_HOME/typed.txt\"\ntome step start Build >/dev/null\nexec sleep 600\n",
    );
    write_wf(&env, "build", "---\nname: build\ndefaults:\n  harness: late\n---\n## Build\nGo.\n");
    env.start_daemon();
    env.json(&["run", "build", "--detach"]);

    eventually("the late start", || {
        states(&env, "1") == ["orchestrator waiting", "orchestrator nudged", "orchestrator ready"]
    });
    let typed = fs::read_to_string(env.home().join("typed.txt")).unwrap();
    assert!(typed.starts_with("tome: read ") && typed.trim_end().ends_with("orchestrator-prompt.md and follow it"), "{typed}");
    std::thread::sleep(std::time::Duration::from_millis(1000));
    assert_eq!(status(&env, "1"), "running");
    let history = show(&env, "1")["handshake"].clone();
    assert_eq!(history[2]["message"], "first tome call, after the nudge");
}

#[test]
fn a_queued_run_has_no_timer_until_it_starts() {
    let env = env_with_timeout("300ms");
    stub_harness(&env, "stub", "tome ready >/dev/null\nexec sleep 600\n");
    write_wf(&env, "build", "---\nname: build\nconcurrency: 1\ndefaults:\n  harness: stub\n---\n## Build\nGo.\n");
    env.start_daemon();
    env.json(&["run", "build", "--detach"]);
    let (_, queued) = env.json(&["run", "build", "--detach"]);
    assert_eq!(queued["status"], "queued");

    std::thread::sleep(std::time::Duration::from_millis(1000));
    assert_eq!(status(&env, "2"), "queued");
    assert!(states(&env, "2").is_empty());

    env.json(&["run", "cancel", "1"]);
    eventually("the queued run to start and be ready", || {
        states(&env, "2") == ["orchestrator waiting", "orchestrator ready"]
    });
}

#[test]
fn the_check_can_be_off() {
    let env = env_with_timeout("off");
    write_wf(&env, "build", "---\nname: build\n---\n## Build\nGo.\n");
    env.start_daemon();
    env.json(&["run", "build", "--detach"]);
    std::thread::sleep(std::time::Duration::from_millis(1000));
    assert_eq!(status(&env, "1"), "running");
    assert!(states(&env, "1").is_empty());
}

/// An env with no `TOME_START_TIMEOUT`, so the workflow's own setting applies.
fn env_without_override() -> Env {
    let mut env = Env::new();
    env.vars.retain(|(k, _)| k != "TOME_START_TIMEOUT");
    env
}

#[test]
fn a_workflow_can_turn_the_check_off() {
    let env = env_without_override();
    write_wf(&env, "build", "---\nname: build\ndefaults:\n  start_timeout: off\n---\n## Build\nGo.\n");
    env.start_daemon();
    env.json(&["run", "build", "--detach"]);
    std::thread::sleep(std::time::Duration::from_millis(1000));
    assert_eq!(status(&env, "1"), "running");
    assert!(states(&env, "1").is_empty());
}

#[test]
fn a_workflow_sets_its_own_timeout() {
    let env = env_without_override();
    write_wf(&env, "build", "---\nname: build\ndefaults:\n  start_timeout: 1s\n---\n## Build\nGo.\n");
    env.start_daemon();
    env.json(&["run", "build", "--detach"]);
    eventually("the nudge", || states(&env, "1") == ["orchestrator waiting", "orchestrator nudged"]);
    let waiting = show(&env, "1")["handshake"][0]["message"].clone();
    assert_eq!(waiting, "waiting 1s for a first tome call");
}

#[test]
fn the_env_var_wins_over_the_workflow() {
    let env = env_with_timeout("off");
    write_wf(&env, "build", "---\nname: build\ndefaults:\n  start_timeout: 1s\n---\n## Build\nGo.\n");
    env.start_daemon();
    env.json(&["run", "build", "--detach"]);
    std::thread::sleep(std::time::Duration::from_millis(1500));
    assert!(states(&env, "1").is_empty());
}

#[test]
fn a_bad_start_timeout_is_invalid() {
    let env = Env::new();
    write_wf(&env, "build", "---\nname: build\ndefaults:\n  start_timeout: soon\n---\n## Build\nGo.\n");
    let (code, v) = env.json(&["validate"]);
    assert_eq!(code, 2, "{v}");
    assert!(v.to_string().contains("defaults.start_timeout"), "{v}");
}

#[test]
fn ready_needs_an_agent() {
    let env = Env::new();
    env.start_daemon();
    let (code, out) = env.json(&["ready"]);
    assert_eq!(code, 2, "{out}");
    assert!(out["error"]["message"].as_str().unwrap().contains("TOME_RUN_ID"), "{out}");
}

// --- workers ----------------------------------------------------------------

/// A tome command as the run's orchestrator (`TOME_RUN_ID` set), `--json`.
fn as_orchestrator(env: &Env, run: &str, args: &[&str]) -> Value {
    let mut full = vec!["--json"];
    full.extend_from_slice(args);
    let out = env.cmd(&full).env("TOME_RUN_ID", run).output().unwrap();
    assert!(out.status.success(), "tome {args:?}: {}", String::from_utf8_lossy(&out.stdout));
    serde_json::from_slice(&out.stdout).unwrap()
}

fn worker(env: &Env, run: &str, name: &str) -> Value {
    as_orchestrator(env, run, &["worker", "status", name])
}

#[test]
fn a_silent_agent_worker_is_nudged_then_failed() {
    let env = env_with_timeout("700ms");
    write_wf(&env, "build", "---\nname: build\n---\n## Build\nGo.\n");
    env.start_daemon();
    env.json(&["run", "build", "--detach"]);
    // The orchestrator's call counts as its start.
    as_orchestrator(&env, "1", &["worker", "spawn", "--name", "w", "--prompt", "Paint the fence."]);
    assert!(env.has_session("tome-1-build-w"));

    eventually("the worker nudge", || states(&env, "1").contains(&"w nudged".to_string()));
    let pane = String::from_utf8_lossy(&env.tmux(&["capture-pane", "-p", "-t", "=tome-1-build-w:"]).stdout).into_owned();
    assert!(pane.contains("worker-w-prompt.md and follow it"), "{pane}");

    eventually("the worker to fail", || worker(&env, "1", "w")["status"] == "failed");
    let w = worker(&env, "1", "w");
    assert_eq!(w["reason"], "worker_no_start", "{w}");
    assert_eq!(
        states(&env, "1"),
        ["orchestrator waiting", "orchestrator ready", "w waiting", "w nudged", "w no_start"]
    );
    assert!(!env.has_session("tome-1-build-w"), "its session is killed");
    assert!(env.home().join("runs/1/worker-w.log").exists(), "its log is kept");

    // The run carries on, and the orchestrator hears about it; the user doesn't.
    assert_eq!(status(&env, "1"), "running");
    eventually("the orchestrator nudge", || {
        let pane = env.tmux(&["capture-pane", "-p", "-t", "=tome-1-build:"]).stdout;
        String::from_utf8_lossy(&pane).contains("[tome] worker w failed. Details: tome worker status w")
    });
    let log = fs::read_to_string(env.home().join("daemon.log")).unwrap();
    assert!(!log.contains("notify:"), "{log}");

    // A late call is refused.
    let out = env.cmd(&["--json", "ready"]).env("TOME_RUN_ID", "1").env("TOME_WORKER_ID", "w").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stdout).contains("has already finished (failed)"));
}

#[test]
fn a_ready_agent_worker_is_left_alone_and_commands_are_not_checked() {
    let env = env_with_timeout("300ms");
    stub_harness(
        &env,
        "painter",
        "grep -q 'run `tome ready`' \"$1\" || exit 3\ntome ready > \"$TOME_HOME/ready.json\"\nexec sleep 600\n",
    );
    write_wf(&env, "build", "---\nname: build\n---\n## Build\nGo.\n");
    env.start_daemon();
    env.json(&["run", "build", "--detach"]);
    as_orchestrator(&env, "1", &["worker", "spawn", "--name", "p", "--harness", "painter", "--prompt", "Paint."]);
    as_orchestrator(&env, "1", &["worker", "spawn", "--name", "c", "--", "sleep", "600"]);

    eventually("the worker's ready", || states(&env, "1").contains(&"p ready".to_string()));
    let ready: Value = serde_json::from_str(&fs::read_to_string(env.home().join("ready.json")).unwrap()).unwrap();
    assert_eq!((ready["role"].as_str(), ready["worker"].as_str()), (Some("worker"), Some("p")), "{ready}");

    std::thread::sleep(std::time::Duration::from_millis(1200));
    assert_eq!(worker(&env, "1", "p")["status"], "running");
    assert_eq!(worker(&env, "1", "c")["status"], "running");
    assert_eq!(states(&env, "1"), ["orchestrator waiting", "orchestrator ready", "p waiting", "p ready"]);
}
