mod common;

use common::{eventually, Env};
use serde_json::Value;
use std::fs;
use std::process::Stdio;

fn write_wf(env: &Env, name: &str, src: &str) {
    let dir = env.project().join(".tome/workflows");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{name}.md")), src).unwrap();
}

/// A scripted orchestrator: a shell script run as `sh <script> <prompt_file>`.
fn stub_harness(env: &Env, name: &str, script: &str) {
    let path = env.home().join(format!("{name}.sh"));
    fs::write(&path, script).unwrap();
    env.set_config(&format!(
        "{}  {name}: [sh, \"{}\", \"{{{{prompt_file}}}}\"]\n",
        common::IDLE_CONFIG,
        path.display()
    ));
}

/// Run attached with `--json`; returns the exit code and the NDJSON events.
fn run_attached(env: &Env, args: &[&str]) -> (i32, Vec<Value>) {
    let mut full = vec!["--json", "run"];
    full.extend_from_slice(args);
    let out = env.cmd(&full).stdin(Stdio::null()).output().unwrap();
    let events = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON ({e}): {l}")))
        .collect();
    (out.status.code().unwrap(), events)
}

fn summary(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .map(|e| match e["type"].as_str() {
            Some("step") => format!("{} {}", e["step"].as_str().unwrap(), e["event"].as_str().unwrap()),
            _ => format!("run {}", e["status"].as_str().unwrap()),
        })
        .collect()
}

const DRIVING_STUB: &str = r#"
grep -q "tome run finish --status succeeded" "$1" || { echo "no command reference" >&2; exit 3; }
cp "$1" "$TOME_HOME/seen-prompt.md"
echo "run=$TOME_RUN_ID output=$TOME_OUTPUT cwd=$(pwd)" > "$TOME_HOME/seen-env.txt"
tome step start Build >/dev/null
tome step done -m compiled >/dev/null
tome step start Ship >/dev/null
tome step done >/dev/null
tome run finish --status succeeded --summary "stub shipped" >/dev/null
"#;

