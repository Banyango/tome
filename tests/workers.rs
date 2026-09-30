mod common;

use common::{eventually, Env};
use serde_json::Value;
use std::fs;
use std::process::{Command, Stdio};

/// A project with a `build` workflow and harnesses from `scripts`
/// (`name: [sh, <script>, {{prompt_file}}]`), plus the idle `claude`.
fn setup(scripts: &[(&str, &str)], orchestrator: &str) -> Env {
    let env = Env::new();
    let mut config = common::IDLE_CONFIG.to_string();
    for (name, script) in scripts {
        let path = env.home().join(format!("{name}.sh"));
        fs::write(&path, script).unwrap();
        config.push_str(&format!(
            "  {name}: [sh, \"{}\", \"{{{{prompt_file}}}}\"]\n",
            path.display()
        ));
    }
    env.set_config(&config);
    let dir = env.project().join(".tome/workflows");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("build.md"),
        format!("---\nname: build\ndefaults:\n  orchestrator_harness: {orchestrator}\n---\n## Build\nBuild it.\n"),
    )
    .unwrap();
    env.start_daemon();
    env
}

fn start(env: &Env) -> String {
    let (code, run) = env.json(&["run", "build", "--detach"]);
    assert_eq!(code, 0, "{run}");
    run["id"].to_string()
}

/// A tome command as the run's orchestrator (`TOME_RUN_ID` set), `--json`.
fn tome(env: &Env, run: &str, args: &[&str]) -> (i32, Value) {
    let mut full = vec!["--json"];
    full.extend_from_slice(args);
    let out = env
        .cmd(&full)
        .env("TOME_RUN_ID", run)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "not JSON ({e}): {stdout}\nstderr: {}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (out.status.code().unwrap(), value)
}

fn ok(env: &Env, run: &str, args: &[&str]) -> Value {
    let (code, v) = tome(env, run, args);
    assert_eq!(code, 0, "tome {args:?}: {v}");
    v
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn git_repo(env: &Env) {
    let p = env.project();
    git(&p, &["init", "-q", "-b", "main"]);
    fs::write(p.join("README"), "hi\n").unwrap();
    git(&p, &["add", "."]);
    git(
        &p,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "-m",
            "init",
        ],
    );
}

#[test]
fn command_workers_report_by_exit_code() {
    let env = setup(&[], "claude");
    let run = start(&env);
    let spawned = ok(
        &env,
        &run,
        &[
            "worker",
            "spawn",
            "--name",
            "ok",
            "--",
            "sh",
            "-c",
            "echo hello from ok",
        ],
    );
    assert_eq!(spawned["name"], "ok");
    assert_eq!(spawned["kind"], "command");
    ok(
        &env,
        &run,
        &[
            "worker",
            "spawn",
            "--name",
            "bad",
            "--",
            "sh",
            "-c",
            "echo boom; exit 7",
        ],
    );

    let w = ok(&env, &run, &["worker", "wait", "ok"]);
    assert_eq!(w["status"], "done", "{w}");
    let summary = w["summary"].as_str().unwrap();
    assert!(summary.starts_with("exit code 0"), "{summary}");
    assert!(summary.contains("hello from ok"), "{summary}");

    let w = ok(&env, &run, &["worker", "wait", "bad"]);
    assert_eq!(
        (
            w["status"].as_str(),
            w["reason"].as_str(),
            w["exit_code"].as_i64()
        ),
        (Some("failed"), Some("exit_code"), Some(7))
    );
    assert!(w["summary"].as_str().unwrap().contains("boom"), "{w}");
    eventually("worker sessions closed", || {
        !env.has_session(&format!("tome-{run}-build-ok"))
    });

    // Names are unique; a bad request spawns nothing.
    let (code, err) = tome(
        &env,
        &run,
        &["worker", "spawn", "--name", "ok", "--", "true"],
    );
    assert_eq!(code, 2, "{err}");
    let (code, _) = tome(&env, &run, &["worker", "spawn", "--name", "x"]);
    assert_eq!(code, 2);
    let (code, _) = tome(
        &env,
        &run,
        &["worker", "spawn", "--name", "orchestrator", "--", "true"],
    );
    assert_eq!(code, 2);
    // Generated names.
    assert_eq!(
        ok(&env, &run, &["worker", "spawn", "--", "true"])["name"],
        "w3",
        "the third worker"
    );
    let all = ok(&env, &run, &["worker", "status"]);
    assert_eq!(all["workers"].as_array().unwrap().len(), 3);
}

