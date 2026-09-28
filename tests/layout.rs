//! Where a run's sessions go: `layout: tab | split | workspace`.

mod common;

use common::Env;
use serde_json::Value;
use std::fs;
use std::process::Stdio;

fn write_wf(env: &Env, name: &str, defaults: &str) {
    let dir = env.project().join(".tome/workflows");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{name}.md")), format!("---\nname: {name}\n{defaults}---\n## Build\nGo.\n")).unwrap();
}

/// An env with no `TOME_LAYOUT`, so the workflow and config decide.
fn env() -> Env {
    let mut env = Env::new();
    env.vars.retain(|(k, _)| k != "TOME_LAYOUT");
    env
}

fn sessions(env: &Env, run: i64) -> Vec<Value> {
    let (_, shown) = env.json(&["runs", "show", &run.to_string()]);
    shown["sessions"].as_array().unwrap_or_else(|| panic!("{shown}")).clone()
}

/// Spawn a command worker as run `run`'s orchestrator.
fn spawn(env: &Env, run: i64, name: &str, argv: &[&str]) -> Value {
    spawn_with(env, run, name, &[], argv)
}

fn spawn_with(env: &Env, run: i64, name: &str, flags: &[&str], argv: &[&str]) -> Value {
    let mut args = vec!["--json", "worker", "spawn", "--name", name];
    args.extend_from_slice(flags);
    args.push("--");
    args.extend_from_slice(argv);
    let out = env.cmd(&args).env("TOME_RUN_ID", run.to_string()).stdin(Stdio::null()).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_str(stdout.trim()).unwrap()
}

#[test]
fn unknown_layout_in_a_workflow_fails_validation() {
    let env = env();
    write_wf(&env, "bad", "defaults:\n  layout: tabs\n");
    let (code, v) = env.json(&["validate", "bad"]);
    assert_eq!(code, 2, "{v}");
    let err = v.to_string();
    assert!(err.contains("unknown `defaults.layout` `tabs`"), "{err}");
    assert!(err.contains("tab, split, workspace"), "{err}");
}

#[test]
fn unknown_layout_in_the_config_is_refused_with_a_hint() {
    let env = env();
    env.set_config(&format!("{}layout: sideways\n", common::IDLE_CONFIG));
    write_wf(&env, "build", "");
    env.start_daemon();
    let (code, err) = env.json(&["run", "build", "--detach"]);
    assert_eq!(code, 2, "{err}");
    let message = err["error"]["message"].as_str().unwrap();
    assert!(message.contains("unknown session layout `sideways` (from `layout` in the tome config)"), "{err}");
    assert!(err["error"]["hint"].as_str().unwrap().contains("tab, split, workspace"), "{err}");
    let (_, runs) = env.json(&["runs", "list"]);
    assert_eq!(runs["runs"].as_array().unwrap().len(), 0);
}

#[test]
fn the_workflow_layout_overrides_the_config() {
    let env = env();
    // Every level is checked, so the overridden config value must be valid.
    env.set_config(&format!("{}layout: split\n", common::IDLE_CONFIG));
    write_wf(&env, "build", "defaults:\n  layout: workspace\n");
    env.start_daemon();
    let (code, run) = env.json(&["run", "build", "--detach"]);
    assert_eq!(code, 0, "{run}");
    let s = &sessions(&env, 1)[0];
    assert_eq!(s["layout"], "workspace", "{s}");
    assert_eq!(s["placement"]["sources"]["layout"], "`defaults.layout`", "{s}");
    // Under `workspace` a run keeps its own tmux session, as before.
    assert!(env.has_session("tome-1-build"));
}

#[test]
fn the_config_layout_is_recorded_on_every_session() {
    let env = env();
    env.set_config(&format!("{}layout: workspace\n", common::IDLE_CONFIG));
    write_wf(&env, "build", "");
    env.start_daemon();
    env.json(&["run", "build", "--detach"]);
    spawn(&env, 1, "w1", &["sleep", "30"]);
    let all = sessions(&env, 1);
    assert_eq!(all.len(), 2, "{all:?}");
    assert!(all.iter().all(|s| s["layout"] == "workspace"), "{all:?}");
    assert!(env.has_session("tome-1-build-w1"));
}

