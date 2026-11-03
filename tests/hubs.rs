mod common;

use common::Env;
use std::fs;

fn env_with_project() -> Env {
    let env = Env::new();
    fs::create_dir_all(env.project().join(".tome/workflows")).unwrap();
    env.start_daemon();
    env
}

#[test]
fn hubs_are_created_attached_and_removed() {
    let env = env_with_project();

    let (code, v) = env.json(&["hub", "attach", "release"]);
    assert_ne!(code, 0, "attaching to a missing hub fails: {v}");
    assert!(v.to_string().contains("tome hub create release"), "{v}");

    let (code, v) = env.json(&["hub", "create", "global"]);
    assert_ne!(code, 0, "{v}");

    let (code, v) = env.json(&["hub", "create", "release"]);
    assert_eq!(code, 0, "{v}");
    assert!(env.home().join("hubs/release/workflows").is_dir());
    assert_ne!(
        env.json(&["hub", "create", "release"]).0,
        0,
        "exists already"
    );

    let (code, v) = env.json(&["hub", "attach", "release"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["changed"], true);
    assert_eq!(env.json(&["hub", "attach", "--global"]).0, 0);
    assert_eq!(env.json(&["hub", "attach", "release"]).1["changed"], false);

    let (_, v) = env.json(&["hub", "show"]);
    assert_eq!(v["attached"], serde_json::json!(["hub release", "global"]));
    let (_, v) = env.json(&["hub", "show", "release"]);
    assert_eq!(v["projects"].as_array().unwrap().len(), 1);
    let (_, v) = env.json(&["hub", "ls"]);
    let names: Vec<&str> = v["hubs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["release", "global"]);

    // Nothing is written into the repo.
    assert_eq!(
        fs::read_dir(env.project().join(".tome")).unwrap().count(),
        1
    );

    let (code, v) = env.json(&["hub", "rm", "release"]);
    assert_ne!(code, 0, "refused while attached: {v}");
    assert!(v.to_string().contains("attached"), "{v}");

    assert_eq!(env.json(&["hub", "detach", "release"]).0, 0);
    let (code, v) = env.json(&["hub", "rm", "release"]);
    assert_eq!(code, 0, "{v}");
    assert!(!env.home().join("hubs/release").exists());
}

#[test]
fn hub_and_global_buses_need_an_attached_project() {
    let env = env_with_project();
    assert_eq!(env.json(&["hub", "create", "release"]).0, 0);

    let (code, v) = env.json(&["publish", "--hub", "release", "build.done", "x"]);
    assert_ne!(code, 0, "not attached yet: {v}");
    assert!(v.to_string().contains("tome hub attach release"), "{v}");
    let (code, v) = env.json(&["publish", "--hub", "nope", "build.done", "x"]);
    assert_ne!(code, 0, "{v}");
    assert!(v.to_string().contains("tome hub create nope"), "{v}");
    let (code, v) = env.json(&["events", "ls", "--global"]);
    assert_ne!(code, 0, "{v}");
    assert!(v.to_string().contains("tome hub attach --global"), "{v}");

    assert_eq!(env.json(&["hub", "attach", "release"]).0, 0);
    let (code, v) = env.json(&["publish", "--hub", "release", "tome.run.x", "x"]);
    assert_ne!(code, 0, "reserved on every bus: {v}");

    let (code, v) = env.json(&["publish", "--hub", "release", "build.done", "hello"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["bus"], "hub release");
    let project = env.project().canonicalize().unwrap().display().to_string();
    assert_eq!(v["event"]["publisher"], project.as_str());
    assert_eq!(v["event"]["project_path"], "hub:release");

    // On the hub's bus only, not the project's.
    let (_, v) = env.json(&["events", "ls", "--hub", "release"]);
    assert_eq!(v["topics"][0]["topic"], "build.done", "{v}");
    let (_, v) = env.json(&["events", "ls"]);
    assert_eq!(v["topics"].as_array().unwrap().len(), 0, "{v}");
    let (_, v) = env.json(&["events", "show", "build.done", "--hub", "release", "--all"]);
    assert_eq!(v["events"][0]["publisher"], project.as_str(), "{v}");

    let id = v["events"][0]["id"].as_i64().unwrap().to_string();
    let (code, v) = env.json(&["events", "remove", &id]);
    assert_ne!(code, 0, "not on the project's bus: {v}");
    let (code, v) = env.json(&["events", "remove", &id, "--hub", "release"]);
    assert_eq!(code, 0, "{v}");
}

/// `--json` output of `tome` run in `dir`.
fn json_in(env: &Env, dir: &std::path::Path, args: &[&str]) -> (i32, serde_json::Value) {
    let mut full = vec!["--json"];
    full.extend_from_slice(args);
    let out = env.cmd(&full).current_dir(dir).output().unwrap();
    let v = serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    (out.status.code().unwrap(), v)
}

#[test]
fn hub_queues_are_shared_by_attached_projects() {
    let env = env_with_project();
    let other = env.project().parent().unwrap().join("q");
    fs::create_dir_all(other.join(".tome/workflows")).unwrap();
    assert_eq!(env.json(&["hub", "create", "release"]).0, 0);

    let (code, v) = env.json(&["queue", "push", "jobs", "a", "--hub", "release"]);
    assert_ne!(code, 0, "not attached: {v}");
    assert!(v.to_string().contains("tome hub attach release"), "{v}");

    assert_eq!(env.json(&["hub", "attach", "release"]).0, 0);
    assert_eq!(json_in(&env, &other, &["hub", "attach", "release"]).0, 0);
    let (code, v) = env.json(&["queue", "push", "jobs", "a", "--hub", "release"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(env.json(&["queue", "push", "jobs", "mine"]).0, 0);

    // The other project pulls it from the hub, not from its own queues.
    assert_eq!(json_in(&env, &other, &["queue", "pull", "jobs"]).0, 3);
    let (code, v) = json_in(&env, &other, &["queue", "pull", "jobs", "--hub", "release"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["message"]["body"], "a", "{v}");
    let id = v["message"]["id"].to_string();
    let (code, v) = json_in(&env, &other, &["queue", "ack", &id, "--hub", "release"]);
    assert_eq!(code, 0, "{v}");

    let (_, v) = env.json(&["queue", "ls", "--hub", "release"]);
    assert_eq!(v["scope"], "hub release", "{v}");
    assert_eq!(v["queues"][0]["name"], "jobs", "{v}");
    let (_, v) = env.json(&["queue", "peek", "jobs"]);
    assert_eq!(v["messages"][0]["body"], "mine", "{v}");

    assert_ne!(
        env.json(&["queue", "ls", "--global"]).0,
        0,
        "not attached to global"
    );
}

#[test]
fn hub_events_are_delivered_into_each_attached_project() {
    let env = env_with_project();
    let other = env.project().parent().unwrap().join("q");
    fs::create_dir_all(other.join(".tome/workflows")).unwrap();
    assert_eq!(env.json(&["hub", "create", "release"]).0, 0);
    let wf = "---\nname: ship\ntriggers:\n  - on: build.*\n    hub: release\n---\n## Ship\nFrom {{trigger.publisher}}.\n";
    let own = "---\nname: own\ntriggers:\n  - on: build.*\n---\n## Own\nGo.\n";
    for dir in [env.project().to_path_buf(), other.clone()] {
        fs::write(dir.join(".tome/workflows/ship.md"), wf).unwrap();
        fs::write(dir.join(".tome/workflows/own.md"), own).unwrap();
    }
    assert_eq!(env.json(&["triggers", "disable"]).0, 0);
    assert_eq!(json_in(&env, &other, &["triggers", "disable"]).0, 0);
    assert_eq!(env.json(&["hub", "attach", "release"]).0, 0);
    assert_eq!(json_in(&env, &other, &["hub", "attach", "release"]).0, 0);

    let (code, v) = env.json(&["publish", "--hub", "release", "build.done", "x"]);
    assert_eq!(code, 0, "{v}");
    let ds = v["deliveries"].as_array().unwrap();
    assert_eq!(
        ds.len(),
        2,
        "one per attached project, not the own-bus workflow: {v}"
    );
    let mut projects: Vec<&str> = ds
        .iter()
        .map(|d| d["project_path"].as_str().unwrap())
        .collect();
    projects.sort();
    let mut want = vec![
        env.project().canonicalize().unwrap().display().to_string(),
        other.canonicalize().unwrap().display().to_string(),
    ];
    want.sort();
    assert_eq!(projects, want, "{v}");
    assert!(
        ds.iter()
            .all(|d| d["workflow"] == "ship" && d["pattern"] == "build.* (hub release)"),
        "{v}"
    );

    // The project's own bus doesn't reach hub subscribers.
    let (_, v) = env.json(&["publish", "build.done", "x"]);
    let ds = v["deliveries"].as_array().unwrap();
    assert_eq!(ds.len(), 1, "{v}");
    assert_eq!(ds[0]["workflow"], "own", "{v}");
}

#[test]
fn a_trigger_on_a_missing_hub_is_invalid() {
    let env = env_with_project();
    fs::write(
        env.project().join(".tome/workflows/ship.md"),
        "---\nname: ship\ntriggers:\n  - on: build.*\n    hub: nope\n---\n## Ship\nGo.\n",
    )
    .unwrap();
    let (code, v) = env.json(&["validate"]);
    assert_ne!(code, 0, "{v}");
    assert!(v.to_string().contains("tome hub create"), "{v}");
}

fn hub_wf(env: &Env, hub: &str, name: &str, front: &str) {
    let dir = env.home().join("hubs").join(hub).join("workflows");
    fs::write(
        dir.join(format!("{name}.md")),
        format!("---\nname: {name}\n{front}---\n## Go\nGo.\n"),
    )
    .unwrap();
}

#[test]
fn hub_workflows_apply_to_attached_projects() {
    let env = env_with_project();
    let other = env.project().parent().unwrap().join("q");
    fs::create_dir_all(other.join(".tome/workflows")).unwrap();
    for hub in ["release", "ops"] {
        assert_eq!(env.json(&["hub", "create", hub]).0, 0);
    }
    hub_wf(&env, "release", "ship", "triggers:\n  - on: build.done\n");
    hub_wf(&env, "release", "tidy", "");
    fs::write(
        env.project().join(".tome/workflows/tidy.md"),
        "---\nname: tidy\n---\n## Mine\nGo.\n",
    )
    .unwrap();

    let (_, v) = env.json(&["workflow", "ls"]);
    assert!(!v.to_string().contains("ship"), "not attached yet: {v}");

    assert_eq!(env.json(&["triggers", "disable"]).0, 0);
    assert_eq!(json_in(&env, &other, &["triggers", "disable"]).0, 0);
    assert_eq!(env.json(&["hub", "attach", "release"]).0, 0);
    assert_eq!(json_in(&env, &other, &["hub", "attach", "release"]).0, 0);
    let (code, v) = env.json(&["validate"]);
    assert_eq!(code, 0, "{v}");
    let scopes: Vec<(String, String)> = v["workflows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| {
            (
                w["name"].as_str().unwrap().into(),
                w["scope"].as_str().unwrap().into(),
            )
        })
        .collect();
    assert!(scopes.contains(&("ship".into(), "hub".into())), "{v}");
    let hub_tidy = v["workflows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["name"] == "tidy" && w["scope"] == "hub")
        .unwrap();
    assert!(
        hub_tidy["overridden_by"].is_string(),
        "the project's wins: {v}"
    );

    // An `on:` without `hub:` listens on each project's own bus.
    let (_, v) = env.json(&["publish", "build.done", "x"]);
    let ds = v["deliveries"].as_array().unwrap();
    assert_eq!(ds.len(), 1, "{v}");
    assert_eq!(ds[0]["workflow"], "ship", "{v}");
    let (_, v) = json_in(&env, &other, &["publish", "build.done", "x"]);
    assert_eq!(v["deliveries"].as_array().unwrap().len(), 1, "{v}");

    // Two attached hubs with one name: an error everywhere it's used.
    hub_wf(&env, "ops", "ship", "");
    assert_eq!(env.json(&["hub", "attach", "ops"]).0, 0);
    let (code, v) = env.json(&["validate"]);
    assert_ne!(code, 0, "{v}");
    assert!(v.to_string().contains("more than one attached hub"), "{v}");
    let (code, v) = env.json(&["run", "ship"]);
    assert_ne!(code, 0, "{v}");
    assert!(v.to_string().contains("defined more than once"), "{v}");
    let (_, v) = env.json(&["triggers", "ls"]);
    assert!(v.to_string().contains("more than one attached hub"), "{v}");
}

#[test]
fn hub_config_layers_between_global_and_project() {
    let env = env_with_project();
    assert_eq!(env.json(&["hub", "create", "release"]).0, 0);
    assert_eq!(env.json(&["hub", "attach", "release"]).0, 0);
    let cfg = env.home().join("hubs/release/config.yaml");
    fs::write(&cfg, "nodes: {}\n").unwrap();
    let (code, v) = env.json(&["validate"]);
    assert_ne!(code, 0, "nodes are the user's: {v}");
    assert!(v.to_string().contains("goes in the global config"), "{v}");
    fs::write(&cfg, "bus:\n  forward:\n    - bad: 1\n").unwrap();
    let (_, v) = env.json(&["validate"]);
    assert!(
        v["bus"]["errors"]
            .to_string()
            .contains("hubs/release/config.yaml"),
        "hub forwarding rules are read: {v}"
    );
}

#[test]
fn detach_returns_claims_and_rm_refuses_pending_work() {
    let env = env_with_project();
    assert_eq!(env.json(&["hub", "create", "release"]).0, 0);
    assert_eq!(env.json(&["hub", "attach", "release"]).0, 0);
    assert_eq!(
        env.json(&["queue", "push", "jobs", "a", "--hub", "release"])
            .0,
        0
    );
    assert_eq!(
        env.json(&["queue", "pull", "jobs", "--hub", "release"]).0,
        0
    );

    let (code, v) = env.json(&["hub", "detach", "release"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["released"], 1, "{v}");

    let (code, v) = env.json(&["hub", "rm", "release"]);
    assert_ne!(code, 0, "refused while a message is queued: {v}");
    assert!(v.to_string().contains("queue jobs (1)"), "{v}");

    assert_eq!(env.json(&["hub", "attach", "release"]).0, 0);
    let (code, v) = env.json(&["hub", "rm", "release", "--force"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["detached"].as_array().unwrap().len(), 1, "{v}");
    assert!(!env.home().join("hubs/release").exists());
    let (_, v) = env.json(&["hub", "show"]);
    assert_eq!(v["attached"], serde_json::json!([]), "{v}");
}
