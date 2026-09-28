mod common;

use common::Env;
use serde_json::json;
use std::time::{Duration, Instant};

#[test]
fn crash_fails_in_progress_runs_and_keeps_worktrees() {
    let env = Env::new();
    env.start_daemon();
    let path = env.project().join("build.md");
    std::fs::write(&path, "---\nname: build\n---\n## Build\ngo\n").unwrap();
    let live = env.rpc_ok("run.create", json!({ "workflow_path": path }))["id"].as_i64().unwrap();
    env.rpc_ok("step.report", json!({ "run_id": live, "step": "Build", "event": "start" }));
    let wt = env.dir.path().join("wt");
    std::fs::create_dir_all(&wt).unwrap();
    env.rpc_ok("worktree.add", json!({ "run_id": live, "path": wt }));
    let done = env.rpc_ok("run.create", json!({ "workflow_path": path }))["id"].as_i64().unwrap();
    env.rpc_ok("run.finish", json!({ "id": done, "status": "succeeded" }));

    // Simulate a crash: SIGKILL leaves the socket and the run behind.
    let pid = env.rpc_ok("daemon.status", json!({}))["pid"].as_i64().unwrap();
    unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    let deadline = Instant::now() + Duration::from_secs(10);
    while unsafe { libc::kill(pid as i32, 0) } == 0 {
        assert!(Instant::now() < deadline, "daemon didn't die");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(env.socket().exists(), "stale socket expected after a crash");

    env.start_daemon();
    let (_, v) = env.json(&["runs", "show", &live.to_string()]);
    assert_eq!(v["run"]["status"], "failed");
    assert_eq!(v["run"]["reason"], "daemon_restart");
    assert_eq!(v["steps"][0]["status"], "failed");
    assert_eq!(v["worktrees"][0]["path"], json!(wt));
    assert!(wt.is_dir(), "worktree must be kept");

    let (_, v) = env.json(&["runs", "show", &done.to_string()]);
    assert_eq!(v["run"]["status"], "succeeded");
    assert!(v["run"]["reason"].is_null());

    let log = std::fs::read_to_string(env.home().join("daemon.log")).unwrap();
    assert!(log.contains(&format!("run {live} (build) marked failed: daemon_restart")), "{log}");
}

fn kill_daemon(env: &Env) {
    let pid = env.rpc_ok("daemon.status", json!({}))["pid"].as_i64().unwrap();
    unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    let deadline = Instant::now() + Duration::from_secs(10);
    while unsafe { libc::kill(pid as i32, 0) } == 0 {
        assert!(Instant::now() < deadline, "daemon didn't die");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn queued_runs_survive_a_restart_and_start_once_idle() {
    let env = Env::new();
    let dir = env.project().join(".tome/workflows");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("build.md"), "---\nname: build\nconcurrency: 1\n---\n## Build\nrun {{run.id}}\n").unwrap();
    env.start_daemon();
    let detach = || env.json(&["run", "build", "--detach"]).1;
    let live = detach()["id"].as_i64().unwrap();
    let queued = detach();
    assert_eq!(queued["status"], "queued", "{queued}");
    let queued = queued["id"].as_i64().unwrap();
    let snapshot = |id: i64| env.json(&["runs", "show", &id.to_string(), "--snapshot"]).1["run"]["workflow_snapshot"].clone();
    let before = snapshot(queued);

    kill_daemon(&env);
    env.start_daemon();
    let status = |id: i64| env.json(&["runs", "show", &id.to_string()]).1["run"].clone();
    assert_eq!(status(live)["reason"], "daemon_restart");
    common::eventually("the queued run to start", || status(queued)["status"] == "running");
    assert!(status(queued)["reason"].is_null());
    assert_eq!(snapshot(queued), before, "keeps its original snapshot");
}