/// A `reader` harness whose orchestrator writes the first line typed into
/// it to `got.txt` in the tome home, then idles.
fn reader(env: &Env, layout: &str) {
    let script = env.home().join("reader.sh");
    fs::write(&script, format!("read line; echo \"$line\" > {}; sleep 30\n", env.home().join("got.txt").display())).unwrap();
    env.set_config(&format!("{}  reader: [sh, \"{}\"]\nlayout: {layout}\n", common::IDLE_CONFIG, script.display()));
    write_wf(env, "build", "defaults:\n  harness: reader\n");
}

fn got(env: &Env) -> String {
    fs::read_to_string(env.home().join("got.txt")).unwrap_or_default()
}

#[test]
fn sessions_are_tracked_by_their_own_pane() {
    let env = env();
    reader(&env, "workspace");
    env.start_daemon();
    env.json(&["run", "build", "--detach"]);
    let s = &sessions(&env, 1)[0];
    let pane = s["pane"].as_str().unwrap_or_else(|| panic!("no pane: {s}"));
    assert!(pane.starts_with('%'), "{s}");

    // The user opens another window in the session: nudges still reach the
    // orchestrator's own pane.
    assert!(env.tmux(&["new-window", "-t", "=tome-1-build:"]).status.success());
    spawn(&env, 1, "w1", &["true"]);
    common::eventually("the nudge", || got(&env).contains("[tome] worker w1 done"));
}

fn lines(out: std::process::Output) -> Vec<String> {
    String::from_utf8_lossy(&out.stdout).lines().map(str::to_string).collect()
}

/// The project's tome session's windows: `(pane, name)`.
fn windows(env: &Env) -> Vec<(String, String)> {
    let out = env.tmux(&["list-windows", "-t", "=p-orchestrator", "-F", "#{pane_id}\t#{window_name}"]);
    lines(out).iter().filter_map(|l| l.split_once('\t')).map(|(p, n)| (p.to_string(), n.to_string())).collect()
}

/// The panes of the project's tome session's first window: `(pane, title)`.
fn panes(env: &Env) -> Vec<(String, String)> {
    let out = env.tmux(&["list-panes", "-t", "=p-orchestrator:^", "-F", "#{pane_id}\t#{pane_title}"]);
    lines(out).iter().filter_map(|l| l.split_once('\t')).map(|(p, n)| (p.to_string(), n.to_string())).collect()
}

fn named(all: &[(String, String)], name: &str) -> bool {
    all.iter().any(|(_, n)| n == name)
}

/// `tome worker wait <name>` as run `run`'s orchestrator.
fn wait(env: &Env, run: i64, name: &str) -> Value {
    let out = env.cmd(&["--json", "worker", "wait", name]).env("TOME_RUN_ID", run.to_string()).output().unwrap();
    serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap()
}

fn status(env: &Env, run: i64) -> Value {
    env.json(&["runs", "show", &run.to_string()]).1["run"].clone()
}

