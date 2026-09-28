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

fn runs(env: &Env) -> Vec<Value> {
    let mut runs = env.json(&["runs", "list"]).1["runs"].as_array().cloned().unwrap_or_default();
    runs.sort_by_key(|r| r["id"].as_i64());
    runs
}

fn finish(env: &Env, id: &Value) {
    let (code, out) = env.json(&["run", "finish", "--status", "succeeded", "--run", &id.to_string()]);
    assert_eq!(code, 0, "{out}");
}

#[test]
fn topic_triggers_start_a_run_per_event_within_their_concurrency() {
    let env = Env::new();
    write_wf(
        &env,
        "review",
        "concurrency: 1\non_conflict: reject\ntriggers:\n  - on: review.*\n",
        "## Review\nGot {{trigger.payload}} on {{trigger.topic}} (event {{trigger.event_id}} from {{trigger.sender}})\n",
    );
    start_fast_daemon(&env);
    let (_, v) = env.json(&["publish", "review.requested", "x", "--dry-run"]);
    assert_eq!(v["matches"][0]["would"], "would start a run of review", "{v}");

    assert_eq!(env.json(&["publish", "review.requested", "first"]).0, 0);
    assert_eq!(env.json(&["publish", "review.requested", "second"]).0, 0);
    common::eventually("a run for the first event", || runs(&env).len() == 1);
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert_eq!(runs(&env).len(), 1, "the second waits for the limit, as a pending delivery rather than a queued run");
    assert_eq!(deliveries(&env), [("review", "claimed"), ("review", "pending")].map(|(a, b)| (a.to_string(), b.to_string())));
    let first = runs(&env)[0]["id"].clone();
    assert_eq!(query(&env, "SELECT run_ids FROM deliveries WHERE id = 1")[0][0], format!("[{first}]"));

    let (_, show) = env.json(&["runs", "show", &first.to_string(), "--snapshot"]);
    let snap = show["run"]["workflow_snapshot"].as_str().unwrap();
    assert!(snap.contains("Got first on review.requested (event 1 from user)"), "{snap}");
    assert_eq!(show["run"]["trigger"]["topic"], "review.requested");
    assert_eq!(show["run"]["trigger"]["depth"], 0);
    let human = String::from_utf8_lossy(&env.run(&["runs", "show", &first.to_string()]).stdout).to_string();
    assert!(human.contains("event     1 on review.requested from user"), "{human}");

    let (_, v) = env.json(&["publish", "review.done", "x", "--dry-run"]);
    let would = v["matches"][0]["would"].as_str().unwrap();
    assert!(would.starts_with("would wait: review at its concurrency limit"), "{would}");

    // A run ending makes room for the next event.
    finish(&env, &first);
    common::eventually("a run for the second event", || runs(&env).len() == 2);
    let second = runs(&env)[1]["id"].clone();
    let (_, show) = env.json(&["runs", "show", &second.to_string(), "--snapshot"]);
    assert!(show["run"]["workflow_snapshot"].as_str().unwrap().contains("Got second"));
    let outcomes: Vec<Value> = query(&env, "SELECT outcome FROM trigger_fires WHERE trigger_index >= 0 ORDER BY id").into_iter().map(|r| r[0].clone()).collect();
    assert_eq!(outcomes, ["started", "started"]);
}

