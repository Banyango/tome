//! Single-agent runs (`mode: single`, the default): one `agent` session does
//! the whole workflow, with no orchestrator and no workers.

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

/// A scripted agent: a shell script run as `sh <script> <prompt_file>`.
fn stub_harness(env: &Env, name: &str, script: &str) {
    let path = env.home().join(format!("{name}.sh"));
    fs::write(&path, script).unwrap();
    env.set_config(&format!(
        "{}  {name}: [sh, \"{}\", \"{{{{prompt_file}}}}\"]\n",
        common::IDLE_CONFIG,
        path.display()
    ));
}

/// A tome command as the run's agent (`TOME_RUN_ID` set), `--json`.
fn tome(env: &Env, run: &str, args: &[&str]) -> (i32, Value) {
    let mut full = vec!["--json"];
    full.extend_from_slice(args);
    let out = env
        .cmd(&full)
        .env("TOME_RUN_ID", run)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let value =
        serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}"));
    (out.status.code().unwrap(), value)
}

fn show(env: &Env, id: &str) -> Value {
    let (code, shown) = env.json(&["runs", "show", id]);
    assert_eq!(code, 0, "{shown}");
    shown
}

const AGENT_STUB: &str = r#"
grep -q "You are the agent of a tome workflow run." "$1" || { echo "not the agent prompt" >&2; exit 3; }
cp "$1" "$TOME_HOME/seen-prompt.md"
tome step start Build >/dev/null
tome step done -m built >/dev/null
tome run finish --status succeeded --summary "agent shipped" >/dev/null
"#;

#[test]
fn a_single_run_is_one_agent_doing_the_workflow() {
    let env = Env::new();
    stub_harness(&env, "stub", AGENT_STUB);
    write_wf(
        &env,
        "build",
        "---\nname: build\ndefaults:\n  harness: stub\n---\n## Build\nBuild it.\n",
    );
    env.start_daemon();

    let out = env
        .cmd(&["run", "build"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(text.contains("run 1 running (build, agent)"), "{text}");
    assert!(text.contains("run 1 succeeded: agent shipped"), "{text}");

    let prompt = fs::read_to_string(env.home().join("seen-prompt.md")).unwrap();
    assert!(prompt.contains("Build it."), "{prompt}");
    assert!(!prompt.contains("tome worker spawn"), "{prompt}");

    let shown = show(&env, "1");
    assert_eq!(shown["run"]["mode"], "single", "{shown}");
    let sessions = shown["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1, "{shown}");
    assert_eq!(sessions[0]["name"], "tome-1-build-agent");
    assert_eq!(sessions[0]["role"], "agent");
    assert_eq!(sessions[0]["harness"], "stub");

    let human = String::from_utf8_lossy(&env.run(&["runs", "show", "1"]).stdout).to_string();
    assert!(human.contains("single"), "{human}");
}

#[test]
fn single_runs_refuse_workers_groups_and_worktrees() {
    let env = Env::new();
    write_wf(&env, "build", "---\nname: build\n---\n## Build\nGo.\n");
    env.start_daemon();
    let (code, run) = env.json(&["run", "build", "--detach"]);
    assert_eq!(code, 0, "{run}");
    let run = run["id"].to_string();
    assert!(env.has_session("tome-1-build-agent"));

    for (args, what) in [
        (&["worker", "spawn", "--", "true"][..], "`tome worker`"),
        (&["worker", "status"][..], "`tome worker`"),
        (&["group", "create", "g"][..], "`tome group`"),
        (&["worktree", "create", "w"][..], "`tome worktree create`"),
    ] {
        let (code, err) = tome(&env, &run, args);
        assert_eq!(code, 2, "{args:?}: {err}");
        let msg = err["error"]["message"].as_str().unwrap();
        assert!(msg.contains(what) && msg.contains("single-agent"), "{msg}");
        assert_eq!(
            err["error"]["hint"],
            "set `mode: orchestrated` in the workflow's frontmatter"
        );
    }

    // Queues and steps still work.
    let (code, v) = tome(&env, &run, &["queue", "push", "notes", "hi"]);
    assert_eq!(code, 0, "{v}");
    let (code, v) = tome(&env, &run, &["step", "start", "Build"]);
    assert_eq!(code, 0, "{v}");
    let (code, v) = tome(&env, &run, &["run", "finish", "--status", "succeeded"]);
    assert_eq!(code, 0, "{v}");
}

#[test]
fn an_agent_exiting_without_finishing_fails_the_run() {
    let env = Env::new();
    stub_harness(
        &env,
        "quitter",
        "tome step start Build >/dev/null\nexit 0\n",
    );
    write_wf(
        &env,
        "build",
        "---\nname: build\ndefaults:\n  harness: quitter\n---\n## Build\nGo.\n",
    );
    env.start_daemon();
    env.json(&["run", "build", "--detach"]);
    eventually("run to fail", || {
        show(&env, "1")["run"]["status"] == "failed"
    });
    assert_eq!(show(&env, "1")["run"]["reason"], "agent_exited");
}

#[test]
fn the_agent_uses_the_plain_defaults_not_the_orchestrator_ones() {
    let env = Env::new();
    stub_harness(&env, "stub", AGENT_STUB);
    write_wf(
        &env,
        "build",
        "---\nname: build\ndefaults:\n  harness: stub\n  orchestrator_harness: nope\n---\n## Build\nGo.\n",
    );
    env.start_daemon();
    let (code, run) = env.json(&["run", "build", "--detach"]);
    assert_eq!(code, 0, "{run}");
    eventually("run to succeed", || {
        show(&env, "1")["run"]["status"] == "succeeded"
    });
}

#[test]
fn validate_checks_the_mode() {
    let env = Env::new();
    write_wf(&env, "bad", "---\nname: bad\nmode: solo\n---\n## Go\nGo.\n");
    let (code, v) = env.json(&["validate", "bad"]);
    assert_eq!(code, 2, "{v}");
    let msg = v["workflows"][0]["errors"][0]["message"].as_str().unwrap();
    assert!(msg.contains("`single` or `orchestrated`"), "{msg}");

    write_wf(
        &env,
        "fan",
        "---\nname: fan\ndefaults:\n  orchestrator_model: opus\n  layout:\n    workers:\n      layout: tab\n---\n## Go\nSpawn a worker per file.\n",
    );
    let (code, v) = env.json(&["validate", "fan"]);
    assert_eq!(code, 0, "warnings don't fail validation: {v}");
    let warnings: Vec<String> = v["workflows"][0]["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["message"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(warnings.len(), 3, "{warnings:?}");
    assert!(
        warnings.iter().any(|w| w.contains("orchestrator_model")),
        "{warnings:?}"
    );
    assert!(
        warnings.iter().any(|w| w.contains("workers")),
        "{warnings:?}"
    );
    assert!(
        warnings.iter().any(|w| w.contains("mode: orchestrated")),
        "{warnings:?}"
    );

    // Orchestrated workflows don't get them.
    write_wf(
        &env,
        "fan2",
        "---\nname: fan2\nmode: orchestrated\ndefaults:\n  orchestrator_model: opus\n---\n## Go\nSpawn a worker per file.\n",
    );
    let (code, v) = env.json(&["validate", "fan2"]);
    assert_eq!(code, 0, "{v}");
    assert!(
        v["workflows"][0]["warnings"]
            .as_array()
            .is_none_or(|w| w.is_empty()),
        "{v}"
    );
}
