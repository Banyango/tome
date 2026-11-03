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