#[test]
fn keep_open_leaves_the_session() {
    let env = setup(&[], "claude");
    let run = start(&env);
    ok(
        &env,
        &run,
        &[
            "worker",
            "spawn",
            "--name",
            "k",
            "--keep-open",
            "--",
            "true",
        ],
    );
    assert_eq!(ok(&env, &run, &["worker", "wait", "k"])["status"], "done");
    std::thread::sleep(std::time::Duration::from_millis(700));
    assert!(env.has_session(&format!("tome-{run}-build-k")));
}

const AGENT: &str = r#"
grep -q "You are a worker in a tome workflow run" "$1" || exit 3
grep -q "Paint the fence" "$1" || exit 4
echo "id=$TOME_WORKER_ID run=$TOME_RUN_ID output=$TOME_OUTPUT" > "$TOME_HOME/agent-env.txt"
tome worker spawn -- true >/dev/null 2>&1; echo "nested=$?" >> "$TOME_HOME/agent-env.txt"
tome worker done --summary "fence painted" >/dev/null
sleep 30
"#;

#[test]
fn agent_workers_report_and_are_closed() {
    let env = setup(&[("painter", AGENT), ("quitter", "exit 0\n")], "claude");
    let run = start(&env);
    let w = ok(
        &env,
        &run,
        &[
            "worker",
            "spawn",
            "--name",
            "p",
            "--harness",
            "painter",
            "--prompt",
            "Paint the fence.",
        ],
    );
    assert_eq!(
        (w["kind"].as_str(), w["harness"].as_str()),
        (Some("agent"), Some("painter"))
    );
    let w = ok(&env, &run, &["worker", "wait", "p"]);
    assert_eq!(
        (w["status"].as_str(), w["summary"].as_str()),
        (Some("done"), Some("fence painted")),
        "{w}"
    );
    let seen = fs::read_to_string(env.home().join("agent-env.txt")).unwrap();
    assert_eq!(seen, format!("id=p run={run} output=json\nnested=2\n"));
    // Closed after it reported, though it would have slept on.
    eventually("reported worker's session closed", || {
        !env.has_session(&format!("tome-{run}-build-p"))
    });
    // First result stands.
    let out = env
        .cmd(&["worker", "fail"])
        .env("TOME_RUN_ID", &run)
        .env("TOME_WORKER_ID", "p")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));

    ok(
        &env,
        &run,
        &[
            "worker",
            "spawn",
            "--name",
            "q",
            "--harness",
            "quitter",
            "--prompt",
            "Do something.",
        ],
    );
    let w = ok(&env, &run, &["worker", "wait", "q"]);
    assert_eq!(
        (w["status"].as_str(), w["reason"].as_str()),
        (Some("failed"), Some("worker_exited")),
        "{w}"
    );
}

