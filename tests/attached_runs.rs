mod common;

use common::Env;
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::{Child, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

const WF: &str = "---\nname: build\n---\n## Build\nBuild it.\n";

fn write_wf(env: &Env, name: &str, src: &str) {
    let dir = env.project().join(".tome/workflows");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{name}.md")), src).unwrap();
}

/// An attached `tome run` in the background, with its stdout lines.
struct Attached {
    child: Child,
    lines: Receiver<String>,
}

impl Attached {
    fn spawn(env: &Env, args: &[&str]) -> Attached {
        let mut child = env
            .cmd(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    return;
                }
            }
        });
        Attached { child, lines }
    }

    fn next_line(&self) -> String {
        self.lines
            .recv_timeout(Duration::from_secs(10))
            .expect("no output from attached run")
    }

    fn next_event(&self) -> Value {
        let line = self.next_line();
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("not JSON ({e}): {line}"))
    }

    /// Exit code, plus any stdout lines not read yet.
    fn wait(mut self) -> (i32, Vec<String>) {
        let status = wait_timeout(&mut self.child, Duration::from_secs(10));
        (status, self.lines.iter().collect())
    }

    fn signal(&self, sig: i32) {
        unsafe { libc::kill(self.child.id() as i32, sig) };
    }
}

fn wait_timeout(child: &mut Child, limit: Duration) -> i32 {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status.code().unwrap_or(-1);
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("attached run didn't exit");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn run_status(env: &Env, id: i64) -> Value {
    env.rpc_ok("run.get", json!({ "id": id }))
}

#[test]
fn attached_run_streams_ndjson_and_exits_0() {
    let env = Env::new();
    env.start_daemon();
    write_wf(&env, "build", WF);
    let run = Attached::spawn(&env, &["--json", "run", "build"]);

    let first = run.next_event();
    assert_eq!(first["type"], "run");
    assert_eq!(first["status"], "running");
    assert_eq!(first["workflow"], "build");
    let id = first["run_id"].as_i64().unwrap();
    let rid = id.to_string();

    assert_eq!(env.json(&["step", "start", "Build", "--run", &rid]).0, 0);
    let ev = run.next_event();
    assert_eq!(
        (
            ev["type"].as_str(),
            ev["step"].as_str(),
            ev["event"].as_str()
        ),
        (Some("step"), Some("Build"), Some("start"))
    );

    assert_eq!(
        env.json(&["step", "done", "-m", "compiled", "--run", &rid])
            .0,
        0
    );
    let ev = run.next_event();
    assert_eq!(ev["event"], "done");
    assert_eq!(ev["message"], "compiled");

    assert_eq!(
        env.json(&[
            "run",
            "finish",
            "--status",
            "succeeded",
            "--summary",
            "shipped",
            "--run",
            &rid
        ])
        .0,
        0
    );
    let ev = run.next_event();
    assert_eq!(ev["type"], "run");
    assert_eq!(ev["status"], "succeeded");
    assert_eq!(ev["summary"], "shipped");

    let (code, rest) = run.wait();
    assert_eq!(code, 0);
    assert!(rest.is_empty(), "{rest:?}");
}

#[test]
fn failed_run_exits_1_with_human_lines() {
    let env = Env::new();
    env.start_daemon();
    write_wf(&env, "build", WF);
    let run = Attached::spawn(&env, &["run", "build"]);

    let first = run.next_line();
    assert!(first.ends_with("run 1 running (build)"), "{first}");
    assert!(first.starts_with('['), "{first}");
    env.json(&["step", "start", "Build", "--run", "1"]);
    assert!(run.next_line().ends_with("] Build: started"));
    env.json(&["step", "fail", "-m", "linker error", "--run", "1"]);
    assert!(run.next_line().ends_with("] Build: failed (linker error)"));
    env.json(&[
        "run",
        "finish",
        "--status",
        "failed",
        "--summary",
        "gave up",
        "--run",
        "1",
    ]);
    assert!(run.next_line().ends_with("] run 1 failed: gave up"));
    assert_eq!(run.wait().0, 1);
}

#[test]
fn ctrl_c_cancels_the_run_and_exits_130() {
    let env = Env::new();
    env.start_daemon();
    write_wf(&env, "build", WF);
    let run = Attached::spawn(&env, &["--json", "run", "build"]);
    let id = run.next_event()["run_id"].as_i64().unwrap();
    env.json(&["step", "start", "Build", "--run", &id.to_string()]);
    run.next_event();

    run.signal(libc::SIGINT);
    // The running step fails with the run, then the run ends cancelled.
    let ev = run.next_event();
    assert_eq!(
        (ev["step"].as_str(), ev["event"].as_str()),
        (Some("Build"), Some("fail"))
    );
    let ev = run.next_event();
    assert_eq!(ev["status"], "cancelled");
    assert_eq!(ev["reason"], "interrupted");
    assert_eq!(run.wait().0, 130);
    assert_eq!(run_status(&env, id)["status"], "cancelled");
}

#[test]
fn caller_dying_cancels_the_run() {
    let env = Env::new();
    env.start_daemon();
    write_wf(&env, "build", WF);
    let run = Attached::spawn(&env, &["--json", "run", "build"]);
    let id = run.next_event()["run_id"].as_i64().unwrap();

    run.signal(libc::SIGKILL);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let got = run_status(&env, id);
        if got["status"] == "cancelled" {
            assert_eq!(got["reason"], "caller_exited");
            break;
        }
        assert!(Instant::now() < deadline, "run not cancelled: {got}");
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = run.wait();
}

#[test]
fn cancel_from_elsewhere_ends_the_attached_run() {
    let env = Env::new();
    env.start_daemon();
    write_wf(&env, "build", WF);
    let run = Attached::spawn(&env, &["run", "build"]);
    run.next_line();
    assert_eq!(env.json(&["run", "cancel", "1"]).0, 0);
    assert!(run.next_line().ends_with("run 1 cancelled: user_cancelled"));
    assert_eq!(run.wait().0, 130);
}

#[test]
fn invalid_workflow_exits_2_without_streaming() {
    let env = Env::new();
    env.start_daemon();
    write_wf(&env, "bad", "---\nname: bad\n---\n{{params.nope}}\n");
    let run = Attached::spawn(&env, &["--json", "run", "bad"]);
    let err = run.next_event();
    assert_eq!(err["error"]["kind"], "invalid_workflow");
    assert_eq!(run.wait().0, 2);
    let (_, runs) = env.json(&["runs", "list"]);
    assert_eq!(runs["runs"].as_array().map(Vec::len), Some(0), "{runs}");
}

#[test]
fn watch_replays_history_of_a_finished_run() {
    let env = Env::new();
    env.start_daemon();
    write_wf(&env, "build", WF);
    let (_, run) = env.json(&["run", "build", "--detach"]);
    let rid = run["id"].to_string();
    env.json(&["step", "start", "Build", "--run", &rid]);
    env.json(&["step", "done", "--run", &rid]);
    env.json(&["run", "finish", "--status", "succeeded", "--run", &rid]);

    let mut stream = UnixStream::connect(env.socket()).unwrap();
    writeln!(
        stream,
        "{}",
        json!({ "jsonrpc": "2.0", "id": 1, "method": "run.watch", "params": { "id": run["id"] } })
    )
    .unwrap();
    let lines: Vec<Value> = BufReader::new(stream)
        .lines()
        .take(4)
        .map(|l| serde_json::from_str(&l.unwrap()).unwrap())
        .collect();
    let events: Vec<(&str, &str)> = lines[..3]
        .iter()
        .map(|l| {
            (
                l["method"].as_str().unwrap(),
                l["params"]["event"]
                    .as_str()
                    .or(l["params"]["status"].as_str())
                    .unwrap(),
            )
        })
        .collect();
    assert_eq!(
        events,
        [
            ("run.event", "start"),
            ("run.event", "done"),
            ("run.event", "succeeded")
        ]
    );
    assert_eq!(lines[3]["id"], 1);
    assert_eq!(lines[3]["result"]["status"], "succeeded");
}
