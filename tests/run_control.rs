mod common;

use common::Env;
use serde_json::json;
use std::fs;

const WF: &str = "---\nname: build\nparams:\n  base: {default: main}\n---\n## Build\nBranch off {{params.base}} as run-{{run.id}}.\n";

fn write_wf(env: &Env, name: &str, source: &str) {
    let dir = env.project().join(".tome/workflows");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{name}.md")), source).unwrap();
}

/// `tome run <wf> --detach`, returning the run id.
fn start(env: &Env, args: &[&str]) -> i64 {
    let mut full = vec!["run"];
    full.extend_from_slice(args);
    full.push("--detach");
    let (code, v) = env.json(&full);
    assert_eq!(code, 0, "{v}");
    v["id"].as_i64().unwrap()
}

#[test]
fn detached_run_records_and_returns_id() {
    let env = Env::new();
    env.start_daemon();
    write_wf(&env, "build", WF);

    let (code, v) = env.json(&["run", "build", "--param", "base=dev", "--detach"]);
    assert_eq!(code, 0, "{v}");
    let id = v["id"].as_i64().unwrap();
    assert_eq!(v["status"], "running");
    assert_eq!(v["workflow_name"], "build");
    assert_eq!(
        v["project_path"],
        json!(env.project().canonicalize().unwrap())
    );

    let (_, show) = env.json(&["runs", "show", &id.to_string(), "--snapshot"]);
    let snap = show["run"]["workflow_snapshot"].as_str().unwrap();
    assert!(
        snap.contains(&format!("Branch off dev as run-{id}.")),
        "{snap}"
    );

    // Human mode says which run started.
    let out = env.run(&["run", "build", "--detach"]);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        format!("run {} running (build)", id + 1)
    );
}

#[test]
fn invalid_workflow_exits_2_without_a_run() {
    let env = Env::new();
    env.start_daemon();
    write_wf(&env, "bad", "---\nname: bad\n---\nUse {{params.nope}}.\n");
    write_wf(
        &env,
        "req",
        "---\nname: req\nparams:\n  ticket: {type: string}\n---\n{{params.ticket}}\n",
    );

    let (code, v) = env.json(&["run", "bad", "--detach"]);
    assert_eq!(code, 2, "{v}");
    assert_eq!(v["error"]["kind"], "invalid_workflow");
    assert_eq!(v["error"]["details"]["errors"][0]["line"], 4);

    let (code, v) = env.json(&["run", "req", "--detach"]);
    assert_eq!(code, 2, "{v}");
    let (code, _) = env.json(&["run", "req", "--param", "bogus=1", "--detach"]);
    assert_eq!(code, 2);

    let (code, _) = env.json(&["run", "missing", "--detach"]);
    assert_eq!(code, 4);

    let (_, v) = env.json(&["runs", "list"]);
    assert!(v["runs"].as_array().unwrap().is_empty(), "{v}");
}

#[test]
fn orchestrator_commands_drive_the_run_through_tome_run_id() {
    let env = Env::new();
    env.start_daemon();
    write_wf(&env, "build", WF);
    let id = start(&env, &["build"]);
    let rid = id.to_string();
    let with_run = |args: &[&str]| {
        let mut full = vec!["--json"];
        full.extend_from_slice(args);
        let out = env.cmd(&full).env("TOME_RUN_ID", &rid).output().unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        (out.status.code().unwrap(), v)
    };

    let (code, v) = with_run(&["step", "start", "Implement"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["status"], "running");
    // done/fail without a name apply to the running step.
    let (code, v) = with_run(&["step", "done", "-m", "compiled"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["name"], "Implement");
    assert_eq!(v["status"], "done");
    let (code, v) = with_run(&["step", "fail"]);
    assert_eq!(code, 2, "{v}");
    assert!(v["error"]["message"]
        .as_str()
        .unwrap()
        .contains("no running step"));

    with_run(&["step", "start", "Review"]);
    with_run(&["step", "fail", "Review", "--message", "changes requested"]);
    with_run(&["step", "start", "Implement"]);
    with_run(&["step", "done", "Implement"]);

    let (code, v) = with_run(&[
        "run",
        "finish",
        "--status",
        "succeeded",
        "--summary",
        "shipped",
    ]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["status"], "succeeded");

    let (_, show) = env.json(&["runs", "show", &rid]);
    assert_eq!(show["run"]["summary"], "shipped");
    let steps: Vec<_> = show["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["name"].clone(),
                s["status"].clone(),
                s["attempts"].clone(),
            )
        })
        .collect();
    // Ordered by (latest) start: Implement was retried after Review.
    assert_eq!(
        steps,
        [
            (json!("Review"), json!("failed"), json!(1)),
            (json!("Implement"), json!("done"), json!(2))
        ]
    );
    assert_eq!(show["history"].as_array().unwrap().len(), 6);

    // A finished run takes no more reports.
    let (code, _) = with_run(&["step", "start", "Again"]);
    assert_eq!(code, 2);
    let (code, _) = with_run(&["run", "finish", "--status", "failed"]);
    assert_eq!(code, 2);
}

#[test]
fn run_side_commands_need_a_run() {
    let env = Env::new();
    env.start_daemon();
    let (code, v) = env.json(&["step", "start", "Build"]);
    assert_eq!(code, 2);
    assert!(v["error"]["hint"].as_str().unwrap().contains("TOME_RUN_ID"));
    let (code, _) = env.json(&["step", "start", "Build", "--run", "99"]);
    assert_eq!(code, 4);
    let (code, _) = env.json(&["run", "finish", "--status", "cancelled", "--run", "1"]);
    assert_eq!(code, 2);
}

#[test]
fn cancel_marks_run_cancelled() {
    let env = Env::new();
    env.start_daemon();
    write_wf(&env, "build", WF);
    let id = start(&env, &["build"]);
    env.json(&["step", "start", "Build", "--run", &id.to_string()]);

    let out = env.run(&["run", "cancel", &id.to_string()]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        format!("run {id} cancelled: user_cancelled")
    );

    let (_, show) = env.json(&["runs", "show", &id.to_string()]);
    assert_eq!(show["run"]["status"], "cancelled");
    assert_eq!(show["steps"][0]["status"], "failed");

    let (code, _) = env.json(&["run", "cancel", &id.to_string()]);
    assert_eq!(code, 2);
}
