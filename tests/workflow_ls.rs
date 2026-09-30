mod common;

use common::Env;

#[test]
fn ls_lists_project_workflows_first_and_marks_shadowed_globals() {
    let env = Env::new();
    env.json(&["workflow", "new", "tidy", "--global"]);
    env.json(&["workflow", "new", "old", "--global"]);
    env.json(&["workflow", "new", "ship", "-d", "Ship it"]);
    env.json(&["workflow", "new", "tidy"]);

    let (code, out) = env.json(&["workflow", "ls"]);
    assert_eq!(code, 0, "{out}");
    let got: Vec<(&str, &str, bool)> = out["workflows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| {
            (
                w["name"].as_str().unwrap(),
                w["scope"].as_str().unwrap(),
                w["shadowed"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("ship", "project", false),
            ("tidy", "project", false),
            ("old", "global", false),
            ("tidy", "global", true),
        ]
    );
    assert_eq!(out["workflows"][0]["description"], "Ship it");

    let (_, out) = env.json(&["workflow", "ls", "--global"]);
    assert_eq!(out["workflows"].as_array().unwrap().len(), 2);
}

#[test]
fn ls_with_nothing_says_so() {
    let env = Env::new();
    let (code, out) = env.json(&["workflow", "ls"]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(out["workflows"], serde_json::json!([]));
}
