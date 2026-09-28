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

#[test]
fn ready_needs_an_agent() {
    let env = Env::new();
    env.start_daemon();
    let (code, out) = env.json(&["ready"]);
    assert_eq!(code, 2, "{out}");
    assert!(out["error"]["message"].as_str().unwrap().contains("TOME_RUN_ID"), "{out}");
}
