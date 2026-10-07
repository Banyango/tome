mod common;

use common::{eventually, Env};
use serde_json::{json, Value};
use std::fs;
use std::process::Stdio;

const WF: &str = "---\nname: build\nmode: orchestrated\nparams:\n  base: {default: main}\n---\n## Build\nBranch off {{params.base}} as run-{{run.id}}.\n";

fn write_wf(env: &Env, name: &str, source: &str) {
    let dir = env.project().join(".tome/workflows");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{name}.md")), source).unwrap();
}

/// `tome run <args> --detach`, returning the run id.
fn start(env: &Env, args: &[&str]) -> i64 {
    let mut full = vec!["run"];
    full.extend_from_slice(args);
    full.push("--detach");
    let (code, v) = env.json(&full);
    assert_eq!(code, 0, "{v}");
    v["id"].as_i64().unwrap()
}

fn cancel(env: &Env, id: i64) {
    let (code, v) = env.json(&["run", "cancel", &id.to_string()]);
    assert_eq!(code, 0, "{v}");
}

fn end(env: &Env, id: i64, status: &str) {
    let (code, v) = env.json(&[
        "run",
        "finish",
        "--status",
        status,
        "--run",
        &id.to_string(),
    ]);
    assert_eq!(code, 0, "{v}");
}

fn resume(env: &Env, id: i64) -> (i32, Value) {
    env.json(&["run", "resume", &id.to_string(), "--detach"])
}

fn show(env: &Env, id: i64) -> Value {
    env.json(&["runs", "show", &id.to_string(), "--snapshot"]).1
}

#[test]
fn resume_starts_a_linked_run_from_the_old_snapshot() {
    let env = Env::new();
    env.start_daemon();
    write_wf(&env, "build", WF);
    let old = start(&env, &["build", "--param", "base=dev"]);
    cancel(&env, old);
    // Later edits to the file don't reach the resumed run.
    write_wf(&env, "build", &WF.replace("Branch off", "Fork"));

    let (code, new) = resume(&env, old);
    assert_eq!(code, 0, "{new}");
    let id = new["id"].as_i64().unwrap();
    assert_ne!(id, old);
    assert_eq!(new["status"], "running");
    assert_eq!(new["resumed_from"], old);
    assert_eq!(new["params"], json!({ "base": "dev" }));
    assert_eq!(new["mode"], "orchestrated");
    assert!(env.has_session(&format!("tome-{id}-build")));

    let (old_shown, new_shown) = (show(&env, old), show(&env, id));
    let snap = new_shown["run"]["workflow_snapshot"].as_str().unwrap();
    assert_eq!(Some(snap), old_shown["run"]["workflow_snapshot"].as_str());
    assert!(
        snap.contains(&format!("Branch off dev as run-{old}.")),
        "{snap}"
    );
    // The old run is left as it was.
    assert_eq!(old_shown["run"]["status"], "cancelled");
    assert_eq!(old_shown["run"]["reason"], "user_cancelled");

    // Human mode says which run started.
    end(&env, id, "failed");
    let out = env.run(&["run", "resume", &id.to_string(), "--detach"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        format!("run {} running (build)", id + 1)
    );
}

#[test]
fn only_failed_or_cancelled_runs_resume_and_only_once() {
    let env = Env::new();
    env.start_daemon();
    write_wf(&env, "build", WF);

    let live = start(&env, &["build"]);
    let (code, v) = resume(&env, live);
    assert_eq!(code, 2, "{v}");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("still running"),
        "{v}"
    );

    let done = start(&env, &["build"]);
    end(&env, done, "succeeded");
    let (code, v) = resume(&env, done);
    assert_eq!(code, 2, "{v}");

    let failed = start(&env, &["build"]);
    end(&env, failed, "failed");
    let first = resume(&env, failed).1["id"].as_i64().unwrap();
    let (code, v) = resume(&env, failed);
    assert_eq!(code, 2, "{v}");
    assert_eq!(
        v["error"]["message"],
        format!("run {failed} was resumed as {first}")
    );
    assert!(v["error"]["hint"]
        .as_str()
        .unwrap()
        .contains(&format!("tome run resume {first}")));

    // A resumed run that fails can be resumed in turn.
    end(&env, first, "failed");
    let (code, v) = resume(&env, first);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["resumed_from"], first);

    let (code, _) = resume(&env, 999);
    assert_eq!(code, 4);
}

