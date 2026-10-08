mod common;

use common::Env;
use serde_json::json;
use std::fs;

const WF: &str = "---\nname: build\nparams:\n  base: {default: main}\n---\n## Build\nBranch off {{params.base}}.\n";

/// Two runs: #1 succeeded with a Build step and log, #2 still running.
fn seed(env: &Env) -> (i64, i64) {
    env.start_daemon();
    let path = env.project().join("build.md");
    fs::write(&path, WF).unwrap();
    let a = env.rpc_ok("run.create", json!({ "workflow_path": path }))["id"]
        .as_i64()
        .unwrap();
    env.rpc_ok(
        "step.report",
        json!({ "run_id": a, "step": "Build", "event": "start" }),
    );
    env.rpc_ok(
        "step.report",
        json!({ "run_id": a, "step": "Build", "event": "done", "message": "built" }),
    );
    env.rpc_ok(
        "worktree.add",
        json!({ "run_id": a, "path": "/tmp/wt-a", "branch": "run-a" }),
    );
    let log: String = (1..=30).map(|i| format!("line {i}\n")).collect();
    fs::write(env.home().join(format!("runs/{a}/Build.log")), log).unwrap();
    env.rpc_ok("run.finish", json!({ "id": a, "status": "succeeded" }));
    let b = env.rpc_ok(
        "run.create",
        json!({ "workflow_path": path, "params": ["base=dev"] }),
    )["id"]
        .as_i64()
        .unwrap();
    (a, b)
}

#[test]
fn runs_list_filters_and_renders() {
    let env = Env::new();
    let (a, b) = seed(&env);

    let (code, v) = env.json(&["runs", "list"]);
    assert_eq!(code, 0);
    let ids: Vec<i64> = v["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids, [b, a]);

    let (_, v) = env.json(&["runs", "list", "--status", "running"]);
    assert_eq!(v["runs"].as_array().unwrap().len(), 1);
    assert_eq!(v["runs"][0]["id"], b);

    let (_, v) = env.json(&["runs", "list", "--workflow", "nope"]);
    assert!(v["runs"].as_array().unwrap().is_empty());

    let (code, v) = env.json(&["runs", "list", "--status", "bogus"]);
    assert_eq!(code, 2);
    assert_eq!(v["error"]["kind"], "invalid");

    let out = env.run(&["runs", "list", "--limit", "1"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.starts_with("ID  WORKFLOW  STATUS"), "{text}");
    assert_eq!(text.lines().count(), 2, "{text}");
}

#[test]
fn runs_show_includes_steps_history_worktrees_logs() {
    let env = Env::new();
    let (a, _) = seed(&env);

    let (code, v) = env.json(&["runs", "show", &a.to_string()]);
    assert_eq!(code, 0);
    assert_eq!(v["run"]["status"], "succeeded");
    assert!(v["run"].get("workflow_snapshot").is_none());
    assert_eq!(v["steps"][0]["name"], "Build");
    assert_eq!(v["steps"][0]["status"], "done");
    assert_eq!(v["history"].as_array().unwrap().len(), 2);
    assert_eq!(v["worktrees"][0]["path"], "/tmp/wt-a");
    assert_eq!(v["logs"][0]["step"], "Build");
    assert!(v["logs"][0]["tail"].as_str().unwrap().ends_with("line 30"));

    let (_, v) = env.json(&["runs", "show", &a.to_string(), "--snapshot"]);
    assert!(v["run"]["workflow_snapshot"]
        .as_str()
        .unwrap()
        .contains("Branch off main."));

    let out = env.run(&["runs", "show", &format!("#{a}")]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("[succeeded]") && text.contains("steps:") && text.contains("Build"),
        "{text}"
    );

    let (code, v) = env.json(&["runs", "show", "999"]);
    assert_eq!(code, 4);
    assert_eq!(v["error"]["kind"], "not_found");
}

#[test]
fn step_status_sets_the_custom_status() {
    let env = Env::new();
    let (_, b) = seed(&env);
    let rid = b.to_string();

    // Nothing to attach it to yet.
    let (code, _) = env.json(&["step", "status", "InProgress", "--run", &rid]);
    assert_eq!(code, 2);

    env.json(&["step", "start", "Build", "--run", &rid]);
    let (code, v) = env.json(&["step", "status", "InProgress", "--run", &rid]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["name"], "Build");
    assert_eq!(v["custom_status"], "InProgress");

    let (_, v) = env.json(&["runs", "show", &rid]);
    assert_eq!(v["run"]["custom_status"], "InProgress");
    assert_eq!(v["run"]["status"], "running");
    assert_eq!(v["steps"][0]["custom_status"], "InProgress");

    let out = env.run(&["runs", "show", &rid]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("custom    InProgress"), "{text}");
}

#[test]
fn runs_logs_prints_content_and_tail() {
    let env = Env::new();
    let (a, b) = seed(&env);
    let id = a.to_string();

    let out = env.run(&["runs", "logs", &id, "--step", "Build", "--tail", "2"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "line 29\nline 30\n");

    let (_, v) = env.json(&["runs", "logs", &id]);
    assert_eq!(
        v["logs"][0]["content"].as_str().unwrap().lines().count(),
        30
    );

    let (code, _) = env.json(&["runs", "logs", &id, "--step", "Nope"]);
    assert_eq!(code, 4);

    let out = env.run(&["runs", "logs", &b.to_string()]);
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "no logs");
}

#[test]
fn query_is_read_only() {
    let env = Env::new();
    let (a, b) = seed(&env);

    let (code, v) = env.json(&["query", "select id, status from runs order by id"]);
    assert_eq!(code, 0);
    assert_eq!(v["columns"], json!(["id", "status"]));
    assert_eq!(v["rows"], json!([[a, "succeeded"], [b, "running"]]));

    let out = env.run(&["query", "select count(*) as n from steps"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(text, "n\n1\n(1 row)\n");

    for sql in [
        "delete from runs",
        "select 1; drop table runs",
        "update runs set status = 'x'",
    ] {
        let (code, v) = env.json(&["query", sql]);
        assert_eq!(code, 2, "{sql}: {v}");
        assert_eq!(v["error"]["kind"], "invalid");
    }
    let (code, _) = env.json(&["query", "select * from read_text('/etc/hosts')"]);
    assert_eq!(code, 2);

    // Nothing was changed.
    let (_, v) = env.json(&["query", "select count(*) from runs"]);
    assert_eq!(v["rows"][0][0], 2);
}

#[test]
fn inspection_needs_the_daemon() {
    let env = Env::new();
    let (code, v) = env.json(&["runs", "list"]);
    assert_eq!(code, 3);
    assert_eq!(v["error"]["kind"], "daemon_not_running");
    let (code, _) = env.json(&["query", "select 1"]);
    assert_eq!(code, 3);
}
