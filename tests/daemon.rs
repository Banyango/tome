mod common;

use common::Env;
use serde_json::json;

#[test]
fn commands_fail_with_hint_when_daemon_is_down() {
    let env = Env::new();
    let (code, v) = env.json(&["daemon", "status"]);
    assert_eq!(code, 3);
    assert_eq!(v["running"], false);

    // Human mode prints a hint on stderr and doesn't start the daemon.
    let out = env.run(&["daemon", "status"]);
    assert_eq!(out.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&out.stdout).contains("tome daemon start"));
    assert!(!env.socket().exists());
}

#[test]
fn start_status_stop_lifecycle() {
    let env = Env::new();
    env.start_daemon();

    let (code, v) = env.json(&["daemon", "status"]);
    assert_eq!(code, 0);
    assert_eq!(v["running"], true);
    assert!(v["pid"].as_u64().unwrap() > 0);

    // Starting again is a no-op.
    let (code, v) = env.json(&["daemon", "start"]);
    assert_eq!(code, 0);
    assert_eq!(v["already_running"], true);

    let (code, v) = env.json(&["daemon", "stop"]);
    assert_eq!(code, 0);
    assert_eq!(v["stopped"], true);
    assert!(!env.socket().exists());

    let (code, _) = env.json(&["daemon", "status"]);
    assert_eq!(code, 3);

    // Stopping a stopped daemon is fine.
    let (code, v) = env.json(&["daemon", "stop"]);
    assert_eq!(code, 0);
    assert_eq!(v["was_running"], false);
}

#[test]
fn start_and_stop_are_shorthands_for_the_daemon_commands() {
    let env = Env::new();
    let (code, v) = env.json(&["start"]);
    assert_eq!(code, 0, "{v}");
    let (code, v) = env.json(&["daemon", "status"]);
    assert_eq!((code, &v["running"]), (0, &json!(true)));

    let (code, v) = env.json(&["stop"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["stopped"], true);
    assert!(!env.socket().exists());
}

#[test]
fn tome_output_env_selects_json() {
    let env = Env::new();
    let out = env
        .cmd(&["daemon", "status"])
        .env("TOME_OUTPUT", "json")
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["running"], false);
}

#[test]
fn protocol_is_ndjson_jsonrpc() {
    let env = Env::new();
    env.start_daemon();

    let resp = env.rpc("daemon.ping", json!({}));
    assert_eq!(resp["jsonrpc"], "2.0");
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["result"]["pong"], true);

    let resp = env.rpc("no.such.method", json!({}));
    assert_eq!(resp["error"]["code"], -32601);
}

#[test]
fn second_daemon_refuses_to_run() {
    let env = Env::new();
    env.start_daemon();
    let out = env.run(&["daemon", "run"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("already running"));
    // The first daemon is unaffected.
    assert_eq!(env.rpc("daemon.ping", json!({}))["result"]["pong"], true);
}

#[test]
fn usage_errors_exit_2() {
    let env = Env::new();
    let out = env.run(&["daemon", "bogus"]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn shutdown_ends_streaming_watches_and_exits_promptly() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};

    let env = Env::new();
    env.set_config(common::IDLE_CONFIG);
    let dir = env.project().join(".tome/workflows");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("build.md"),
        "---\nname: build\nmode: orchestrated\n---\n## Build\nBuild it.\n",
    )
    .unwrap();
    env.start_daemon();
    let (code, run) = env.json(&["run", "build", "--detach"]);
    assert_eq!(code, 0, "{run}");

    let mut watch = UnixStream::connect(env.socket()).unwrap();
    let req =
        json!({ "jsonrpc": "2.0", "id": 1, "method": "run.watch", "params": { "id": run["id"] } });
    writeln!(watch, "{req}").unwrap();
    let mut lines = BufReader::new(watch).lines();
    let first: serde_json::Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    assert_eq!(first["params"]["status"], "running", "{first}");

    let started = Instant::now();
    let (code, v) = env.json(&["daemon", "stop"]);
    assert_eq!(code, 0, "{v}");
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "stopped in {:?}",
        started.elapsed()
    );
    assert!(!env.socket().exists());

    // The watch got an answer rather than being cut off mid-wait.
    let last: serde_json::Value = lines
        .map_while(Result::ok)
        .map(|l| serde_json::from_str(&l).unwrap())
        .find(|l: &serde_json::Value| l.get("id").is_some())
        .expect("the watch was answered");
    assert!(
        last["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("shutting down")),
        "{last}"
    );
}
