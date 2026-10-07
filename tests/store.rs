mod common;

use common::Env;
use serde_json::json;
use std::fs;

const WF: &str = "---\nname: build\nparams:\n  base: {default: main}\n---\n## Build\nBranch off {{params.base}} as run-{{run.id}}.\n";

fn create_run(env: &Env, path: &std::path::Path, params: &[&str]) -> serde_json::Value {
    env.rpc_ok(
        "run.create",
        json!({ "workflow_path": path, "project_path": env.project(), "params": params }),
    )
}

#[test]
fn run_keeps_snapshot_taken_at_start() {
    let env = Env::new();
    env.start_daemon();
    let path = env.project().join("build.md");
    fs::write(&path, WF).unwrap();

    let run = create_run(&env, &path, &["base=dev"]);
    let id = run["id"].as_i64().unwrap();
    assert_eq!(run["status"], "running");
    assert_eq!(run["params"]["base"], "dev");
    let snap = run["workflow_snapshot"].as_str().unwrap();
    assert!(
        snap.contains(&format!("Branch off dev as run-{id}.")),
        "{snap}"
    );

    // Editing the file mid-run doesn't change the saved copy.
    fs::write(&path, WF.replace("Branch off", "Fork from")).unwrap();
    let got = env.rpc_ok("run.get", json!({ "id": id }));
    assert_eq!(got["workflow_snapshot"], run["workflow_snapshot"]);

    // New runs see the edit.
    let run2 = create_run(&env, &path, &[]);
    assert!(run2["workflow_snapshot"]
        .as_str()
        .unwrap()
        .contains("Fork from main"));
}

#[test]
fn invalid_workflow_refused_before_run_starts() {
    let env = Env::new();
    env.start_daemon();
    let path = env.project().join("bad.md");
    fs::write(&path, "---\nname: bad\n---\n{{params.nope}}\n").unwrap();
    let resp = env.rpc("run.create", json!({ "workflow_path": path }));
    assert_eq!(resp["error"]["data"]["kind"], "invalid_workflow");
    assert_eq!(resp["error"]["data"]["details"]["errors"][0]["line"], 4);

    let path = env.project().join("req.md");
    fs::write(
        &path,
        "---\nname: req\nparams:\n  ticket: {type: string}\n---\n{{params.ticket}}\n",
    )
    .unwrap();
    let resp = env.rpc("run.create", json!({ "workflow_path": path }));
    assert_eq!(resp["error"]["data"]["kind"], "invalid_workflow");
}

#[test]
fn malformed_run_request_refused_before_recording() {
    let env = Env::new();
    env.start_daemon();
    let path = env.project().join("build.md");
    fs::write(&path, WF).unwrap();
    for bad in [
        json!({ "workflow_path": path, "params": [1] }),
        json!({ "workflow_path": path, "params": "base=dev" }),
        json!({ "workflow_path": path, "project_path": 5 }),
        json!({ "workflow_path": path, "placement": { "layout": "nope" } }),
    ] {
        for method in ["run.create", "run.start"] {
            let resp = env.rpc(method, bad.clone());
            assert_eq!(resp["error"]["data"]["kind"], "invalid", "{method} {bad}");
        }
    }
    let runs = env.rpc_ok("runs.list", json!({}));
    assert_eq!(runs["runs"], json!([]));
}

#[test]
fn state_survives_daemon_restart() {
    let env = Env::new();
    env.start_daemon();
    let path = env.project().join("build.md");
    fs::write(&path, WF).unwrap();
    let id = create_run(&env, &path, &[])["id"].as_i64().unwrap();
    env.rpc_ok(
        "step.report",
        json!({ "run_id": id, "step": "Build", "event": "start" }),
    );
    env.rpc_ok(
        "step.report",
        json!({ "run_id": id, "step": "Build", "event": "done" }),
    );
    env.rpc_ok(
        "run.finish",
        json!({ "id": id, "status": "succeeded", "summary": "ok" }),
    );
    assert!(env.home().join("tome.duckdb").exists());
    assert!(env.home().join("runs").join(id.to_string()).is_dir());

    let (code, _) = env.json(&["daemon", "stop"]);
    assert_eq!(code, 0);
    env.start_daemon();

    let run = env.rpc_ok("run.get", json!({ "id": id }));
    assert_eq!(run["status"], "succeeded");
    assert_eq!(run["summary"], "ok");
    // Ids keep increasing across restarts.
    let next = create_run(&env, &path, &[])["id"].as_i64().unwrap();
    assert_eq!(next, id + 1);
}

#[test]
fn step_reports_validate_input() {
    let env = Env::new();
    env.start_daemon();
    let resp = env.rpc(
        "step.report",
        json!({ "run_id": 77, "step": "x", "event": "start" }),
    );
    assert_eq!(resp["error"]["data"]["kind"], "not_found");
    let resp = env.rpc(
        "step.report",
        json!({ "run_id": 1, "step": "x", "event": "explode" }),
    );
    assert_eq!(resp["error"]["data"]["kind"], "invalid");
}
