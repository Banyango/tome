mod common;

use common::Env;
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::process::Stdio;

fn write_wf(env: &Env, name: &str, frontmatter: &str, body: &str) {
    let dir = env.project().join(".tome/workflows");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{name}.md")), format!("---\nname: {name}\n{frontmatter}---\n{body}")).unwrap();
}

fn query(env: &Env, sql: &str) -> Vec<Value> {
    let (code, v) = env.json(&["query", sql]);
    assert_eq!(code, 0, "{v}");
    v["rows"].as_array().cloned().unwrap_or_default()
}

fn deliveries(env: &Env) -> Vec<(String, String)> {
    query(env, "SELECT workflow_name, state FROM deliveries ORDER BY id")
        .iter()
        .map(|r| (r[0].as_str().unwrap().to_string(), r[1].as_str().unwrap().to_string()))
        .collect()
}

fn start_fast_daemon(env: &Env) {
    let out = env.cmd(&["daemon", "start"]).env("TOME_TRIGGER_TICK_MS", "100").output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn publishing_records_an_event_and_a_delivery_per_subscribing_workflow() {
    let env = Env::new();
    write_wf(&env, "review", "triggers:\n  - on: review.*\n  - on: review.requested\n", "## Review\n{{trigger.payload}}\n");
    write_wf(&env, "audit", "triggers:\n  - on: \"**\"\n", "## Audit\nGo.\n");
    write_wf(&env, "deploy", "triggers:\n  - on: deploy\n", "## Deploy\nGo.\n");
    env.start_daemon();
    // Deliveries accumulate while the project's triggers are off.
    assert_eq!(env.json(&["triggers", "disable"]).0, 0);

    // A dry run lists the matches and records nothing.
    let (code, v) = env.json(&["publish", "review.requested", "hi", "--dry-run"]);
    assert_eq!(code, 0, "{v}");
    let matched: Vec<&str> = v["matches"].as_array().unwrap().iter().map(|m| m["workflow"].as_str().unwrap()).collect();
    assert_eq!(matched, ["audit", "review"]);
    assert_eq!(v["matches"][1]["on"], "on review.*", "the first matching trigger");
    assert!(query(&env, "SELECT id FROM bus_events").is_empty());

    let (code, v) = env.json(&["publish", "review.requested", "please look at #12"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["event"]["sender"], "user");
    assert_eq!(v["event"]["depth"], 0);
    assert_eq!(v["delivered_to"], serde_json::json!(["audit", "review"]));
    let human = String::from_utf8_lossy(&env.run(&["publish", "review.done", "ok"]).stdout).to_string();
    assert!(human.starts_with("event 2 on review.done: delivered to audit, review"), "{human}");

    // stdin, and an event nobody listens for.
    let mut child = env.cmd(&["--json", "publish", "nobody.here", "-"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(b"from stdin\n").unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["event"]["payload"], "from stdin\n");
    assert_eq!(v["delivered_to"], serde_json::json!(["audit"]), "`**` matches every topic");

    assert_eq!(
        deliveries(&env),
        [("audit", "pending"), ("review", "pending"), ("audit", "pending"), ("review", "pending"), ("audit", "pending")]
            .map(|(a, b)| (a.to_string(), b.to_string()))
    );
    assert_eq!(query(&env, "SELECT count(*) FROM bus_events")[0][0], 3);
}

#[test]
fn bad_publishes_exit_2() {
    let env = Env::new();
    fs::create_dir_all(env.project().join(".tome/workflows")).unwrap();
    env.start_daemon();
    let (code, v) = env.json(&["publish", "tome.run.x.failed", "no"]);
    assert_eq!(code, 2, "{v}");
    assert!(v["error"]["message"].as_str().unwrap().contains("reserved"), "{v}");
    let (code, _) = env.json(&["publish", "Bad Topic", "no"]);
    assert_eq!(code, 2);
    // Through stdin: it's bigger than an argument may be.
    let mut child = env.cmd(&["--json", "publish", "big", "-"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all("x".repeat(1024 * 1024 + 1).as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["error"]["hint"].as_str().unwrap().contains("file path"), "{v}");
    let out = env.cmd(&["--json", "publish", "a", "b"]).current_dir(env.home()).output().unwrap();
    assert_eq!(out.status.code(), Some(2), "outside a project");
    assert!(query(&env, "SELECT id FROM bus_events").is_empty());
}

#[test]
fn a_publish_inside_a_run_names_the_run_and_shows_in_its_stream() {
    let env = Env::new();
    write_wf(&env, "impl", "", "## Go\nGo.\n");
    write_wf(&env, "review", "triggers:\n  - on: impl.done\n", "## Review\nGo.\n");
    env.start_daemon();
    assert_eq!(env.json(&["triggers", "disable"]).0, 0);
    let (code, run) = env.json(&["run", "impl", "--detach"]);
    assert_eq!(code, 0, "{run}");
    let id = run["id"].to_string();

    let out = env.cmd(&["--json", "publish", "impl.done", "branch x"]).env("TOME_RUN_ID", &id).env("TOME_WORKER_ID", "w1").output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["event"]["sender"], format!("run {id} worker w1"));
    assert_eq!(v["event"]["sender_run_id"].to_string(), id);

    let events = query(&env, &format!("SELECT event, message FROM worker_events WHERE run_id = {id} AND event = 'published'"));
    assert_eq!(events.len(), 1);
    let message = events[0][1].as_str().unwrap();
    assert!(message.contains("impl.done") && message.contains("review"), "{message}");
}

#[test]
fn removed_subscriptions_drop_their_pending_deliveries() {
    let env = Env::new();
    write_wf(&env, "review", "triggers:\n  - on: review.requested\n", "## Review\nGo.\n");
    write_wf(&env, "audit", "triggers:\n  - on: review.requested\n", "## Audit\nGo.\n");
    start_fast_daemon(&env);
    assert_eq!(env.json(&["triggers", "disable"]).0, 0);
    assert_eq!(env.json(&["publish", "review.requested", "x"]).0, 0);
    assert_eq!(deliveries(&env).len(), 2);

    // An invalid workflow keeps its deliveries until it's fixed.
    write_wf(&env, "audit", "triggers:\n  - on: Bad\n", "## Audit\nGo.\n");
    // Removing the trigger drops the delivery once the project is re-armed.
    write_wf(&env, "review", "triggers:\n  - manual\n", "## Review\nGo.\n");
    assert_eq!(env.json(&["triggers", "enable"]).0, 0);
    common::eventually("review's delivery dropped", || {
        deliveries(&env).contains(&("review".to_string(), "dropped".to_string()))
    });
    assert!(deliveries(&env).contains(&("audit".to_string(), "pending".to_string())));
}
