//! A project's `.tome/config.yaml`: the same keys as `~/.tome/config.yaml`,
//! winning per key and per named entry.

mod common;

use common::Env;
use std::fs;

fn write_wf(env: &Env, name: &str, defaults: &str) {
    let dir = env.project().join(".tome/workflows");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{name}.md")), format!("---\nname: {name}\n{defaults}---\n## Build\nGo.\n")).unwrap();
}

fn set_project_config(env: &Env, yaml: &str) {
    let dir = env.project().join(".tome");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("config.yaml"), yaml).unwrap();
}

/// An env with no `TOME_LAYOUT`, so the config decides.
fn env() -> Env {
    let mut env = Env::new();
    env.vars.retain(|(k, _)| k != "TOME_LAYOUT");
    env
}

#[test]
fn the_project_config_wins_per_key_and_per_harness() {
    let env = env();
    // The global config's layout and `claude` harness are both bad; the
    // project's replace them. Its `backend` isn't overridden.
    env.set_config("backend: tmux\nlayout: sideways\nharnesses:\n  claude: [no-such-agent]\n  other: [echo]\n");
    set_project_config(&env, "layout: workspace\nharnesses:\n  claude: [sleep, \"600\"]\n");
    write_wf(&env, "build", "");
    env.start_daemon();
    let (code, run) = env.json(&["run", "build", "--detach"]);
    assert_eq!(code, 0, "{run}");
    let (_, shown) = env.json(&["runs", "show", "1"]);
    let s = &shown["sessions"][0];
    assert_eq!((s["layout"].as_str(), s["backend"].as_str()), (Some("workspace"), Some("tmux")), "{s}");
    common::eventually("the orchestrator's own session", || env.has_session("tome-1-build"));
    let script = fs::read_to_string(env.home().join("runs/1/orchestrator.sh")).unwrap();
    assert!(script.contains("sleep") && !script.contains("no-such-agent"), "the project's claude harness: {script}");
}

#[test]
fn a_bad_value_in_the_project_config_names_the_file() {
    let env = env();
    set_project_config(&env, "layout: sideways\n");
    write_wf(&env, "build", "");
    env.start_daemon();
    let (code, err) = env.json(&["run", "build", "--detach"]);
    assert_eq!(code, 2, "{err}");
    let message = err["error"]["message"].as_str().unwrap();
    assert!(message.contains("unknown session layout `sideways` (from `layout` in the project config ("), "{err}");
}

#[test]
fn an_unknown_key_is_refused_in_either_config() {
    for project in [false, true] {
        let env = env();
        let yaml = "backend: tmux\nlayot: split\n";
        if project {
            set_project_config(&env, yaml);
        } else {
            env.set_config(&format!("{}{yaml}", common::IDLE_CONFIG));
        }
        write_wf(&env, "build", "");
        env.start_daemon();
        let (code, err) = env.json(&["run", "build", "--detach"]);
        assert_eq!(code, 2, "{err}");
        let message = err["error"]["message"].as_str().unwrap();
        assert!(message.contains("config.yaml: unknown key `layot`"), "{err}");
        assert!(err["error"]["hint"].as_str().unwrap().contains("backend, harnesses, layout, layout_presets"), "{err}");
        let (_, runs) = env.json(&["runs", "list"]);
        assert_eq!(runs["runs"].as_array().unwrap().len(), 0);
    }
}
