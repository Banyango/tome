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
fn tome_output_env_selects_json() {
    let env = Env::new();
    let out = env.cmd(&["daemon", "status"]).env("TOME_OUTPUT", "json").output().unwrap();
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