#[test]
fn a_run_whose_project_is_gone_is_refused() {
    let env = Env::new();
    env.start_daemon();
    write_wf(&env, "build", WF);
    let gone = env.home().join("gone");
    fs::create_dir_all(&gone).unwrap();
    let run = env.rpc_ok(
        "run.create",
        json!({
            "workflow_path": env.project().join(".tome/workflows/build.md"),
            "project_path": gone,
        }),
    );
    let id = run["id"].as_i64().unwrap();
    cancel(&env, id);
    fs::remove_dir(&gone).unwrap();

    let (code, v) = resume(&env, id);
    assert_eq!(code, 2, "{v}");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("no longer exists"),
        "{v}"
    );
}

#[test]
fn resume_follows_the_snapshot_concurrency_rule() {
    let env = Env::new();
    env.start_daemon();
    write_wf(&env, "build", &WF.replace("mode:", "concurrency: 1\nmode:"));
    let old = start(&env, &["build"]);
    cancel(&env, old);
    let busy = start(&env, &["build"]);

    let (code, v) = resume(&env, old);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["status"], "queued");
    let queued = v["id"].as_i64().unwrap();
    end(&env, busy, "succeeded");
    assert_eq!(show(&env, queued)["run"]["status"], "running");

    // Under `on_conflict: reject` it's refused instead.
    write_wf(
        &env,
        "strict",
        &WF.replace("name: build", "name: strict")
            .replace("mode:", "concurrency: 1\non_conflict: reject\nmode:"),
    );
    let old = start(&env, &["strict"]);
    cancel(&env, old);
    start(&env, &["strict"]);
    let (code, v) = resume(&env, old);
    assert_eq!(code, 1, "{v}");
    assert_eq!(v["error"]["kind"], "conflict");
}

#[test]
fn placement_flags_carry_over_unless_given() {
    let env = Env::new();
    env.start_daemon();
    write_wf(&env, "build", WF);
    let old = start(&env, &["build", "--layout", "tab"]);
    cancel(&env, old);
    let (_, v) = resume(&env, old);
    assert_eq!(v["placement"]["flags"]["layout"], "tab", "{v}");

    let id = v["id"].as_i64().unwrap();
    cancel(&env, id);
    let (code, v) = env.json(&[
        "run",
        "resume",
        &id.to_string(),
        "--detach",
        "--layout",
        "workspace",
    ]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["placement"]["flags"]["layout"], "workspace", "{v}");
}

#[test]
fn attached_resume_streams_the_new_run() {
    let env = Env::new();
    env.start_daemon();
    write_wf(&env, "build", WF);
    let old = start(&env, &["build"]);
    cancel(&env, old);

    let child = env
        .cmd(&["--json", "run", "resume", &old.to_string()])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let new = old + 1;
    eventually("the resumed run", || {
        show(&env, new)["run"]["status"] == "running"
    });
    end(&env, new, "failed");
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let last: Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).lines().last().unwrap()).unwrap();
    assert_eq!(last["run_id"], new);
    assert_eq!(last["status"], "failed");
}

#[test]
fn runs_show_links_the_chain_both_ways() {
    let env = Env::new();
    env.start_daemon();
    write_wf(&env, "build", WF);
    let old = start(&env, &["build"]);
    cancel(&env, old);
    let new = resume(&env, old).1["id"].as_i64().unwrap();

    assert_eq!(show(&env, old)["run"]["resumed_as"], new);
    assert!(show(&env, old)["run"]["resumed_from"].is_null());
    assert_eq!(show(&env, new)["run"]["resumed_from"], old);
    assert!(show(&env, new)["run"]["resumed_as"].is_null());

    let human = |id: i64| {
        String::from_utf8_lossy(&env.run(&["runs", "show", &id.to_string()]).stdout).into_owned()
    };
    assert!(
        human(old).contains(&format!("resumed as run {new}")),
        "{}",
        human(old)
    );
    assert!(
        human(new).contains(&format!("resumes    run {old}")),
        "{}",
        human(new)
    );
}
