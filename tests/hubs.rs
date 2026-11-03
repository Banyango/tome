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
