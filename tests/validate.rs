mod common;

use common::Env;
use std::fs;

fn write(dir: &std::path::Path, file: &str, body: &str) {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join(file), body).unwrap();
}

#[test]
fn validates_all_and_project_overrides_global() {
    let env = Env::new();
    let global = env.home().join("workflows");
    let project = env.project().join(".tome/workflows");
    write(&global, "review.md", "---\nname: review\ndescription: global\n---\nbody\n");
    write(&project, "review.md", "---\nname: review\ndescription: project\n---\nbody\n");

    let (code, v) = env.json(&["validate"]);
    assert_eq!(code, 0, "{v}");
    let wfs = v["workflows"].as_array().unwrap();
    assert_eq!(wfs.len(), 2);
    assert_eq!(wfs[0]["scope"], "global");
    assert!(wfs[0]["overridden_by"].as_str().unwrap().ends_with(".tome/workflows/review.md"));

    // Lookup by name resolves the project copy.
    let (code, v) = env.json(&["validate", "review"]);
    assert_eq!(code, 0);
    assert!(v["workflows"][0]["path"].as_str().unwrap().contains("/p/.tome/"));
}

#[test]
fn project_lookup_walks_up_from_subdirectories() {
    let env = Env::new();
    write(&env.project().join(".tome/workflows"), "a.md", "---\nname: a\n---\n");
    let sub = env.project().join("src/deep");
    fs::create_dir_all(&sub).unwrap();
    let out = env.cmd(&["--json", "validate", "a"]).current_dir(&sub).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stdout));
}

#[test]
fn invalid_workflow_exits_2_with_line_numbers() {
    let env = Env::new();
    write(
        &env.project().join(".tome/workflows"),
        "bad.md",
        "---\nname: bad\nconcurency: 2\n---\n## Step\nuse {{params.base}}\n",
    );
    let (code, v) = env.json(&["validate", "bad"]);
    assert_eq!(code, 2);
    assert_eq!(v["valid"], false);
    let errors = v["workflows"][0]["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 2);
    assert_eq!(errors[0]["line"], 3);
    assert!(errors[0]["message"].as_str().unwrap().contains("concurency"));
    assert_eq!(errors[1]["line"], 6);

    // Human output: the path on the header, line numbers under it.
    let out = env.run(&["validate", "bad"]);
    assert_eq!(out.status.code(), Some(2));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("bad.md\n"), "{text}");
    assert!(text.contains("        line 3: unknown frontmatter key `concurency`"), "{text}");
}

#[test]
fn validate_does_not_need_the_daemon_and_checks_params() {
    let env = Env::new();
    let path = env.project().join("wf.md");
    fs::write(&path, "---\nname: wf\nparams:\n  n: {type: int}\n---\n{{params.n}}\n").unwrap();
    let (code, _) = env.json(&["validate", "./wf.md"]);
    assert_eq!(code, 0);
    let (code, v) = env.json(&["validate", "./wf.md", "--param", "n=abc"]);
    assert_eq!(code, 2, "{v}");
    let (code, _) = env.json(&["validate", "./wf.md", "--param", "n=5"]);
    assert_eq!(code, 0);
}

#[test]
fn unknown_workflow_exits_4() {
    let env = Env::new();
    let (code, v) = env.json(&["validate", "nope"]);
    assert_eq!(code, 4);
    assert_eq!(v["error"]["kind"], "not_found");
}

#[test]
fn duplicate_names_in_one_scope_are_invalid() {
    let env = Env::new();
    let dir = env.project().join(".tome/workflows");
    write(&dir, "one.md", "---\nname: dup\n---\n");
    write(&dir, "two.md", "---\nname: dup\n---\n");
    let (code, _) = env.json(&["validate"]);
    assert_eq!(code, 2);
    let (code, v) = env.json(&["validate", "dup"]);
    assert_eq!(code, 2);
    assert_eq!(v["error"]["kind"], "invalid_workflow");
}

#[test]
fn trigger_validation() {
    let env = Env::new();
    let global = env.home().join("workflows");
    let project = env.project().join(".tome/workflows");
    let wf = |name: &str, triggers: &str| format!("---\nname: {name}\ntriggers:\n{triggers}---\nfired by {{{{trigger.kind}}}} at {{{{trigger.time}}}}\n");
    write(&project, "ok.md", &wf("ok", "  - manual\n  - file: \"specs/**/*.md\"\n  - cron: \"0 9 * * 1-5\"\n    to: running-or-new\n"));
    write(&global, "home.md", &wf("home", "  - file: \"~/inbox/*.md\"\n"));
    let (code, v) = env.json(&["validate"]);
    assert_eq!(code, 0, "{v}");

    // A relative glob is fine in a project, but not in a global workflow.
    write(&global, "rel.md", &wf("rel", "  - file: \"specs/*.md\"\n"));
    let (code, v) = env.json(&["validate", "rel"]);
    assert_eq!(code, 2, "{v}");
    let err = &v["workflows"][0]["errors"][0];
    assert_eq!(err["line"], 4);
    assert!(err["message"].as_str().unwrap().contains("absolute or `~/`"), "{err}");

    write(&project, "bad.md", &wf("bad", "  - cron: \"0 25 * * *\"\n  - file: \"*.md\"\n    to: running\n"));
    let (code, v) = env.json(&["validate", "bad"]);
    assert_eq!(code, 2);
    let lines: Vec<i64> = v["workflows"][0]["errors"].as_array().unwrap().iter().map(|e| e["line"].as_i64().unwrap()).collect();
    assert_eq!(lines, vec![4, 5], "{v}");
}
