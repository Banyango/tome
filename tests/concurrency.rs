mod common;

use common::{eventually, Env};
use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader};
use std::process::Stdio;

fn write_wf(env: &Env, name: &str, concurrency: &str) {
    let dir = env.project().join(".tome/workflows");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join(format!("{name}.md")),
        format!("---\nname: {name}\n{concurrency}---\n## Build\nGo.\n"),
    )
    .unwrap();
}

fn detach(env: &Env, wf: &str) -> (i32, Value) {
    env.json(&["run", wf, "--detach"])
}

fn status(env: &Env, id: i64) -> Value {
    env.json(&["runs", "show", &id.to_string()]).1["run"]["status"].clone()
}

fn finish(env: &Env, id: i64) {
    let (code, out) = env.json(&[
        "run",
        "finish",
        "--status",
        "succeeded",
        "--run",
        &id.to_string(),
    ]);
    assert_eq!(code, 0, "{out}");
}

#[test]
fn over_the_limit_runs_queue_and_start_in_order() {
    let env = Env::new();
    write_wf(&env, "build", "concurrency: 1\n");
    env.start_daemon();

    assert_eq!(detach(&env, "build").1["status"], "running");
    let (code, second) = detach(&env, "build");
    assert_eq!(code, 0, "{second}");
    assert_eq!(second["status"], "queued");
    assert_eq!(detach(&env, "build").1["status"], "queued");
    // Queued runs have no orchestrator yet.
    assert!(!env.has_session("tome-2-build"));

    let out = env.cmd(&["run", "build", "--detach"]).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "run 4 queued (build)"
    );

    finish(&env, 1);
    assert_eq!(status(&env, 2), "running");
    assert_eq!(status(&env, 3), "queued");
    assert!(env.has_session("tome-2-build"));

    finish(&env, 2);
    assert_eq!(status(&env, 3), "running");
    assert_eq!(status(&env, 4), "queued");
}

#[test]
fn limit_counts_only_that_workflow() {
    let env = Env::new();
    write_wf(&env, "build", "concurrency: 2\non_conflict: queue\n");
    write_wf(&env, "lint", "");
    env.start_daemon();

    assert_eq!(detach(&env, "build").1["status"], "running");
    assert_eq!(detach(&env, "build").1["status"], "running");
    assert_eq!(detach(&env, "build").1["status"], "queued");
    // Unlimited by default.
    for _ in 0..3 {
        assert_eq!(detach(&env, "lint").1["status"], "running");
    }
    finish(&env, 4);
    assert_eq!(
        status(&env, 3),
        "queued",
        "a lint run finishing frees no build slot"
    );
    finish(&env, 2);
    assert_eq!(status(&env, 3), "running");
}

#[test]
fn over_the_limit_runs_are_rejected_with_on_conflict_reject() {
    let env = Env::new();
    write_wf(&env, "build", "concurrency: 1\non_conflict: reject\n");
    env.start_daemon();

    assert_eq!(detach(&env, "build").0, 0);
    let (code, err) = detach(&env, "build");
    assert_eq!(code, 1, "{err}");
    assert_eq!(err["error"]["kind"], "conflict");
    assert!(
        err["error"]["message"]
            .as_str()
            .unwrap()
            .contains("concurrency limit (1 of 1 running)"),
        "{err}"
    );
    let (_, runs) = env.json(&["runs", "list"]);
    assert_eq!(
        runs["runs"].as_array().unwrap().len(),
        1,
        "a rejected run isn't recorded"
    );

    // Human output explains, and attached runs are refused the same way.
    let out = env
        .cmd(&["run", "build"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("concurrency limit") && stderr.contains("on_conflict: queue"),
        "{stderr}"
    );

    finish(&env, 1);
    assert_eq!(detach(&env, "build").1["status"], "running");
}

#[test]
fn cancelled_queued_runs_are_skipped() {
    let env = Env::new();
    write_wf(&env, "build", "concurrency: 1\n");
    env.start_daemon();
    for _ in 0..3 {
        detach(&env, "build");
    }
    let (code, run) = env.json(&["run", "cancel", "2"]);
    assert_eq!(code, 0, "{run}");
    assert_eq!(run["status"], "cancelled");
    assert_eq!(
        status(&env, 1),
        "running",
        "cancelling a queued run leaves the others alone"
    );
    assert_eq!(status(&env, 3), "queued");

    finish(&env, 1);
    assert_eq!(status(&env, 2), "cancelled");
    assert_eq!(status(&env, 3), "running");
}

#[test]
fn a_run_failing_on_its_own_frees_its_slot() {
    let env = Env::new();
    write_wf(&env, "build", "concurrency: 1\n");
    env.start_daemon();
    detach(&env, "build");
    detach(&env, "build");
    env.tmux(&["kill-session", "-t", "=tome-1-build"]);
    eventually("run 2 to start", || status(&env, 2) == "running");
    assert_eq!(status(&env, 1), "failed");
    assert!(env.has_session("tome-2-build"));
}

#[test]
fn attached_queued_run_streams_until_it_starts() {
    let env = Env::new();
    write_wf(&env, "build", "concurrency: 1\n");
    env.start_daemon();
    detach(&env, "build");

    let mut child = env
        .cmd(&["--json", "run", "build"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap())
        .lines()
        .map(|l| {
            let l = l.unwrap();
            serde_json::from_str::<Value>(&l).unwrap_or_else(|e| panic!("not JSON ({e}): {l}"))
        });
    let first = lines.next().unwrap();
    assert_eq!(
        (first["run_id"].as_i64(), first["status"].as_str()),
        (Some(2), Some("queued"))
    );

    finish(&env, 1);
    assert_eq!(lines.next().unwrap()["status"], "running");
    finish(&env, 2);
    assert_eq!(lines.next().unwrap()["status"], "succeeded");
    assert_eq!(child.wait().unwrap().code(), Some(0));
}
