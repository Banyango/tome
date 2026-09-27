mod common;

use common::Env;
use serde_json::Value;
use std::fs;

fn write_wf(env: &Env, name: &str, frontmatter: &str, body: &str) {
    let dir = env.project().join(".tome/workflows");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{name}.md")), format!("---\nname: {name}\n{frontmatter}---\n{body}")).unwrap();
}

fn outcomes(env: &Env) -> Vec<String> {
    fires(env).iter().map(|r| r[0].as_str().unwrap_or("").to_string()).collect()
}

fn fires(env: &Env) -> Vec<Value> {
    let (code, v) = env.json(&["query", "SELECT outcome, trigger_desc, message FROM trigger_fires ORDER BY id"]);
    assert_eq!(code, 0, "{v}");
    v["rows"].as_array().cloned().unwrap_or_default()
}

const REVIEW: &str = "params:\n  target: {type: string}\n  depth: {type: int, default: 1}\n\
triggers:\n  - manual\n  - cron: \"0 9 * * 1-5\"\n    params: {target: nightly, depth: 3}\n  - file: \"specs/**/*.md\"\n    params: {target: specs}\n";

#[test]
fn a_fired_trigger_starts_a_run_that_records_its_cause() {
    let env = Env::new();
    write_wf(&env, "review", REVIEW, "## Review\nReview {{params.target}} ({{trigger.kind}}, {{trigger.event}}) [{{trigger.paths}}]\n");
    env.start_daemon();

    // Dry run: the resolved params, nothing started or recorded.
    let (code, v) = env.json(&["triggers", "fire", "review", "--dry-run"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["index"], 1, "defaults to the first non-manual trigger");
    assert_eq!(v["params"]["target"], "nightly");
    assert_eq!(v["params"]["depth"], 3);
    assert_eq!(v["run_ids"], serde_json::json!([]));
    let (_, runs) = env.json(&["runs", "list"]);
    assert!(runs["runs"].as_array().unwrap().is_empty());
    assert!(fires(&env).is_empty(), "dry runs aren't recorded");

    let (code, v) = env.json(&["triggers", "fire", "review"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["outcome"], "started");
    let id = v["run_ids"][0].as_i64().expect("a run id");

    let (_, show) = env.json(&["runs", "show", &id.to_string(), "--snapshot"]);
    let run = &show["run"];
    assert_eq!(run["params"]["target"], "nightly");
    assert_eq!(run["trigger"]["kind"], "cron");
    assert_eq!(run["trigger"]["trigger"], "cron 0 9 * * 1-5");
    assert_eq!(run["trigger"]["synthetic"], true);
    assert!(run["workflow_snapshot"].as_str().unwrap().contains("Review nightly (cron, scheduled) []"), "{run}");
    let human = String::from_utf8_lossy(&env.run(&["runs", "show", &id.to_string()]).stdout).to_string();
    assert!(human.contains("trigger   cron 0 9 * * 1-5 [fired by hand]"), "{human}");

    // A file trigger reports its paths, relative to the project root. It's
    // muted while the cron run is active.
    fs::create_dir_all(env.project().join("specs")).unwrap();
    let (code, v) = env.json(&["triggers", "fire", "review", "--index", "2", "--path", "specs/a.md"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["outcome"], "muted", "{v}");

    let (code, _) = env.json(&["run", "finish", "--status", "succeeded", "--run", &id.to_string()]);
    assert_eq!(code, 0);
    let (code, v) = env.json(&["triggers", "fire", "review", "--index", "2", "--path", "specs/a.md"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["outcome"], "started");
    let (_, show) = env.json(&["runs", "show", &v["run_ids"][0].to_string(), "--snapshot"]);
    let snap = show["run"]["workflow_snapshot"].as_str().unwrap();
    assert!(snap.contains("Review specs (file, modified) [specs/a.md (modified)]"), "{snap}");

    assert_eq!(outcomes(&env), ["started", "muted", "started"]);
}

#[test]
fn file_trigger_paths_render_into_the_run() {
    let env = Env::new();
    write_wf(&env, "lint", "triggers:\n  - file: \"src/*.rs\"\n    on: [created]\n", "## Lint\nLint {{trigger.paths}} on {{trigger.event}}.\n");
    env.start_daemon();
    let (code, v) = env.json(&["triggers", "fire", "lint", "--path", "src/a.rs", "--path", "src/b.rs"]);
    assert_eq!(code, 0, "{v}");
    let id = v["run_ids"][0].to_string();
    let (_, show) = env.json(&["runs", "show", &id, "--snapshot"]);
    let snap = show["run"]["workflow_snapshot"].as_str().unwrap();
    assert!(snap.contains("Lint src/a.rs (created), src/b.rs (created) on created."), "{snap}");
}

#[test]
fn rejected_and_invalid_fires_are_recorded() {
    let env = Env::new();
    write_wf(&env, "build", "concurrency: 1\non_conflict: reject\ntriggers:\n  - cron: \"* * * * *\"\n", "## Build\nGo.\n");
    env.start_daemon();

    assert_eq!(env.json(&["triggers", "fire", "build"]).1["outcome"], "started");
    let (code, v) = env.json(&["triggers", "fire", "build"]);
    assert_eq!(code, 1, "{v}");
    assert_eq!(v["outcome"], "rejected");
    assert!(v["message"].as_str().unwrap().contains("concurrency limit"), "{v}");

    // No such trigger.
    let (code, v) = env.json(&["triggers", "fire", "build", "--index", "5"]);
    assert_eq!(code, 1);
    assert!(v["message"].as_str().unwrap().contains("no trigger at index 5"), "{v}");

    // An invalid workflow is still fired, and the error recorded.
    write_wf(&env, "build", "triggers:\n  - cron: \"nope\"\n", "## Build\nGo.\n");
    let (code, v) = env.json(&["triggers", "fire", "build", "--index", "0"]);
    assert_eq!(code, 1, "{v}");
    assert_eq!(v["outcome"], "error");
    assert!(v["message"].as_str().unwrap().contains("invalid cron expression"), "{v}");

    assert_eq!(outcomes(&env), ["started", "rejected", "error", "error"]);
}