#[test]
fn stub_orchestrator_drives_the_run_to_success() {
    let env = Env::new();
    stub_harness(&env, "stub", DRIVING_STUB);
    write_wf(
        &env,
        "build",
        "---\nname: build\nparams:\n  target: {default: web}\ndefaults:\n  harness: claude\n  orchestrator_harness: stub\norchestrator: Keep step messages under five words.\n---\n## Build\nBuild {{params.target}} for run {{run.id}}.\n\n## Ship\nShip it.\n",
    );
    env.start_daemon();

    let (code, events) = run_attached(&env, &["build", "--param", "target=api"]);
    assert_eq!(code, 0, "{events:?}");
    assert_eq!(
        summary(&events),
        ["run running", "Build start", "Build done", "Ship start", "Ship done", "run succeeded"]
    );
    assert_eq!(events.last().unwrap()["summary"], "stub shipped");

    // The bootstrap: built-in prompt, run details, extra instructions, resolved body.
    let prompt = fs::read_to_string(env.home().join("seen-prompt.md")).unwrap();
    assert!(prompt.starts_with("You are the orchestrator of a tome workflow run."), "{prompt}");
    assert!(prompt.contains("Run #1 of workflow `build`"), "{prompt}");
    assert!(prompt.contains("- target = api"), "{prompt}");
    assert!(prompt.contains("Keep step messages under five words."), "{prompt}");
    assert!(prompt.contains("Build api for run 1."), "{prompt}");

    let seen = fs::read_to_string(env.home().join("seen-env.txt")).unwrap();
    let project = env.project().canonicalize().unwrap();
    assert_eq!(seen.trim(), format!("run=1 output=json cwd={}", project.display()));

    // The session was recorded (with the harness it ran) and its output logged.
    let (_, shown) = env.json(&["runs", "show", "1"]);
    let sessions = shown["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1, "{shown}");
    assert_eq!(sessions[0]["name"], "tome-1-build");
    assert_eq!(sessions[0]["role"], "orchestrator");
    assert_eq!(sessions[0]["harness"], "stub");
    assert!(shown["logs"].as_array().unwrap().iter().any(|l| l["step"] == "orchestrator"), "{shown}");
}

#[test]
fn orchestrator_exiting_early_fails_the_run() {
    let env = Env::new();
    stub_harness(&env, "quitter", "tome step start Build >/dev/null\nexit 0\n");
    write_wf(&env, "build", "---\nname: build\ndefaults:\n  harness: quitter\n---\n## Build\nGo.\n");
    env.start_daemon();

    let (code, events) = run_attached(&env, &["build"]);
    assert_eq!(code, 1, "{events:?}");
    assert_eq!(summary(&events), ["run running", "Build start", "Build fail", "run failed"]);
    assert_eq!(events[2]["message"], "orchestrator_exited");
    assert_eq!(events[3]["reason"], "orchestrator_exited");

    // The notify hook ran (for now it writes to the daemon log).
    eventually("notify in the daemon log", || {
        fs::read_to_string(env.home().join("daemon.log"))
            .is_ok_and(|log| log.contains("notify: run 1 (build) failed: orchestrator_exited"))
    });
}

#[test]
fn cancel_kills_the_session_and_keeps_the_run_quiet() {
    let env = Env::new();
    write_wf(&env, "build", "---\nname: build\n---\n## Build\nGo.\n");
    env.start_daemon();
    let (code, run) = env.json(&["run", "build", "--detach"]);
    assert_eq!(code, 0, "{run}");
    assert!(env.has_session("tome-1-build"));
    // A worker session of the same run goes too.
    env.tmux(&["new-session", "-d", "-s", "tome-1-build-worker", "sleep", "600"]);

    let (code, run) = env.json(&["run", "cancel", "1"]);
    assert_eq!(code, 0, "{run}");
    assert_eq!(run["status"], "cancelled");
    assert!(!env.has_session("tome-1-build"));
    assert!(!env.has_session("tome-1-build-worker"));

    // Still cancelled after the monitor has had a look, and nobody was notified.
    std::thread::sleep(std::time::Duration::from_millis(700));
    let (_, shown) = env.json(&["runs", "show", "1"]);
    assert_eq!(shown["run"]["status"], "cancelled");
    let log = fs::read_to_string(env.home().join("daemon.log")).unwrap();
    assert!(!log.contains("notify:"), "{log}");
}

#[test]
fn closing_the_orchestrator_pane_fails_the_run() {
    let env = Env::new();
    write_wf(&env, "build", "---\nname: build\n---\n## Build\nGo.\n");
    env.start_daemon();
    env.json(&["run", "build", "--detach"]);
    env.tmux(&["kill-session", "-t", "=tome-1-build"]);
    eventually("run to fail", || env.json(&["runs", "show", "1"]).1["run"]["status"] == "failed");
    let (_, shown) = env.json(&["runs", "show", "1"]);
    assert_eq!(shown["run"]["reason"], "orchestrator_exited");
}

#[test]
fn unknown_harness_is_refused_without_a_run() {
    let env = Env::new();
    write_wf(&env, "build", "---\nname: build\ndefaults:\n  harness: nope\n---\n## Build\nGo.\n");
    env.start_daemon();
    let (code, err) = env.json(&["run", "build", "--detach"]);
    assert_eq!(code, 2, "{err}");
    assert!(err["error"]["message"].as_str().unwrap().contains("unknown harness `nope`"), "{err}");
    let (_, runs) = env.json(&["runs", "list"]);
    assert_eq!(runs["runs"].as_array().unwrap().len(), 0);
}

#[test]
fn unknown_backend_is_refused_without_a_run() {
    let env = Env::new();
    write_wf(&env, "build", "---\nname: build\ndefaults:\n  backend: screen\n---\n## Build\nGo.\n");
    env.start_daemon();
    let (code, err) = env.json(&["run", "build", "--detach"]);
    assert_eq!(code, 2, "{err}");
    assert!(err["error"]["message"].as_str().unwrap().contains("unknown session backend `screen`"), "{err}");
    let (_, runs) = env.json(&["runs", "list"]);
    assert_eq!(runs["runs"].as_array().unwrap().len(), 0);
}

#[test]
fn daemon_restart_kills_orphaned_sessions() {
    let env = Env::new();
    write_wf(&env, "build", "---\nname: build\n---\n## Build\nGo.\n");
    env.start_daemon();
    env.json(&["run", "build", "--detach"]);
    assert!(env.has_session("tome-1-build"));
    // Kill the daemon without letting it clean up.
    let (_, status) = env.json(&["daemon", "status"]);
    unsafe { libc::kill(status["pid"].as_i64().unwrap() as i32, libc::SIGKILL) };
    eventually("daemon to die", || env.json(&["daemon", "status"]).0 == 3);

    env.start_daemon();
    assert!(!env.has_session("tome-1-build"));
    let (_, shown) = env.json(&["runs", "show", "1"]);
    assert_eq!(shown["run"]["reason"], "daemon_restart");
}

/// Opt-in: drives a real Claude Code orchestrator. Needs `claude` on PATH and
/// logged in; run with `TOME_SMOKE_CLAUDE=1 cargo test --test orchestrator -- --ignored`.
/// From a cmux terminal it opens in a cmux workspace ("tome: hello #1");
/// otherwise attach with the tmux command printed below to watch or unblock it.
#[test]
#[ignore]
fn smoke_real_claude_preset() {
    if std::env::var("TOME_SMOKE_CLAUDE").is_err() {
        eprintln!("skipping: set TOME_SMOKE_CLAUDE=1 to run");
        return;
    }
    let env = Env::cmux().unwrap_or_else(Env::new);
    env.set_config(""); // the built-in `claude` preset
    write_wf(
        &env,
        "hello",
        "---\nname: hello\n---\n## Greet\nReport this step, write the word hello to a file named hello.txt in the current directory, then finish the run as succeeded.\n",
    );
    env.start_daemon();
    if env.backend == "tmux" {
        eprintln!("watch with: tmux -L {} attach -t tome-1-hello", env.tmux_socket());
    }
    let (code, events) = run_attached(&env, &["hello"]);
    assert_eq!(code, 0, "{events:?}");
    assert!(summary(&events).contains(&"Greet done".to_string()), "{events:?}");
}