#[test]
fn backlogs_wait_for_triggers_to_be_enabled_and_workflows_to_be_valid() {
    let env = Env::new();
    write_wf(&env, "deploy", "triggers:\n  - on: deploy\n", "## Deploy\n{{trigger.payload}}\n");
    start_fast_daemon(&env);
    assert_eq!(env.json(&["triggers", "disable"]).0, 0);
    let (_, v) = env.json(&["publish", "deploy", "x", "--dry-run"]);
    assert_eq!(v["matches"][0]["would"], "would wait: the project's triggers are disabled");
    assert_eq!(env.json(&["publish", "deploy", "a"]).0, 0);
    assert_eq!(env.json(&["publish", "deploy", "b"]).0, 0);

    // Invalid when enabled: nothing is claimed, and the error is recorded.
    let dir = env.project().join(".tome/workflows");
    fs::write(dir.join("deploy.md"), "---\nname: deploy\nconcurrency: nope\ntriggers:\n  - on: deploy\n---\n## Deploy\nGo.\n").unwrap();
    assert_eq!(env.json(&["triggers", "enable"]).0, 0);
    common::eventually("the invalid workflow recorded", || {
        !query(&env, "SELECT id FROM trigger_fires WHERE workflow_name = 'deploy' AND outcome = 'error'").is_empty()
    });
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(runs(&env).is_empty());
    assert_eq!(deliveries(&env), [("deploy", "pending"), ("deploy", "pending")].map(|(a, b)| (a.to_string(), b.to_string())));

    // Fixed: the backlog is taken in publish order.
    write_wf(&env, "deploy", "triggers:\n  - on: deploy\n", "## Deploy\n{{trigger.payload}}\n");
    common::eventually("a run per event", || runs(&env).len() == 2);
    let causes: Vec<Value> = runs(&env).iter().map(|r| env.json(&["runs", "show", &r["id"].to_string()]).1["run"]["trigger"]["event_id"].clone()).collect();
    assert_eq!(causes, [1, 2]);
    assert_eq!(deliveries(&env), [("deploy", "claimed"), ("deploy", "claimed")].map(|(a, b)| (a.to_string(), b.to_string())));
}

fn daemon_log(env: &Env) -> String {
    fs::read_to_string(env.home().join("daemon.log")).unwrap_or_default()
}

#[test]
fn deliveries_settle_with_the_run_that_claimed_them() {
    let env = Env::new();
    write_wf(&env, "review", "triggers:\n  - on: review\n", "## Review\nGo.\n");
    start_fast_daemon(&env);
    for payload in ["fine", "breaks\nsecond line", "stopped"] {
        assert_eq!(env.json(&["publish", "review", payload]).0, 0);
    }
    common::eventually("a run per event", || runs(&env).len() == 3);
    let ids: Vec<Value> = runs(&env).iter().map(|r| r["id"].clone()).collect();
    finish(&env, &ids[0]);
    let (code, out) = env.json(&["run", "finish", "--status", "failed", "--summary", "tests failed", "--run", &ids[1].to_string()]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(env.json(&["run", "cancel", &ids[2].to_string()]).0, 0);
    let want = [("review", "done"), ("review", "failed"), ("review", "failed")].map(|(a, b)| (a.to_string(), b.to_string()));
    common::eventually("the deliveries settled", || deliveries(&env) == want);

    let log = daemon_log(&env);
    let notes: Vec<&str> = log.lines().filter(|l| l.contains("notify: review didn't handle review")).collect();
    assert_eq!(notes.len(), 2, "{log}");
    assert!(notes[0].contains(&format!("run {} failed (tests failed)", ids[1])) && notes[0].contains("\"breaks\""), "{}", notes[0]);
    assert!(!notes[0].contains("second line"), "first line only");
    assert!(notes[1].contains(&format!("run {} cancelled", ids[2])), "{}", notes[1]);
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert_eq!(runs(&env).len(), 3, "failed deliveries aren't handed out again");

    // gc takes the done delivery; the failed ones keep their events.
    let (code, v) = env.json(&["gc", "--older-than", "1h"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["bus"], serde_json::json!({ "deliveries": 1, "events": 1 }));
    assert_eq!(query(&env, "SELECT id FROM bus_events ORDER BY id").iter().map(|r| r[0].clone()).collect::<Vec<_>>(), [2, 3]);
}

#[test]
fn runs_lost_to_a_daemon_restart_fail_their_deliveries() {
    let env = Env::new();
    write_wf(&env, "review", "triggers:\n  - on: review\n", "## Review\nGo.\n");
    start_fast_daemon(&env);
    assert_eq!(env.json(&["publish", "review", "x"]).0, 0);
    common::eventually("a run", || runs(&env).len() == 1);
    let (_, status) = env.json(&["daemon", "status"]);
    unsafe { libc::kill(status["pid"].as_i64().unwrap() as i32, libc::SIGKILL) };
    common::eventually("daemon to die", || env.json(&["daemon", "status"]).0 == 3);

    start_fast_daemon(&env);
    common::eventually("the delivery failed", || deliveries(&env) == [("review".to_string(), "failed".to_string())]);
    assert!(daemon_log(&env).contains("(daemon_restart); event 1 \"x\" is parked"), "{}", daemon_log(&env));
}