#[test]
fn tab_is_the_default_and_puts_sessions_in_windows_of_the_project_session() {
    let env = env();
    write_wf(&env, "build", "");
    env.start_daemon();
    env.json(&["run", "build", "--detach"]);
    let orch = &sessions(&env, 1)[0];
    assert_eq!(orch["layout"], "tab", "{orch}");
    assert!(env.has_session("p-orchestrator"));
    assert!(!env.has_session("tome-1-build"), "no session of its own");
    let pane = orch["pane"].as_str().unwrap();
    let attach = orch["attach"].as_str().unwrap();
    assert!(attach.contains(&format!(r"select-window -t {pane} \; select-pane -t {pane} \; attach -t")), "{attach}");
    common::eventually("the orchestrator's window", || named(&windows(&env), "tome: build #1"));

    // Workers, and a second run, share the one session.
    spawn(&env, 1, "w1", &["sleep", "30"]);
    env.json(&["run", "build", "--detach"]);
    let all = windows(&env);
    assert!(named(&all, "tome: build #1 / w1") && named(&all, "tome: build #2"), "{all:?}");
    assert_eq!(all.len(), 4, "the shell and three sessions: {all:?}");

    // A finished worker's window closes, unless it's kept open.
    spawn(&env, 1, "w2", &["true"]);
    spawn_with(&env, 1, "w3", &["--keep-open"], &["true"]);
    assert_eq!(wait(&env, 1, "w2")["status"], "done");
    assert_eq!(wait(&env, 1, "w3")["status"], "done");
    common::eventually("w2's window closed", || !named(&windows(&env), "tome: build #1 / w2"));
    assert!(named(&windows(&env), "tome: build #1 / w3"), "kept open");

    // Closing one window ends just that session.
    let (w1, _) = windows(&env).into_iter().find(|(_, n)| n == "tome: build #1 / w1").unwrap();
    assert!(env.tmux(&["kill-window", "-t", &w1]).status.success());
    let w = wait(&env, 1, "w1");
    assert_eq!((w["status"].as_str(), w["reason"].as_str()), (Some("failed"), Some("worker_exited")), "{w}");
    let (orch2, _) = windows(&env).into_iter().find(|(_, n)| n == "tome: build #2").unwrap();
    assert!(env.tmux(&["kill-window", "-t", &orch2]).status.success());
    common::eventually("run 2 failed", || status(&env, 2)["reason"] == "orchestrator_exited");
    assert_eq!(status(&env, 1)["status"], "running");

    // Cancelling the run closes its windows; the tome session stays.
    assert_eq!(env.json(&["run", "cancel", "1"]).0, 0);
    common::eventually("run 1's windows closed", || windows(&env).iter().all(|(_, n)| !n.starts_with("tome: build #1")));
    assert_eq!(windows(&env).len(), 1, "{:?}", windows(&env));
}

#[test]
fn split_puts_each_session_in_its_own_pane() {
    let env = env();
    env.set_config(&format!("{}layout: split\n", common::IDLE_CONFIG));
    write_wf(&env, "build", "");
    env.start_daemon();
    env.json(&["run", "build", "--detach"]);
    assert_eq!(sessions(&env, 1)[0]["layout"], "split");
    spawn(&env, 1, "w1", &["sleep", "30"]);
    spawn(&env, 1, "w2", &["sleep", "30"]);
    let all = panes(&env);
    assert_eq!(all.len(), 4, "the shell and three sessions, side by side: {all:?}");
    assert!(named(&all, "tome: build #1") && named(&all, "tome: build #1 / w2"), "{all:?}");
    assert_eq!(windows(&env).len(), 1);
    // The newest is on the right.
    let right = lines(env.tmux(&["list-panes", "-t", "=p-orchestrator:^", "-F", "#{pane_right} #{pane_title}"]));
    let rightmost = right.iter().max_by_key(|l| l.split(' ').next().unwrap().parse::<i64>().unwrap()).unwrap();
    assert!(rightmost.ends_with("tome: build #1 / w2"), "{right:?}");

    // Closing one pane ends just that session.
    let (w1, _) = all.iter().find(|(_, n)| n == "tome: build #1 / w1").unwrap();
    assert!(env.tmux(&["kill-pane", "-t", w1]).status.success());
    let w = wait(&env, 1, "w1");
    assert_eq!((w["status"].as_str(), w["reason"].as_str()), (Some("failed"), Some("worker_exited")), "{w}");

    assert_eq!(env.json(&["run", "cancel", "1"]).0, 0);
    common::eventually("only the shell is left", || panes(&env).len() == 1);
}

