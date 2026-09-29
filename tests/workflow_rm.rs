mod common;

use common::Env;
use serde_json::json;
use std::fs;

fn msg(err: &serde_json::Value) -> &str {
    err["error"]["message"].as_str().unwrap()
}

fn hint(err: &serde_json::Value) -> &str {
    err["error"]["hint"].as_str().unwrap_or_default()
}

#[test]
fn removes_a_project_workflow_by_name() {
    let env = Env::new();
    env.start_daemon();
    assert_eq!(env.json(&["workflow", "new", "ship"]).0, 0);
    let dir = env.project().join(".tome/workflows");
    let path = dir.join("ship.md").canonicalize().unwrap();

    let (code, out) = env.json(&["workflow", "rm", "ship"]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(out["name"], "ship");
    assert_eq!(out["scope"], "project");
    assert_eq!(out["path"], path.to_str().unwrap());
    assert!(out["falls_back_to"].is_null(), "{out}");
    assert!(!path.exists());
    // Only the file goes; the directory stays.
    assert!(dir.is_dir());

    let (code, err) = env.json(&["validate", "ship"]);
    assert_eq!(code, 4, "{err}");
}

#[test]
fn global_flag_picks_the_global_library_and_never_crosses_scopes() {
    let env = Env::new();
    env.start_daemon();
    assert_eq!(env.json(&["workflow", "new", "tidy", "--global"]).0, 0);
    let global = env.home().join("workflows/tidy.md");

    // Without --global only the project is searched; the hint points across.
    let (code, err) = env.json(&["workflow", "rm", "tidy"]);
    assert_eq!(code, 4, "{err}");
    assert!(msg(&err).contains("no project workflow named `tidy`"), "{err}");
    assert!(hint(&err).contains("pass --global"), "{err}");
    assert!(global.exists());

    let (code, out) = env.json(&["workflow", "rm", "tidy", "--global"]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(out["scope"], "global");
    assert!(!global.exists());

    // And the other way round.
    assert_eq!(env.json(&["workflow", "new", "ship"]).0, 0);
    let (code, err) = env.json(&["workflow", "rm", "ship", "--global"]);
    assert_eq!(code, 4, "{err}");
    assert!(hint(&err).contains("drop --global"), "{err}");
    assert!(env.project().join(".tome/workflows/ship.md").exists());
}

#[test]
fn removing_a_project_override_notes_the_global_fallback() {
    let env = Env::new();
    env.start_daemon();
    assert_eq!(env.json(&["workflow", "new", "tidy", "--global"]).0, 0);
    assert_eq!(env.json(&["workflow", "new", "tidy"]).0, 0);

    let (code, out) = env.json(&["workflow", "rm", "tidy"]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(out["falls_back_to"], env.home().join("workflows/tidy.md").to_str().unwrap());
    assert!(env.home().join("workflows/tidy.md").exists());

    // The human text says so too.
    assert_eq!(env.json(&["workflow", "new", "tidy"]).0, 0);
    let out = env.run(&["workflow", "rm", "tidy"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("removed workflow `tidy`"), "{text}");
    assert!(text.contains("`tome run tidy` now uses the global workflow at"), "{text}");
}

#[test]
fn refuses_an_ambiguous_name() {
    let env = Env::new();
    env.start_daemon();
    let dir = env.project().join(".tome/workflows");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("a.md"), "---\nname: dup\n---\nhi\n").unwrap();
    fs::write(dir.join("b.md"), "---\nname: dup\n---\nhi\n").unwrap();

    let (code, err) = env.json(&["workflow", "rm", "dup"]);
    assert_eq!(code, 2, "{err}");
    assert!(msg(&err).contains("a.md") && msg(&err).contains("b.md"), "{err}");
    assert!(hint(&err).contains("path"), "{err}");
    assert!(dir.join("a.md").exists() && dir.join("b.md").exists());

    // Removing one by path leaves the other.
    let (code, out) = env.json(&["workflow", "rm", ".tome/workflows/b.md"]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(out["name"], "dup");
    assert!(dir.join("a.md").exists() && !dir.join("b.md").exists());
}

#[test]
fn removes_a_broken_file_by_path() {
    let env = Env::new();
    env.start_daemon();
    let dir = env.project().join(".tome/workflows");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("broken.md"), "---\nname: [oops\n---\n").unwrap();

    // By name it can't be found, but the hint points at the file.
    let (code, err) = env.json(&["workflow", "rm", "broken"]);
    assert_eq!(code, 4, "{err}");
    assert!(hint(&err).contains("broken.md"), "{err}");

    let abs = dir.join("broken.md");
    let (code, out) = env.json(&["workflow", "rm", abs.to_str().unwrap()]);
    assert_eq!(code, 0, "{out}");
    assert!(out["name"].is_null(), "{out}");
    assert_eq!(out["scope"], "project");
    assert!(!abs.exists());
}

#[test]
fn a_global_path_works_with_or_without_the_flag() {
    let env = Env::new();
    env.start_daemon();
    assert_eq!(env.json(&["workflow", "new", "a", "--global"]).0, 0);
    assert_eq!(env.json(&["workflow", "new", "b", "--global"]).0, 0);
    let a = env.home().join("workflows/a.md");
    let b = env.home().join("workflows/b.md");

    let (code, out) = env.json(&["workflow", "rm", a.to_str().unwrap()]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(out["scope"], "global");
    let (code, out) = env.json(&["workflow", "rm", b.to_str().unwrap(), "--global"]);
    assert_eq!(code, 0, "{out}");
    assert!(!a.exists() && !b.exists());
}

#[test]
fn refuses_bad_paths() {
    let env = Env::new();
    assert_eq!(env.json(&["workflow", "new", "ship"]).0, 0);
    let wf = env.project().join(".tome/workflows/ship.md");

    // A project path with --global contradicts itself.
    let (code, err) = env.json(&["workflow", "rm", wf.to_str().unwrap(), "--global"]);
    assert_eq!(code, 2, "{err}");
    assert!(msg(&err).contains("--global"), "{err}");
    assert!(wf.exists());

    // Not inside a workflows directory.
    fs::write(env.project().join("notes.md"), "---\nname: notes\n---\nhi\n").unwrap();
    let (code, err) = env.json(&["workflow", "rm", "notes.md"]);
    assert_eq!(code, 2, "{err}");
    assert!(msg(&err).contains("not a workflow file"), "{err}");
    assert!(env.project().join("notes.md").exists());

    // Not a `.md` file.
    let txt = env.project().join(".tome/workflows/ship.txt");
    fs::write(&txt, "hi").unwrap();
    let (code, _) = env.json(&["workflow", "rm", txt.to_str().unwrap()]);
    assert_eq!(code, 2);
    assert!(txt.exists());

    // Missing file.
    let (code, _) = env.json(&["workflow", "rm", ".tome/workflows/nope.md"]);
    assert_eq!(code, 4);
}

#[test]
fn refuses_while_the_file_has_live_runs_unless_forced() {
    let env = Env::new();
    env.start_daemon();
    assert_eq!(env.json(&["workflow", "new", "ship"]).0, 0);
    assert_eq!(env.json(&["workflow", "new", "ship", "--global"]).0, 0);
    let project = env.project().join(".tome/workflows/ship.md").canonicalize().unwrap();
    let global = env.home().join("workflows/ship.md").canonicalize().unwrap();
    let run = env.rpc_ok("run.create", json!({ "workflow_path": project, "project_path": env.project() }));
    let id = run["id"].as_i64().unwrap();

    let (code, err) = env.json(&["workflow", "rm", "ship"]);
    assert_eq!(code, 1, "{err}");
    assert_eq!(err["error"]["kind"], "conflict");
    assert!(msg(&err).contains(&format!("#{id}")), "{err}");
    assert_eq!(err["error"]["details"]["runs"], json!([id]));
    assert!(hint(&err).contains("--force") && hint(&err).contains("tome run cancel"), "{err}");
    assert!(project.exists());

    // The match is on the file, not the name: the global `ship` has no runs.
    let (code, out) = env.json(&["workflow", "rm", "ship", "--global"]);
    assert_eq!(code, 0, "{out}");
    assert!(!global.exists());

    let (code, out) = env.json(&["workflow", "rm", "ship", "--force"]);
    assert_eq!(code, 0, "{out}");
    assert!(!project.exists());

    // The run carries on, and its snapshot is still there to show.
    let (code, shown) = env.json(&["runs", "show", &id.to_string(), "--snapshot"]);
    assert_eq!(code, 0, "{shown}");
    assert_eq!(shown["run"]["status"], "running");
    assert!(shown["run"]["workflow_snapshot"].as_str().is_some_and(|s| s.contains("name: ship")), "{shown}");
}

#[test]
fn finished_runs_do_not_block_removal() {
    let env = Env::new();
    env.start_daemon();
    assert_eq!(env.json(&["workflow", "new", "ship"]).0, 0);
    let path = env.project().join(".tome/workflows/ship.md").canonicalize().unwrap();
    let run = env.rpc_ok("run.create", json!({ "workflow_path": path }));
    let id = run["id"].to_string();
    let (code, out) = env.json(&["run", "finish", "--status", "succeeded", "--run", &id]);
    assert_eq!(code, 0, "{out}");

    let (code, out) = env.json(&["workflow", "rm", "ship"]);
    assert_eq!(code, 0, "{out}");
    assert!(!path.exists());
}

#[test]
fn refuses_without_the_daemon_unless_forced() {
    let env = Env::new();
    assert_eq!(env.json(&["workflow", "new", "ship"]).0, 0);
    let path = env.project().join(".tome/workflows/ship.md");

    let (code, err) = env.json(&["workflow", "rm", "ship"]);
    assert_eq!(code, 3, "{err}");
    assert_eq!(err["error"]["kind"], "daemon_not_running");
    assert!(msg(&err).contains("can't check for live runs: daemon is not running"), "{err}");
    assert!(hint(&err).contains("tome daemon start") && hint(&err).contains("--force"), "{err}");
    assert!(path.exists());

    let (code, out) = env.json(&["workflow", "rm", "ship", "--force"]);
    assert_eq!(code, 0, "{out}");
    assert!(!path.exists());
}