#[test]
fn groups_finish_and_fail_fast_cancels_the_rest() {
    let env = setup(&[], "claude");
    let run = start(&env);
    ok(
        &env,
        &run,
        &[
            "worker", "spawn", "--group", "g", "--name", "a", "--", "true",
        ],
    );
    ok(
        &env,
        &run,
        &[
            "worker", "spawn", "--group", "g", "--name", "b", "--", "sh", "-c", "exit 1",
        ],
    );
    let g = ok(&env, &run, &["group", "wait", "g"]);
    assert_eq!(g["group"]["status"], "finished", "{g}");
    let statuses: Vec<&str> = g["workers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["status"].as_str().unwrap())
        .collect();
    assert_eq!(statuses, ["done", "failed"]);
    // Waiting closed it.
    let (code, err) = tome(
        &env,
        &run,
        &["worker", "spawn", "--group", "g", "--", "true"],
    );
    assert_eq!(code, 2, "{err}");

    ok(&env, &run, &["group", "create", "ff", "--fail-fast"]);
    let (code, _) = tome(&env, &run, &["group", "create", "ff"]);
    assert_eq!(code, 2);
    let (code, _) = tome(&env, &run, &["group", "wait", "ff"]);
    assert_eq!(code, 2, "waiting on an empty group");
    ok(
        &env,
        &run,
        &[
            "worker", "spawn", "--group", "ff", "--name", "slow", "--", "sleep", "30",
        ],
    );
    ok(
        &env,
        &run,
        &[
            "worker",
            "spawn",
            "--group",
            "ff",
            "--name",
            "boom",
            "--",
            "sh",
            "-c",
            "sleep 0.3; exit 1",
        ],
    );
    let g = ok(&env, &run, &["group", "wait", "ff"]);
    assert_eq!(g["group"]["status"], "finished");
    let slow = ok(&env, &run, &["worker", "status", "slow"]);
    assert_eq!(
        (slow["status"].as_str(), slow["reason"].as_str()),
        (Some("cancelled"), Some("fail_fast"))
    );
    eventually("cut-off worker killed", || {
        !env.has_session(&format!("tome-{run}-build-slow"))
    });
}

#[test]
fn worktrees_branch_from_head_and_are_ignored() {
    let env = setup(&[], "claude");
    git_repo(&env);
    fs::write(env.project().join("dirty.txt"), "uncommitted").unwrap();
    let run = start(&env);
    let w = ok(
        &env,
        &run,
        &[
            "worker",
            "spawn",
            "--name",
            "wt",
            "--worktree",
            "--",
            "sh",
            "-c",
            "echo \"$TOME_WORKTREE\" > seen.txt; pwd >> seen.txt",
        ],
    );
    let path = env
        .project()
        .canonicalize()
        .unwrap()
        .join(format!(".tome/worktrees/{run}-wt"));
    assert_eq!(w["worktree"].as_str(), Some(path.to_str().unwrap()), "{w}");
    assert_eq!(
        w["branch"].as_str(),
        Some(format!("tome/{run}/wt").as_str())
    );
    assert_eq!(w["base"], "main");
    ok(&env, &run, &["worker", "wait", "wt"]);
    let seen = fs::read_to_string(path.join("seen.txt")).unwrap();
    let lines: Vec<&str> = seen.lines().collect();
    assert_eq!(lines[0], path.to_str().unwrap());
    assert_eq!(fs::canonicalize(lines[1]).unwrap(), path);
    assert!(!path.join("dirty.txt").exists());
    assert!(fs::read_to_string(env.project().join(".gitignore"))
        .unwrap()
        .contains(".tome/worktrees/"));

    let free = ok(
        &env,
        &run,
        &["worktree", "create", "scratch", "--base", "main"],
    );
    assert!(std::path::Path::new(free["path"].as_str().unwrap()).is_dir());
    let (code, _) = tome(
        &env,
        &run,
        &["worktree", "create", "nope", "--base", "no-such-ref"],
    );
    assert_eq!(code, 2);
    let (code, _) = tome(
        &env,
        &run,
        &[
            "worker",
            "spawn",
            "--worktree",
            "--base",
            "no-such-ref",
            "--",
            "true",
        ],
    );
    assert_eq!(code, 2);
    let (code, _) = tome(
        &env,
        &run,
        &["worker", "spawn", "--base", "main", "--", "true"],
    );
    assert_eq!(code, 2, "--base needs --worktree");

    let shown = env.json(&["runs", "show", &run]).1;
    let worktrees = shown["worktrees"].as_array().unwrap();
    assert_eq!(worktrees.len(), 2, "{shown}");
    assert_eq!(worktrees[0]["worker"], "wt");
}

#[test]
fn worktrees_need_a_repo() {
    let env = setup(&[], "claude");
    let run = start(&env);
    let (code, err) = tome(&env, &run, &["worker", "spawn", "--worktree", "--", "true"]);
    assert_eq!(code, 2, "{err}");
    assert!(ok(&env, &run, &["worker", "status"])["workers"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn queues_claim_ack_and_release() {
    let env = setup(&[], "claude");
    let run = start(&env);
    let (code, empty) = tome(&env, &run, &["queue", "pull", "jobs"]);
    assert_eq!((code, empty["status"].as_str()), (3, Some("empty")));

    let m1 = ok(&env, &run, &["queue", "push", "jobs", "one"]);
    assert_eq!(m1["sender"], "orchestrator");
    let mut push = env
        .cmd(&["--json", "queue", "push", "jobs", "-"])
        .env("TOME_RUN_ID", &run)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    push.stdin.take().unwrap().write_all(b"two\nlines").unwrap();
    assert!(push.wait().unwrap().success());

    // A worker claims one and ends without acking it: it goes back.
    ok(
        &env,
        &run,
        &[
            "worker",
            "spawn",
            "--name",
            "c",
            "--",
            "sh",
            "-c",
            "tome queue pull jobs > \"$TOME_HOME/claimed.json\"",
        ],
    );
    ok(&env, &run, &["worker", "wait", "c"]);
    let claimed: Value =
        serde_json::from_str(&fs::read_to_string(env.home().join("claimed.json")).unwrap())
            .unwrap();
    assert_eq!(claimed["message"]["body"], "one");

    let got = ok(&env, &run, &["queue", "pull", "jobs"]);
    assert_eq!(got["message"]["body"], "one", "released back to the front");
    let got2 = ok(&env, &run, &["queue", "pull", "jobs"]);
    assert_eq!(got2["message"]["body"], "two\nlines");
    ok(
        &env,
        &run,
        &["queue", "ack", &got["message"]["id"].to_string()],
    );
    let ls = ok(&env, &run, &["queue", "ls"]);
    assert_eq!(
        (
            ls["queues"][0]["pending"].as_i64(),
            ls["queues"][0]["claimed"].as_i64()
        ),
        (Some(0), Some(1))
    );

    ok(&env, &run, &["queue", "close", "jobs"]);
    let started = std::time::Instant::now();
    let (code, closed) = tome(&env, &run, &["queue", "pull", "jobs", "--wait"]);
    assert_eq!((code, closed["status"].as_str()), (3, Some("closed")));
    assert!(started.elapsed().as_secs() < 5);
    let started = std::time::Instant::now();
    let (code, _) = tome(&env, &run, &["queue", "pull", "other", "--wait", "1s"]);
    assert_eq!(code, 3);
    assert!(started.elapsed().as_millis() >= 900);
}

#[test]
fn finish_waits_for_workers_and_cancel_ends_them() {
    let env = setup(&[], "claude");
    let run = start(&env);
    ok(
        &env,
        &run,
        &["worker", "spawn", "--name", "s", "--", "sleep", "30"],
    );
    let (code, err) = tome(&env, &run, &["run", "finish", "--status", "succeeded"]);
    assert_eq!(code, 1, "{err}");
    assert!(
        err["error"]["message"].as_str().unwrap().contains("s"),
        "{err}"
    );
    let killed = ok(&env, &run, &["worker", "kill", "s"]);
    assert_eq!(
        (killed["status"].as_str(), killed["reason"].as_str()),
        (Some("cancelled"), Some("killed"))
    );
    assert!(!env.has_session(&format!("tome-{run}-build-s")));
    ok(&env, &run, &["run", "finish", "--status", "succeeded"]);

    let run = start(&env);
    ok(
        &env,
        &run,
        &["worker", "spawn", "--name", "t", "--", "sleep", "30"],
    );
    ok(&env, &run, &["run", "cancel"]);
    let t = ok(&env, &run, &["worker", "status", "t"]);
    assert_eq!(
        (t["status"].as_str(), t["reason"].as_str()),
        (Some("cancelled"), Some("user_cancelled"))
    );
    eventually("worker session killed", || {
        !env.has_session(&format!("tome-{run}-build-t"))
    });
}

/// An orchestrator that records what's typed into its pane.
const LISTENER: &str = r#"
while read line; do echo "$line" >> "$TOME_HOME/typed.txt"; done
"#;

#[test]
fn the_orchestrator_is_nudged_when_workers_and_groups_finish() {
    let env = setup(&[("listener", LISTENER)], "listener");
    let run = start(&env);
    eventually("orchestrator up", || {
        env.has_session(&format!("tome-{run}-build"))
    });
    ok(
        &env,
        &run,
        &[
            "worker", "spawn", "--group", "h", "--name", "a", "--", "sleep", "0.5",
        ],
    );
    ok(&env, &run, &["group", "close", "h"]);
    let typed = env.home().join("typed.txt");
    eventually("nudges typed", || {
        fs::read_to_string(&typed)
            .is_ok_and(|t| t.contains("[tome] group h finished. Details: tome group status h"))
    });
    let t = fs::read_to_string(&typed).unwrap();
    assert!(
        t.contains("[tome] worker a done. Details: tome worker status a"),
        "{t}"
    );
}

const DELEGATING: &str = r#"
tome worker spawn --name w -- sh -c 'exit 0' >/dev/null
tome worker wait w >/dev/null
tome run finish --status succeeded --summary delegated >/dev/null
"#;

#[test]
fn worker_events_are_streamed() {
    let env = setup(&[("delegator", DELEGATING)], "delegator");
    let out = env
        .cmd(&["--json", "run", "build"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let events: Vec<Value> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(out.status.code(), Some(0), "{events:?}");
    let workers: Vec<String> = events
        .iter()
        .filter(|e| e["type"] == "worker")
        .map(|e| {
            format!(
                "{} {}",
                e["worker"].as_str().unwrap(),
                e["event"].as_str().unwrap()
            )
        })
        .collect();
    assert_eq!(workers, ["w spawned", "w started", "w done"]);

    let shown = env.json(&["runs", "show", "1"]).1;
    assert_eq!(shown["workers"][0]["status"], "done", "{shown}");
}