#[test]
fn a_restart_kills_the_sessions_but_leaves_the_tome_session() {
    let env = env();
    write_wf(&env, "build", "");
    env.start_daemon();
    env.json(&["run", "build", "--detach"]);
    spawn(&env, 1, "w1", &["sleep", "30"]);
    assert_eq!(windows(&env).len(), 3);

    let pid = env.rpc_ok("daemon.status", serde_json::json!({}))["pid"].as_i64().unwrap();
    unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    common::eventually("the daemon died", || unsafe { libc::kill(pid as i32, 0) } != 0);
    env.start_daemon();
    assert_eq!(status(&env, 1)["reason"], "daemon_restart");
    common::eventually("the sessions are gone", || windows(&env).len() == 1);
    assert!(env.has_session("p-orchestrator"));

    // The next run finds it again.
    env.json(&["run", "build", "--detach"]);
    common::eventually("run 2's window", || named(&windows(&env), "tome: build #2"));
    let out = env.tmux(&["list-sessions", "-F", "#{session_name}"]);
    assert_eq!(lines(out), ["p-orchestrator"]);
}

#[test]
fn layout_presets_lists_built_in_global_and_project_presets() {
    let env = env();
    env.set_config(&format!("{}layout_presets:\n  side:\n    layout: split\n    split:\n      size: 30%\n  wide:\n    layout: tab\n", common::IDLE_CONFIG));
    let dir = env.project().join(".tome");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("config.yaml"), "layout_presets:\n  wide:\n    layout: split\n    split:\n      direction: down\n").unwrap();
    let (code, out) = env.json(&["layout", "presets"]);
    assert_eq!(code, 0, "{out}");
    let presets = out["presets"].as_array().unwrap();
    let find = |name: &str, scope: &str| presets.iter().find(|p| p["name"] == name && p["scope"] == scope).cloned();
    assert!(find("workspace", "built-in").is_some(), "{out}");
    assert_eq!(find("side", "global").unwrap()["settings"]["size"], "30%", "{out}");
    assert_eq!(find("wide", "global").unwrap()["overridden"], true, "{out}");
    assert_eq!(find("wide", "project").unwrap()["settings"]["direction"], "down", "{out}");
}

#[test]
fn a_preset_sets_the_placement_and_names_itself_as_the_source() {
    let env = env();
    env.set_config(&format!("{}layout_presets:\n  side:\n    layout: workspace\n", common::IDLE_CONFIG));
    write_wf(&env, "build", "defaults:\n  layout:\n    preset: side\n");
    env.start_daemon();
    let (code, run) = env.json(&["run", "build", "--detach"]);
    assert_eq!(code, 0, "{run}");
    let s = &sessions(&env, 1)[0];
    assert_eq!(s["layout"], "workspace", "{s}");
    let source = s["placement"]["sources"]["layout"].as_str().unwrap();
    assert!(source.starts_with("preset `side`"), "{s}");
}

