mod common;

use common::Env;
use std::fs;

#[test]
fn new_workflow_is_valid_and_runnable_by_name() {
    let env = Env::new();
    let (code, out) = env.json(&["workflow", "new", "ship", "-d", "Ship it: fast"]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(out["scope"], "project");
    let path = env.project().join(".tome/workflows/ship.md");
    assert_eq!(out["path"], path.canonicalize().unwrap().to_str().unwrap());
    let src = fs::read_to_string(&path).unwrap();
    assert!(src.contains("description: \"Ship it: fast\""), "{src}");

    let (code, out) = env.json(&["validate", "ship"]);
    assert_eq!(code, 0, "{out}");

    env.start_daemon();
    let (code, run) = env.json(&["run", "ship", "--detach", "--param", "base=dev"]);
    assert_eq!(code, 0, "{run}");
    assert_eq!(run["workflow_name"], "ship");
    assert!(run["workflow_snapshot"].as_str().unwrap().contains("starting from dev"));
}

#[test]
fn global_flag_writes_to_tome_home() {
    let env = Env::new();
    let (code, out) = env.json(&["workflow", "new", "tidy", "--global"]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(out["scope"], "global");
    assert!(env.home().join("workflows/tidy.md").is_file());

    // A project workflow of the same name is allowed; it overrides the global one.
    let (code, out) = env.json(&["workflow", "new", "tidy"]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(out["shadows"], env.home().join("workflows/tidy.md").to_str().unwrap());
}

#[test]
fn uses_the_nearest_project_directory() {
    let env = Env::new();
    fs::create_dir_all(env.project().join(".tome/workflows")).unwrap();
    let sub = env.project().join("src/deep");
    fs::create_dir_all(&sub).unwrap();
    let out = env.cmd(&["--json", "workflow", "new", "deep"]).current_dir(&sub).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(env.project().join(".tome/workflows/deep.md").is_file());
    assert!(!sub.join(".tome").exists());
}

#[test]
fn refuses_to_clobber_or_duplicate() {
    let env = Env::new();
    assert_eq!(env.json(&["workflow", "new", "ship"]).0, 0);
    let (code, err) = env.json(&["workflow", "new", "ship"]);
    assert_eq!(code, 2);
    assert!(err["error"]["message"].as_str().unwrap().contains("already exists"), "{err}");
    assert_eq!(env.json(&["workflow", "new", "ship", "--force"]).0, 0);

    // Same `name:` in a differently named file would make `tome run` ambiguous.
    let dir = env.project().join(".tome/workflows");
    fs::write(dir.join("other.md"), "---\nname: dup\n---\nhi\n").unwrap();
    let (code, err) = env.json(&["workflow", "new", "dup"]);
    assert_eq!(code, 2);
    assert!(err["error"]["message"].as_str().unwrap().contains("other.md"), "{err}");
    assert!(!dir.join("dup.md").exists());
}

#[test]
fn rejects_bad_names() {
    let env = Env::new();
    for name in ["1st", "a b", "../x", "a.b"] {
        let (code, _) = env.json(&["workflow", "new", name]);
        assert_eq!(code, 2, "{name}");
    }
    assert!(!env.project().join(".tome").exists());
}
