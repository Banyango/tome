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
    let (code, v) = env.json(&["query", "SELECT outcome, trigger_desc, message FROM trigger_fires WHERE trigger_index >= 0 ORDER BY id"]);
    assert_eq!(code, 0, "{v}");
    v["rows"].as_array().cloned().unwrap_or_default()
}

const REVIEW: &str = "params:\n  target: {type: string}\n  depth: {type: int, default: 1}\ntriggers:\n  - manual\n  - cron: \"0 9 * * 1-5\"\n    params: {target: nightly, depth: 3}\n  - file: \"specs/**/*.md\"\n    params: {target: specs}\n";

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

const LISTENER: &str = r#"
while read line; do echo "$line" >> "$TOME_HOME/typed.txt"; done
"#;

#[test]
fn running_targets_are_signalled_through_the_events_queue() {
    let env = Env::new();
    let script = env.home().join("listener.sh");
    fs::write(&script, LISTENER).unwrap();
    env.set_config(&format!(
        "{}  listener: [sh, \"{}\", \"{{{{prompt_file}}}}\"]\n",
        common::IDLE_CONFIG,
        script.display()
    ));
    write_wf(
        &env,
        "watch",
        concat!(
            "defaults:\n  orchestrator_harness: listener\ntriggers:\n",
            "  - cron: \"*/5 * * * *\"\n    to: running\n",
            "  - cron: \"0 * * * *\"\n    to: running-or-new\n",
            "  - file: \"docs/*.md\"\n    to: running-or-new\n",
        ),
        "## Watch\nWatch.\n",
    );
    env.start_daemon();

    // Nothing to signal yet.
    let (code, v) = env.json(&["triggers", "fire", "watch", "--index", "0"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["outcome"], "no_target");

    // running-or-new with nothing running starts one.
    let (_, v) = env.json(&["triggers", "fire", "watch", "--index", "1"]);
    assert_eq!(v["outcome"], "started", "{v}");
    let run = v["run_ids"][0].as_i64().unwrap();
    common::eventually("orchestrator up", || env.has_session(&format!("tome-{run}-watch")));

    let (_, v) = env.json(&["triggers", "fire", "watch", "--index", "0", "--dry-run"]);
    assert_eq!((v["outcome"].as_str(), &v["runs"]), (Some("signalled"), &serde_json::json!([run])), "{v}");

    // Now both signal the run instead.
    for index in ["0", "1"] {
        let (code, v) = env.json(&["triggers", "fire", "watch", "--index", index]);
        assert_eq!(code, 0, "{v}");
        assert_eq!(v["outcome"], "signalled", "{v}");
        assert_eq!(v["run_ids"], serde_json::json!([run]));
    }
    // A file trigger's running-or-new is `new`, so it's muted.
    assert_eq!(env.json(&["triggers", "fire", "watch", "--index", "2"]).1["outcome"], "muted");

    let typed = env.home().join("typed.txt");
    common::eventually("nudge typed", || {
        fs::read_to_string(&typed).is_ok_and(|t| t.contains("[tome] trigger cron fired. Details: tome queue pull events"))
    });

    let pull = |env: &Env| {
        let out = env.cmd(&["--json", "queue", "pull", "events"]).env("TOME_RUN_ID", run.to_string()).output().unwrap();
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["message"].clone()
    };
    let msg = pull(&env);
    let body: Value = serde_json::from_str(msg["body"].as_str().unwrap()).unwrap();
    assert_eq!(body["type"], "trigger");
    assert_eq!(body["kind"], "cron");
    assert_eq!(body["trigger"], "cron */5 * * * *");
    assert_eq!(body["event"], "scheduled");
    assert_eq!(msg["sender"], "trigger");

    let (_, v) = env.json(&["query", "SELECT count(*) FROM worker_events WHERE event = 'trigger'"]);
    assert_eq!(v["rows"][0][0], 2, "{v}");
    assert_eq!(outcomes(&env), ["no_target", "started", "signalled", "signalled", "muted"]);
}

fn ls(env: &Env) -> Value {
    let (code, v) = env.json(&["triggers", "ls"]);
    assert_eq!(code, 0, "{v}");
    v
}

fn canonical(p: std::path::PathBuf) -> String {
    p.canonicalize().unwrap().display().to_string()
}

#[test]
fn projects_register_themselves_and_their_triggers_are_listed() {
    let env = Env::new();
    write_wf(&env, "nightly", "triggers:\n  - manual\n  - cron: \"0 2 * * *\"\n", "## Go\nGo.\n");
    let global = env.home().join("workflows");
    fs::create_dir_all(&global).unwrap();
    fs::write(global.join("inbox.md"), "---\nname: inbox\ntriggers:\n  - file: \"~/inbox/*.txt\"\n---\n## Go\nGo.\n").unwrap();
    env.start_daemon();

    // Any command in the project registers it.
    let v = ls(&env);
    let project = canonical(env.project());
    assert_eq!(v["projects"][0]["path"], project.as_str(), "{v}");
    assert_eq!(v["projects"][0]["enabled"], true);
    let t = &v["projects"][0]["triggers"];
    assert_eq!(t.as_array().unwrap().len(), 1, "manual triggers aren't armed: {v}");
    assert_eq!((t[0]["workflow"].as_str(), t[0]["trigger"].as_str()), (Some("nightly"), Some("cron 0 2 * * *")));
    assert!(t[0]["last"].is_null());
    assert_eq!(v["global"]["triggers"][0]["trigger"], "file ~/inbox/*.txt");

    // The last fire shows up.
    env.json(&["triggers", "fire", "nightly"]);
    let v = ls(&env);
    assert_eq!(v["projects"][0]["triggers"][0]["last"]["outcome"], "started", "{v}");
    let human = String::from_utf8_lossy(&env.run(&["triggers", "ls"]).stdout).to_string();
    assert!(human.contains("nightly") && human.contains("started (run 1"), "{human}");

    // Disabling sticks across a daemon restart.
    let (code, v) = env.json(&["triggers", "disable"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(env.json(&["daemon", "stop"]).0, 0);
    env.start_daemon();
    assert_eq!(ls(&env)["projects"][0]["enabled"], false);
    env.json(&["triggers", "enable", "--project", "."]);
    assert_eq!(ls(&env)["projects"][0]["enabled"], true);
}

#[test]
fn workflows_that_turn_invalid_are_disarmed_and_recorded() {
    let env = Env::new();
    write_wf(&env, "nightly", "triggers:\n  - cron: \"0 2 * * *\"\n", "## Go\nGo.\n");
    env.start_daemon();
    assert_eq!(ls(&env)["projects"][0]["triggers"].as_array().unwrap().len(), 1);

    write_wf(&env, "nightly", "triggers:\n  - cron: \"0 25 * * *\"\n", "## Go\nGo.\n");
    common::eventually("error recorded", || {
        let (_, v) = env.json(&["query", "SELECT message FROM trigger_fires WHERE trigger_index = -1"]);
        v["rows"][0][0].as_str().is_some_and(|m| m.contains("hour `25` is out of range"))
    });
    let v = ls(&env);
    assert!(v["projects"][0]["triggers"].as_array().unwrap().is_empty(), "{v}");
    let e = &v["projects"][0]["errors"][0];
    assert_eq!(e["workflow"], "nightly");
    assert_eq!(e["last"]["outcome"], "error");
    let human = String::from_utf8_lossy(&env.run(&["triggers", "ls"]).stdout).to_string();
    assert!(human.contains("nightly: not armed: workflow is invalid"), "{human}");

    // Fixed again: re-armed.
    write_wf(&env, "nightly", "triggers:\n  - cron: \"0 3 * * *\"\n", "## Go\nGo.\n");
    let v = ls(&env);
    assert_eq!(v["projects"][0]["triggers"][0]["trigger"], "cron 0 3 * * *", "{v}");
}

#[test]
fn missing_projects_are_dropped() {
    let env = Env::new();
    env.start_daemon();
    let other = env.home().parent().unwrap().join("other");
    fs::create_dir_all(other.join(".tome/workflows")).unwrap();
    let (code, v) = env.json(&["triggers", "disable", "--project", other.to_str().unwrap()]);
    assert_eq!(code, 0, "{v}");
    let listed = |env: &Env| -> Vec<String> {
        ls(env)["projects"].as_array().unwrap().iter().map(|p| p["path"].as_str().unwrap().to_string()).collect()
    };
    assert!(listed(&env).contains(&canonical(other.clone())));
    fs::remove_dir_all(&other).unwrap();
    assert!(!listed(&env).iter().any(|p| p.ends_with("/other")));

    // Worktrees inside `.tome/` never register as projects.
    let wt = env.project().join(".tome/worktrees/1-a");
    fs::create_dir_all(wt.join(".tome/workflows")).unwrap();
    env.cmd(&["runs", "list"]).current_dir(&wt).output().unwrap();
    assert!(!listed(&env).iter().any(|p| p.contains("worktrees")));
}

fn start_fast_daemon(env: &Env) {
    let out = env.cmd(&["daemon", "start"]).env("TOME_TRIGGER_TICK_MS", "100").output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

fn runs(env: &Env) -> Vec<Value> {
    env.json(&["runs", "list"]).1["runs"].as_array().cloned().unwrap_or_default()
}

#[test]
fn file_changes_start_runs_in_the_project() {
    let env = Env::new();
    write_wf(
        &env,
        "specs",
        "triggers:\n  - file: \"specs/**/*.md\"\n    debounce: 1\n    ignore: [\"specs/drafts/**\"]\n",
        "## Go\nChanged: {{trigger.paths}}\n",
    );
    start_fast_daemon(&env);
    // Registers the project; the glob matches nothing yet.
    assert_eq!(ls(&env)["projects"][0]["triggers"][0]["trigger"], "file specs/**/*.md");
    std::thread::sleep(std::time::Duration::from_millis(500));

    fs::create_dir_all(env.project().join("specs/drafts")).unwrap();
    fs::write(env.project().join("specs/a.md"), "a").unwrap();
    fs::write(env.project().join("specs/b.md"), "b").unwrap();
    fs::write(env.project().join("specs/drafts/x.md"), "x").unwrap();
    common::eventually("a triggered run", || !runs(&env).is_empty());
    let run = &runs(&env)[0];
    assert_eq!(canonical(env.project()), run["project_path"].as_str().unwrap(), "runs start in the project root");
    let (_, show) = env.json(&["runs", "show", &run["id"].to_string(), "--snapshot"]);
    let snap = show["run"]["workflow_snapshot"].as_str().unwrap();
    assert!(snap.contains("Changed: specs/a.md (created), specs/b.md (created)"), "one batched event: {snap}");
    assert_eq!(show["run"]["trigger"]["synthetic"], false);

    // Muted while that run is active: this change is dropped, not queued.
    fs::write(env.project().join("specs/a.md"), "edited by an agent").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let (code, _) = env.json(&["run", "finish", "--status", "succeeded", "--run", &run["id"].to_string()]);
    assert_eq!(code, 0);
    std::thread::sleep(std::time::Duration::from_millis(1500));
    assert_eq!(runs(&env).len(), 1, "muted changes don't fire later");

    fs::write(env.project().join("specs/drafts/y.md"), "ignored").unwrap();
    fs::write(env.project().join("specs/b.md"), "edited").unwrap();
    common::eventually("a second run", || runs(&env).len() == 2);
    let (_, show) = env.json(&["runs", "show", "2", "--snapshot"]);
    let snap = show["run"]["workflow_snapshot"].as_str().unwrap();
    assert!(snap.contains("Changed: specs/b.md (modified)"), "{snap}");
}