#[test]
fn an_undefined_preset_fails_the_run_and_warns_in_validate() {
    let env = env();
    env.set_config(common::IDLE_CONFIG);
    write_wf(&env, "build", "defaults:\n  layout:\n    preset: nope\n");
    let (code, out) = env.json(&["validate", "build"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.to_string().contains("layout preset `nope` isn't defined"), "{out}");
    env.start_daemon();
    let (code, err) = env.json(&["run", "build", "--detach"]);
    assert_eq!(code, 2, "{err}");
    assert!(err["error"]["message"].as_str().unwrap().contains("unknown layout preset `nope`"), "{err}");
    assert!(err["error"]["hint"].as_str().unwrap().contains("workspace"), "{err}");
}

#[test]
fn run_flags_place_the_orchestrator_and_sit_below_the_workers_blocks() {
    let env = env();
    env.set_config(&format!("{}layout: tab\n", common::IDLE_CONFIG));
    write_wf(&env, "build", "defaults:\n  layout:\n    workers:\n      - match: review-*\n        layout: tab\n");
    env.start_daemon();
    let (code, run) = env.json(&["run", "build", "--detach", "--layout", "split", "--size", "30%"]);
    assert_eq!(code, 0, "{run}");
    let o = &sessions(&env, 1)[0];
    assert_eq!(o["layout"], "split", "{o}");
    assert_eq!(o["placement"]["sources"]["layout"], "`tome run` flags", "{o}");
    assert_eq!(o["placement"]["size"], "30%", "{o}");
    spawn(&env, 1, "review-1", &["sleep", "600"]);
    spawn(&env, 1, "build-1", &["sleep", "600"]);
    let all = sessions(&env, 1);
    let find = |n: &str| all.iter().find(|s| s["name"].as_str().unwrap().ends_with(&format!("-{n}"))).cloned().unwrap_or_else(|| panic!("{all:?}"));
    // The rule beats the run's flags; with no rule, they apply.
    assert_eq!(find("review-1")["layout"], "tab");
    assert_eq!(find("build-1")["layout"], "split");
    assert_eq!(find("build-1")["placement"]["sources"]["layout"], "`tome run` flags");
}

#[test]
fn spawn_flags_are_the_top_level_for_a_worker() {
    let env = env();
    env.set_config(common::IDLE_CONFIG);
    write_wf(&env, "build", "defaults:\n  layout:\n    workers:\n      layout: split\n");
    env.start_daemon();
    env.json(&["run", "build", "--detach"]);
    spawn_with(&env, 1, "w", &["--layout", "tab", "--direction", "down"], &["sleep", "600"]);
    let w = sessions(&env, 1).into_iter().find(|s| s["name"] == "tome-1-build-w").unwrap();
    assert_eq!(w["layout"], "tab", "{w}");
    assert_eq!(w["placement"]["direction"], "down", "{w}");
    assert_eq!(w["placement"]["sources"]["layout"], "`tome worker spawn` flags", "{w}");
}

#[test]
fn a_reserved_word_flag_is_the_keyword_and_bad_flags_are_refused() {
    let env = env();
    env.set_config(common::IDLE_CONFIG);
    write_wf(&env, "build", "");
    env.start_daemon();
    let (code, run) = env.json(&["run", "build", "--detach", "--workspace", "own"]);
    assert_eq!(code, 0, "{run}");
    let o = &sessions(&env, 1)[0];
    assert_eq!((o["layout"].as_str(), o["placement"]["workspace"].as_str()), (Some("workspace"), Some("own")), "{o}");

    for (flag, value, message) in [
        ("--layout", "sideways", "--layout: unknown `layout` `sideways`"),
        ("--size", "0", "--size:"),
        ("--workspace", "a b", "--workspace: unknown workspace `a b`"),
        ("--preset", "nope", "unknown layout preset `nope` (from `tome run` flags)"),
    ] {
        let (code, err) = env.json(&["run", "build", "--detach", flag, value]);
        assert_eq!(code, 2, "{flag} {value}: {err}");
        assert!(err["error"]["message"].as_str().unwrap().contains(message), "{err}");
    }
    let (_, runs) = env.json(&["runs", "list"]);
    assert_eq!(runs["runs"].as_array().unwrap().len(), 1);
}

#[test]
fn spawning_with_an_undefined_preset_fails_with_the_known_ones() {
    let env = env();
    env.set_config(common::IDLE_CONFIG);
    write_wf(&env, "build", "");
    env.start_daemon();
    env.json(&["run", "build", "--detach"]);
    let out = env
        .cmd(&["--json", "worker", "spawn", "--name", "w", "--preset", "nope", "--", "sleep", "600"])
        .env("TOME_RUN_ID", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(err["error"]["message"].as_str().unwrap().contains("unknown layout preset `nope` (from `tome worker spawn` flags)"), "{err}");
    assert!(err["error"]["hint"].as_str().unwrap().contains("known presets: split, tab, workspace"), "{err}");
    let (_, workers) = env.json(&["worker", "status", "--run", "1"]);
    assert_eq!(workers["workers"].as_array().map(Vec::len), Some(0), "{workers}");
}

/// `(pane id, left, top, width, height)` of each pane of the project session's first window.
fn geometry(env: &Env) -> Vec<(String, i64, i64, i64, i64)> {
    lines(env.tmux(&["list-panes", "-t", "=p-orchestrator:^", "-F", "#{pane_id} #{pane_left} #{pane_top} #{pane_width} #{pane_height}"]))
        .iter()
        .map(|l| {
            let f: Vec<&str> = l.split(' ').collect();
            let n = |i: usize| f[i].parse::<i64>().unwrap();
            (f[0].to_string(), n(1), n(2), n(3), n(4))
        })
        .collect()
}

fn pane_of(env: &Env, name: &str) -> String {
    let s = sessions(env, 1).into_iter().find(|s| s["name"] == name).unwrap_or_else(|| panic!("no session {name}"));
    s["pane"].as_str().unwrap().to_string()
}

#[test]
fn a_split_follows_its_direction_and_size() {
    let env = env();
    env.set_config(&format!("{}layout: split\n", common::IDLE_CONFIG));
    write_wf(&env, "build", "defaults:\n  layout:\n    workers:\n      split:\n        direction: down\n        size: 12\n");
    env.start_daemon();
    env.json(&["run", "build", "--detach"]);
    spawn(&env, 1, "w", &["sleep", "600"]);
    let all = geometry(&env);
    let orch = pane_of(&env, "tome-1-build");
    let w = pane_of(&env, "tome-1-build-w");
    let g = |id: &str| all.iter().find(|p| p.0 == id).cloned().unwrap_or_else(|| panic!("{id} in {all:?}"));
    // Along the whole bottom edge, 12 rows high.
    assert_eq!(g(&w).4, 12, "{all:?}");
    assert!(g(&w).2 > g(&orch).2, "below: {all:?}");
    assert_eq!(g(&w).3, 200, "full width: {all:?}");
}

#[test]
fn from_opens_off_the_anchor_pane_and_a_too_big_size_is_clamped() {
    let env = env();
    env.set_config(&format!("{}layout: split\n", common::IDLE_CONFIG));
    write_wf(&env, "build", "");
    env.start_daemon();
    env.json(&["run", "build", "--detach", "--size", "30%"]);
    spawn(&env, 1, "a", &["sleep", "600"]);
    spawn_with(&env, 1, "b", &["--from", "orchestrator", "--direction", "down", "--size", "95%"], &["sleep", "600"]);
    let all = geometry(&env);
    let g = |id: &str| all.iter().find(|p| p.0 == id).cloned().unwrap_or_else(|| panic!("{id} in {all:?}"));
    let (orch, b) = (g(&pane_of(&env, "tome-1-build")), g(&pane_of(&env, "tome-1-build-b")));
    // b shares the orchestrator's column, under it.
    assert_eq!((b.1, b.3), (orch.1, orch.3), "{all:?}");
    assert!(b.2 > orch.2, "{all:?}");
    let s = sessions(&env, 1).into_iter().find(|s| s["name"] == "tome-1-build-b").unwrap();
    let warnings = s["placement"]["warnings"].to_string();
    assert!(warnings.contains("clamped to 90%"), "{s}");
    assert_eq!(s["placement"]["from"], "orchestrator", "{s}");
}

#[test]
fn from_falls_back_to_the_last_pane_when_its_anchor_is_gone() {
    let env = env();
    env.set_config(&format!("{}layout: split\n", common::IDLE_CONFIG));
    write_wf(&env, "build", "");
    env.start_daemon();
    env.json(&["run", "build", "--detach", "--layout", "tab"]);
    // The orchestrator is in a window of its own; `from: orchestrator`
    // opens next to it there.
    spawn_with(&env, 1, "a", &["--layout", "split", "--from", "orchestrator"], &["sleep", "600"]);
    let orch = pane_of(&env, "tome-1-build");
    let a = pane_of(&env, "tome-1-build-a");
    let window = |p: &str| lines(env.tmux(&["display-message", "-p", "-t", p, "#{window_id}"]))[0].clone();
    assert_eq!(window(&a), window(&orch));
    // With the orchestrator's pane gone, the next opens off the last (a).
    assert!(env.tmux(&["kill-pane", "-t", &orch]).status.success());
    spawn_with(&env, 1, "b", &["--layout", "split", "--from", "orchestrator"], &["sleep", "600"]);
    let b = pane_of(&env, "tome-1-build-b");
    assert_eq!(window(&b), window(&a));
}
