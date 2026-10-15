mod common;

use common::Env;
use serde_json::{json, Value};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Write a plugin folder under the env's scratch dir. `files` are relative
/// paths and contents; anything under `bin/` is made executable.
fn plugin_dir(env: &Env, folder: &str, manifest: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = env.dir.path().join("src").join(folder);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("plugin.yaml"), manifest).unwrap();
    for (rel, text) in files {
        let path = dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        if rel.starts_with("bin/") {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    dir
}

const WORKFLOW: &str =
    "---\nname: provision\ndescription: Provision a box\n---\n\n## Do it\n\nDo the thing.\n";

fn full_plugin(env: &Env) -> PathBuf {
    plugin_dir(
        env,
        "cloudvm",
        "name: cloudvm\nversion: 0.1.0\nprotocol: 1\n\
         subcommands:\n  cloud: bin/cloud\n\
         hooks:\n  provide:\n  release:\n  run.finished:\n\
         providers: [cloud-vm]\nworkflows: [provision]\n\
         harnesses:\n  mine: [\"mine-agent\", \"{{prompt}}\"]\n",
        &[
            // Running this would leave a marker: install must never run plugin code.
            ("bin/cloud", "#!/bin/sh\ntouch \"$(dirname \"$0\")/ran\"\n"),
            ("workflows/provision.md", WORKFLOW),
        ],
    )
}

fn install(env: &Env, source: &Path) -> (i32, Value) {
    env.json(&["plugin", "install", source.to_str().unwrap()])
}

fn state_of(env: &Env, name: &str) -> String {
    let (code, out) = env.json(&["plugin", "ls"]);
    assert_eq!(code, 0, "{out}");
    out["plugins"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == name)
        .unwrap_or_else(|| panic!("{name} not listed: {out}"))["state"]
        .as_str()
        .unwrap()
        .to_string()
}

fn error_text(out: &Value) -> String {
    out["error"].to_string()
}

#[test]
fn install_copies_the_folder_without_running_it() {
    let env = Env::new();
    let src = full_plugin(&env);
    let (code, out) = install(&env, &src);
    assert_eq!(code, 0, "{out}");
    assert_eq!(out["name"], "cloudvm");

    let installed = env.home().join("plugins/cloudvm");
    assert!(installed.join("plugin.yaml").is_file());
    assert!(installed.join("workflows/provision.md").is_file());
    assert!(
        !installed.join("bin/ran").exists(),
        "install ran plugin code"
    );
    let mode = fs::metadata(installed.join("bin/cloud"))
        .unwrap()
        .permissions()
        .mode();
    assert_ne!(mode & 0o111, 0, "the executable bit is kept");

    let (code, out) = env.json(&["plugin", "ls"]);
    assert_eq!(code, 0, "{out}");
    let p = &out["plugins"][0];
    assert_eq!(p["state"], "enabled");
    assert_eq!(p["version"], "0.1.0");
    assert_eq!(p["hooks"], json!(["provide", "release", "run.finished"]));
    assert_eq!(p["subcommands"], json!(["cloud"]));
    assert_eq!(p["providers"], json!(["cloud-vm"]));
    assert_eq!(p["conflicts"], json!([]));
}

#[test]
fn ls_with_nothing_installed_says_so() {
    let env = Env::new();
    let (code, out) = env.json(&["plugin", "ls"]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(out["plugins"], json!([]));
}

#[test]
fn install_refuses_name_conflicts_with_exit_2_naming_both_sources() {
    let env = Env::new();
    env.json(&["workflow", "new", "provision", "--global"]);
    env.json(&["node", "add", "cloud-vm", "me@host"]);

    // A built-in command name for a subcommand.
    let clash = plugin_dir(
        &env,
        "bad",
        "name: bad\nprotocol: 1\nsubcommands:\n  run: bin/run\n",
        &[("bin/run", "#!/bin/sh\n")],
    );
    let (code, out) = install(&env, &clash);
    assert_eq!(code, 2, "{out}");
    assert!(error_text(&out).contains("command `run`"), "{out}");
    assert!(error_text(&out).contains("a built-in command"), "{out}");

    // The workflow and the provider clash with the user's own.
    let src = full_plugin(&env);
    let (code, out) = install(&env, &src);
    assert_eq!(code, 2, "{out}");
    let text = error_text(&out);
    assert!(
        text.contains("workflow `provision`") && text.contains("your workflow"),
        "{out}"
    );
    assert!(
        text.contains("provider `cloud-vm`") && text.contains("configured node `cloud-vm`"),
        "{out}"
    );
    assert!(
        !env.home().join("plugins/cloudvm").exists(),
        "nothing installed"
    );
}

#[test]
fn install_refuses_a_clash_with_another_plugin() {
    let env = Env::new();
    let a = plugin_dir(
        &env,
        "a",
        "name: a\nprotocol: 1\nworkflows: [provision]\n",
        &[("workflows/provision.md", WORKFLOW)],
    );
    let b = plugin_dir(
        &env,
        "b",
        "name: b\nprotocol: 1\nworkflows: [provision]\n",
        &[("workflows/provision.md", WORKFLOW)],
    );
    assert_eq!(install(&env, &a).0, 0);
    let (code, out) = install(&env, &b);
    assert_eq!(code, 2, "{out}");
    assert!(error_text(&out).contains("plugin `a`"), "{out}");
}

#[test]
fn install_refuses_bad_manifests() {
    let env = Env::new();
    let cases = [
        (
            "unknown",
            "name: x\nprotocol: 1\nhooks: [run.exploded]\n",
            "unknown hook",
        ),
        (
            "reserved",
            "name: x\nprotocol: 1\nhooks: [worker.place]\n",
            "not yet supported",
        ),
        ("noproto", "name: x\n", "`protocol` is required"),
        (
            "nofile",
            "name: x\nprotocol: 1\nsubcommands:\n  go: bin/missing\n",
            "doesn't exist",
        ),
    ];
    for (folder, manifest, expect) in cases {
        let src = plugin_dir(&env, folder, manifest, &[]);
        let (code, out) = install(&env, &src);
        assert_eq!(code, 2, "{folder}: {out}");
        assert!(error_text(&out).contains(expect), "{folder}: {out}");
    }
    // Not a plugin folder at all.
    let (code, out) = env.json(&["plugin", "install", "/nonexistent/plugin"]);
    assert_eq!(code, 2, "{out}");

    // A command that isn't executable, and a workflow named differently.
    let src = plugin_dir(
        &env,
        "noexec",
        "name: x\nprotocol: 1\nsubcommands:\n  go: lib/go\n",
        &[("lib/go", "#!/bin/sh\n")],
    );
    let (code, out) = install(&env, &src);
    assert_eq!(code, 2);
    assert!(error_text(&out).contains("isn't executable"), "{out}");
    let src = plugin_dir(
        &env,
        "badwf",
        "name: x\nprotocol: 1\nworkflows: [other]\n",
        &[("workflows/other.md", WORKFLOW)],
    );
    let (code, out) = install(&env, &src);
    assert_eq!(code, 2);
    assert!(error_text(&out).contains("named `provision`"), "{out}");
    assert!(!env.home().join("plugins/x").exists());
}

#[test]
fn install_refuses_an_incompatible_plugin_with_a_hint() {
    let env = Env::new();
    let src = plugin_dir(&env, "future", "name: future\nprotocol: 99\n", &[]);
    let (code, out) = install(&env, &src);
    assert_eq!(code, 2, "{out}");
    assert!(error_text(&out).contains("protocol 99"), "{out}");
    assert_eq!(out["error"]["hint"], "upgrade tome");

    let src = plugin_dir(&env, "old", "name: old\nprotocol: 1\ntome: \"<0.1\"\n", &[]);
    let (code, out) = install(&env, &src);
    assert_eq!(code, 2, "{out}");
    assert!(
        out["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("tome plugin update old"),
        "{out}"
    );
}

#[test]
fn installing_twice_is_refused_and_rm_removes() {
    let env = Env::new();
    let src = full_plugin(&env);
    assert_eq!(install(&env, &src).0, 0);
    let (code, out) = install(&env, &src);
    assert_eq!(code, 2, "{out}");
    assert!(error_text(&out).contains("already installed"), "{out}");

    let (code, out) = env.json(&["plugin", "rm", "cloudvm"]);
    assert_eq!(code, 0, "{out}");
    assert!(!env.home().join("plugins/cloudvm").exists());
    assert_eq!(env.json(&["plugin", "rm", "cloudvm"]).0, 4);
    assert_eq!(env.json(&["plugin", "rm", "../x"]).0, 4);
    // Out of the way, the name is free again.
    assert_eq!(install(&env, &src).0, 0);
}

#[test]
fn enable_and_disable_are_idempotent_and_shown_in_ls() {
    let env = Env::new();
    assert_eq!(install(&env, &full_plugin(&env)).0, 0);
    assert_eq!(state_of(&env, "cloudvm"), "enabled");

    let (code, out) = env.json(&["plugin", "disable", "cloudvm"]);
    assert_eq!((code, &out["changed"]), (0, &json!(true)), "{out}");
    assert_eq!(state_of(&env, "cloudvm"), "disabled");
    let (_, out) = env.json(&["plugin", "disable", "cloudvm"]);
    assert_eq!(out["changed"], false);

    let (code, out) = env.json(&["plugin", "enable", "cloudvm"]);
    assert_eq!((code, &out["changed"]), (0, &json!(true)), "{out}");
    assert_eq!(state_of(&env, "cloudvm"), "enabled");
    assert_eq!(env.json(&["plugin", "enable", "nope"]).0, 4);
}

#[test]
fn management_commands_refuse_on() {
    let env = Env::new();
    env.json(&["node", "add", "box", "me@host"]);
    for args in [
        vec!["plugin", "ls"],
        vec!["plugin", "install", "./x"],
        vec!["plugin", "update"],
        vec!["plugin", "rm", "x"],
        vec!["plugin", "enable", "x"],
        vec!["plugin", "disable", "x"],
    ] {
        let mut full = vec!["--on", "box"];
        full.extend(&args);
        let (code, out) = env.json(&full);
        assert_eq!(code, 2, "{args:?}: {out}");
        assert!(
            error_text(&out).contains("plugins are installed per machine"),
            "{out}"
        );
    }
}

#[test]
fn ls_flags_a_conflict_that_appears_after_install() {
    let env = Env::new();
    let src = plugin_dir(
        &env,
        "wf",
        "name: wf\nprotocol: 1\nworkflows: [provision]\n",
        &[("workflows/provision.md", WORKFLOW)],
    );
    assert_eq!(install(&env, &src).0, 0);
    env.json(&["workflow", "new", "provision", "--global"]);

    let (code, out) = env.json(&["plugin", "ls"]);
    assert_eq!(code, 0, "{out}");
    let conflict = &out["plugins"][0]["conflicts"][0];
    assert_eq!(conflict["kind"], "workflow");
    assert_eq!(conflict["name"], "provision");
    assert!(conflict["theirs"]
        .as_str()
        .unwrap()
        .contains("your workflow"));

    let human = env.run(&["plugin", "ls"]);
    assert!(String::from_utf8_lossy(&human.stdout).contains("conflict: workflow `provision`"));
}

#[test]
fn ls_shows_an_installed_plugin_that_became_incompatible_or_invalid() {
    let env = Env::new();
    assert_eq!(
        install(
            &env,
            &plugin_dir(&env, "ok", "name: ok\nprotocol: 1\n", &[])
        )
        .0,
        0
    );
    assert_eq!(
        install(
            &env,
            &plugin_dir(&env, "later", "name: later\nprotocol: 1\n", &[])
        )
        .0,
        0
    );
    // A tome upgrade or a bad edit after install: the plugin stays installed.
    fs::write(
        env.home().join("plugins/ok/plugin.yaml"),
        "name: ok\nprotocol: 7\n",
    )
    .unwrap();
    fs::write(
        env.home().join("plugins/later/plugin.yaml"),
        "name: later\nprotocol: 1\nhooks: [nope]\n",
    )
    .unwrap();

    assert_eq!(state_of(&env, "ok"), "incompatible");
    assert_eq!(state_of(&env, "later"), "invalid");
    let (_, out) = env.json(&["plugin", "ls"]);
    let by_name = |n: &str| {
        out["plugins"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == n)
            .unwrap()
            .clone()
    };
    assert_eq!(by_name("ok")["incompatible"]["hint"], "upgrade tome");
    assert!(by_name("later")["error"]
        .as_str()
        .unwrap()
        .contains("unknown hook"));
}

// ---- git installs ----------------------------------------------------------

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn commit_all(dir: &Path, msg: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", msg]);
}

fn git_plugin(env: &Env) -> PathBuf {
    let dir = plugin_dir(
        env,
        "gitp",
        "name: gitp\nversion: 1.0.0\nprotocol: 1\n",
        &[],
    );
    git(&dir, &["init", "-q", "-b", "main"]);
    commit_all(&dir, "one");
    dir
}

#[test]
fn git_install_and_update_pull_new_commits() {
    let env = Env::new();
    let repo = git_plugin(&env);
    let url = format!("file://{}", repo.display());
    let (code, out) = env.json(&["plugin", "install", &url]);
    assert_eq!(code, 0, "{out}");
    assert!(
        env.home().join("plugins/gitp/.git").is_dir(),
        "a git install keeps its clone"
    );

    let (_, out) = env.json(&["plugin", "update", "gitp"]);
    assert_eq!(out["plugins"][0]["status"], "up-to-date", "{out}");

    fs::write(
        repo.join("plugin.yaml"),
        "name: gitp\nversion: 1.1.0\nprotocol: 1\n",
    )
    .unwrap();
    commit_all(&repo, "two");
    let (code, out) = env.json(&["plugin", "update"]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(out["plugins"][0]["status"], "updated", "{out}");
    let (_, ls) = env.json(&["plugin", "ls"]);
    assert_eq!(ls["plugins"][0]["version"], "1.1.0");
}

#[test]
fn git_install_can_pin_a_ref() {
    let env = Env::new();
    let repo = git_plugin(&env);
    git(&repo, &["tag", "v1"]);
    fs::write(
        repo.join("plugin.yaml"),
        "name: gitp\nversion: 2.0.0\nprotocol: 1\n",
    )
    .unwrap();
    commit_all(&repo, "two");

    let url = format!("file://{}#v1", repo.display());
    let (code, out) = env.json(&["plugin", "install", &url]);
    assert_eq!(code, 0, "{out}");
    let (_, ls) = env.json(&["plugin", "ls"]);
    assert_eq!(ls["plugins"][0]["version"], "1.0.0");
    assert_eq!(ls["plugins"][0]["ref"], "v1");

    let (_, out) = env.json(&["plugin", "update", "gitp"]);
    assert_eq!(
        out["plugins"][0]["status"], "up-to-date",
        "a tag stays where it is: {out}"
    );
}

#[test]
fn a_bad_update_is_rolled_back() {
    let env = Env::new();
    env.json(&["workflow", "new", "taken", "--global"]);
    let repo = git_plugin(&env);
    let url = format!("file://{}", repo.display());
    assert_eq!(env.json(&["plugin", "install", &url]).0, 0);

    // The new version claims a workflow name the user already has.
    fs::create_dir_all(repo.join("workflows")).unwrap();
    fs::write(
        repo.join("workflows/taken.md"),
        "---\nname: taken\ndescription: x\n---\n\n## Go\n\nGo.\n",
    )
    .unwrap();
    fs::write(
        repo.join("plugin.yaml"),
        "name: gitp\nversion: 1.1.0\nprotocol: 1\nworkflows: [taken]\n",
    )
    .unwrap();
    commit_all(&repo, "clash");

    let (code, out) = env.json(&["plugin", "update", "gitp"]);
    assert_eq!(code, 1, "{out}");
    assert_eq!(out["plugins"][0]["status"], "failed");
    assert!(
        out["plugins"][0]["detail"]
            .as_str()
            .unwrap()
            .contains("rolled back"),
        "{out}"
    );
    let (_, ls) = env.json(&["plugin", "ls"]);
    assert_eq!(
        ls["plugins"][0]["version"], "1.0.0",
        "still the old version"
    );
    assert_eq!(ls["plugins"][0]["conflicts"], json!([]));
}

#[test]
fn updating_a_path_install_is_skipped() {
    let env = Env::new();
    assert_eq!(install(&env, &full_plugin(&env)).0, 0);
    let (code, out) = env.json(&["plugin", "update"]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(out["plugins"][0]["status"], "skipped");
}
