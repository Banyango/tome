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

    assert_ne!(env.json(&["queue", "ls", "--global"]).0, 0, "not attached to global");
}
