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
        "name: cloudvm\nversion: 0.1.0\nprotocol: 1\ncommand: bin/hooks\n\
         subcommands:\n  cloud: bin/cloud\n\
         hooks:\n  provide:\n  release:\n  run.finished:\n\
         providers: [cloud-vm]\nworkflows: [provision]\n\
         harnesses:\n  mine: [\"mine-agent\", \"{{prompt}}\"]\n",
        &[
            // Running this would leave a marker: install must never run plugin code.
            ("bin/cloud", "#!/bin/sh\ntouch \"$(dirname \"$0\")/ran\"\n"),
            ("bin/hooks", "#!/bin/sh\ntouch \"$(dirname \"$0\")/ran\"\n"),
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

// ---- subcommands (020-2) ---------------------------------------------------

const ECHO_CMD: &str =
    "#!/bin/sh\necho \"hello $TOME_PLUGIN $* $(basename \"$TOME_PLUGIN_DATA\")\"\nexit 7\n";

fn cmd_plugin(env: &Env, name: &str, sub: &str) -> PathBuf {
    plugin_dir(
        env,
        name,
        &format!("name: {name}\nprotocol: 1\nsubcommands:\n  {sub}: bin/{sub}\n"),
        &[(&format!("bin/{sub}"), ECHO_CMD)],
    )
}

#[test]
fn a_plugin_subcommand_runs_directly_with_args_and_its_exit_code() {
    let env = Env::new();
    assert_eq!(install(&env, &cmd_plugin(&env, "cloudvm", "cloud")).0, 0);
    let out = env.run(&["cloud", "up", "--fast"]);
    assert_eq!(out.status.code(), Some(7));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "hello cloudvm up --fast data"
    );
}

#[test]
fn disabled_or_incompatible_plugins_contribute_no_subcommands() {
    let env = Env::new();
    assert_eq!(install(&env, &cmd_plugin(&env, "cloudvm", "cloud")).0, 0);
    env.json(&["plugin", "disable", "cloudvm"]);
    let out = env.run(&["cloud"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown command `cloud`"));

    env.json(&["plugin", "enable", "cloudvm"]);
    assert_eq!(env.run(&["cloud"]).status.code(), Some(7));
    fs::write(
        env.home().join("plugins/cloudvm/plugin.yaml"),
        "name: cloudvm\nprotocol: 9\nsubcommands:\n  cloud: bin/cloud\n",
    )
    .unwrap();
    assert_eq!(env.run(&["cloud"]).status.code(), Some(2));
}

#[test]
fn built_in_commands_win_over_a_plugin_subcommand() {
    let env = Env::new();
    assert_eq!(install(&env, &cmd_plugin(&env, "cloudvm", "cloud")).0, 0);
    // Slip a built-in name in after install, as a hand edit would.
    fs::write(
        env.home().join("plugins/cloudvm/plugin.yaml"),
        "name: cloudvm\nprotocol: 1\nsubcommands:\n  workflow: bin/cloud\n",
    )
    .unwrap();
    let out = env.run(&["workflow", "ls"]);
    assert_ne!(out.status.code(), Some(7), "the plugin shadowed a built-in");
}

#[test]
fn plugin_subcommands_refuse_on() {
    let env = Env::new();
    assert_eq!(install(&env, &cmd_plugin(&env, "cloudvm", "cloud")).0, 0);
    let (code, out) = env.json(&["--on", "somewhere", "cloud"]);
    assert_eq!(code, 2, "{out}");
}

#[test]
fn an_unknown_command_still_exits_2() {
    let env = Env::new();
    let (code, out) = env.json(&["nonsense"]);
    assert_eq!(code, 2, "{out}");
}

// ---- workflows and harnesses (020-3) --------------------------------------

fn find<'a>(list: &'a Value, key: &str, name: &str) -> Vec<&'a Value> {
    list[key]
        .as_array()
        .unwrap()
        .iter()
        .filter(|w| w["name"] == name)
        .collect()
}

#[test]
fn plugin_workflows_and_harnesses_are_available_by_plain_name_with_their_source() {
    let env = Env::new();
    assert_eq!(install(&env, &full_plugin(&env)).0, 0);

    let (code, out) = env.json(&["workflow", "ls"]);
    assert_eq!(code, 0, "{out}");
    let found = find(&out, "workflows", "provision");
    assert_eq!(found.len(), 1, "{out}");
    assert_eq!(found[0]["source"], "plugin:cloudvm");
    assert_eq!(found[0]["valid"], true);
    let human = String::from_utf8_lossy(&env.run(&["workflow", "ls"]).stdout).to_string();
    assert!(human.contains("plugin cloudvm"), "{human}");

    let (code, out) = env.json(&["validate", "provision"]);
    assert_eq!(code, 0, "{out}");
    let (code, out) = env.json(&["validate"]);
    assert_eq!(code, 0, "{out}");

    let (code, out) = env.json(&["harness", "ls"]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(
        find(&out, "harnesses", "mine")[0]["source"],
        "plugin:cloudvm"
    );
    let (code, out) = env.json(&["harness", "validate", "mine"]);
    // The program doesn't exist on this machine, but the harness resolves.
    assert!(code == 0 || code == 2, "{out}");
    assert_eq!(out["harnesses"][0]["name"], "mine");
}

#[test]
fn a_disabled_plugin_offers_no_workflows_or_harnesses() {
    let env = Env::new();
    assert_eq!(install(&env, &full_plugin(&env)).0, 0);
    env.json(&["plugin", "disable", "cloudvm"]);
    let (_, out) = env.json(&["workflow", "ls"]);
    assert!(find(&out, "workflows", "provision").is_empty(), "{out}");
    let (_, out) = env.json(&["harness", "ls"]);
    assert!(find(&out, "harnesses", "mine").is_empty(), "{out}");
}

#[test]
fn a_workflow_name_clash_after_install_errors_and_names_both_sources() {
    let env = Env::new();
    assert_eq!(install(&env, &full_plugin(&env)).0, 0);
    // The user adds their own `provision` afterwards.
    let (code, _) = env.json(&["workflow", "new", "provision", "--global"]);
    assert_eq!(code, 0);

    let (code, out) = env.json(&["validate", "provision"]);
    assert_eq!(code, 2, "{out}");
    let text = error_text(&out);
    assert!(
        text.contains("plugin `cloudvm`") && text.contains("your workflow"),
        "{out}"
    );

    let (code, out) = env.json(&["validate"]);
    assert_eq!(code, 2, "{out}");
    assert_eq!(out["valid"], false);
    let c = &out["plugin_conflicts"][0];
    assert_eq!(
        (c["kind"].as_str(), c["name"].as_str()),
        (Some("workflow"), Some("provision"))
    );

    // Starting it refuses too, before any daemon is needed.
    let (code, out) = env.json(&["run", "provision", "--detach"]);
    assert_eq!(code, 2, "{out}");
    assert!(error_text(&out).contains("more than once"), "{out}");

    let (_, out) = env.json(&["workflow", "ls"]);
    assert_eq!(find(&out, "workflows", "provision").len(), 2);
    assert!(find(&out, "workflows", "provision")
        .iter()
        .all(|w| !w["conflicts"].as_array().unwrap().is_empty()));

    assert_eq!(state_of(&env, "cloudvm"), "enabled");
    let (_, out) = env.json(&["plugin", "ls"]);
    assert_eq!(out["plugins"][0]["conflicts"][0]["name"], "provision");

    // Plugin workflows aren't the user's to remove.
    let (code, _) = env.json(&["workflow", "rm", "provision", "--global", "--force"]);
    assert_eq!(code, 0);
    assert!(env
        .home()
        .join("plugins/cloudvm/workflows/provision.md")
        .is_file());
    let (code, out) = env.json(&["validate", "provision"]);
    assert_eq!(code, 0, "{out}");
}

#[test]
fn a_harness_name_clash_after_install_errors_on_use() {
    let env = Env::new();
    assert_eq!(install(&env, &full_plugin(&env)).0, 0);
    env.set_config("harnesses:\n  mine: [other, \"{{prompt}}\"]\n");
    let (code, out) = env.json(&["harness", "validate", "mine"]);
    assert_eq!(code, 2, "{out}");
    assert!(
        out["harnesses"][0]["error"]
            .as_str()
            .unwrap()
            .contains("plugin `cloudvm`"),
        "{out}"
    );
    let (code, out) = env.json(&["validate"]);
    assert_eq!(code, 2, "{out}");
    assert_eq!(out["plugin_conflicts"][0]["kind"], "harness");
    let (_, out) = env.json(&["harness", "ls"]);
    assert_eq!(find(&out, "harnesses", "mine").len(), 2);
}

// ---------------------------------------------------------------------------
// Running plugins: supervision, hooks, notify, before_start

/// A plugin that speaks the protocol. Its behavior is set by files in its
/// state dir (`~/.tome/plugins/<name>/data`), which the tests write: so a
/// test can change it while the daemon runs. Everything it is sent is
/// appended to `calls.jsonl` there.
const FAKE: &str = r#"#!/usr/bin/env python3
import sys, json, os
data = os.environ["TOME_PLUGIN_DATA"]

def knob(name, default=""):
    try:
        return open(os.path.join(data, name)).read().strip()
    except OSError:
        return default

def send(o):
    sys.stdout.write(json.dumps(o) + "\n")
    sys.stdout.flush()

if knob("die_on_start"):
    sys.stderr.write("refusing to start\n")
    sys.exit(3)

for line in sys.stdin:
    m = json.loads(line)
    method, id_ = m.get("method"), m.get("id")
    with open(os.path.join(data, "calls.jsonl"), "a") as f:
        f.write(json.dumps({"method": method, "params": m.get("params")}) + "\n")
    if id_ is None:
        continue
    if knob("hang_" + method):
        continue
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": id_, "result": {"protocol": int(knob("protocol", "1"))}})
    elif method == "run.before_start":
        mode = knob("before_start", "allow")
        if mode == "crash":
            sys.exit(1)
        if mode == "reject":
            r = {"action": "reject", "reason": "over budget"}
        elif mode == "modify":
            r = {"action": "modify", "params": {"base": "dev"}, "env": {"MY_VAR": "hello"}}
        elif mode == "garbage":
            r = {"action": "dance"}
        else:
            r = {"action": "allow"}
        send({"jsonrpc": "2.0", "id": id_, "result": r})
    else:
        if knob("notify_on_" + method):
            send({"jsonrpc": "2.0", "id": "n1", "method": "notify",
                  "params": {"title": "hello", "message": knob("notify_on_" + method)}})
        send({"jsonrpc": "2.0", "id": id_, "result": {}})
"#;

const BUILD: &str = "---\nname: build\nparams:\n  base: {default: main}\n---\n## Build\nBranch off {{params.base}}.\n";

/// Install the fake plugin as `fake` with these hooks (manifest `hooks:` body).
fn fake_plugin(env: &Env, hooks: &str) {
    let src = plugin_dir(
        env,
        "fake",
        &format!("name: fake\nprotocol: 1\ncommand: bin/fake\nhooks:\n{hooks}"),
        &[("bin/fake", FAKE)],
    );
    let (code, out) = install(env, &src);
    assert_eq!(code, 0, "{out}");
}

fn knob(env: &Env, name: &str, value: &str) {
    let dir = env.home().join("plugins/fake/data");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(name), value).unwrap();
}

fn calls(env: &Env) -> Vec<Value> {
    fs::read_to_string(env.home().join("plugins/fake/data/calls.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn called(env: &Env, method: &str) -> Option<Value> {
    calls(env).into_iter().find(|c| c["method"] == method)
}

fn write_build(env: &Env) {
    let dir = env.project().join(".tome/workflows");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("build.md"), BUILD).unwrap();
}

fn fast_restarts(env: &mut Env) {
    env.set_var("TOME_PLUGIN_BACKOFF_MS", "20");
}

#[test]
fn the_daemon_starts_a_plugin_and_initializes_it() {
    let env = Env::new();
    fake_plugin(&env, "  daemon.started:\n  run.finished:\n");
    env.start_daemon();
    common::eventually("the plugin to run", || state_of(&env, "fake") == "running");

    let init = called(&env, "initialize").expect("initialize was sent");
    assert_eq!(init["params"]["protocol"], 1);
    assert_eq!(init["params"]["tome_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        init["params"]["state_dir"],
        json!(env.home().join("plugins/fake/data"))
    );
    assert_eq!(calls(&env)[0]["method"], "initialize", "it goes first");
    common::eventually("daemon.started", || {
        called(&env, "daemon.started").is_some()
    });
    assert_eq!(
        called(&env, "daemon.started").unwrap()["params"]["pid"],
        env.rpc_ok("daemon.status", json!({}))["pid"]
    );
    assert!(
        env.home().join("plugins/fake/plugin.log").is_file(),
        "the plugin has a log"
    );
}

#[test]
fn a_protocol_mismatch_disables_the_plugin_as_incompatible() {
    let env = Env::new();
    fake_plugin(&env, "  run.finished:\n");
    knob(&env, "protocol", "2");
    env.start_daemon();
    common::eventually("incompatible", || state_of(&env, "fake") == "incompatible");
    let (_, out) = env.json(&["plugin", "ls"]);
    let note = find(&out, "plugins", "fake")[0]["runtime"]["error"].to_string();
    assert!(note.contains("protocol 2"), "{note}");
    // Not restarted: it was asked once.
    assert_eq!(
        calls(&env)
            .iter()
            .filter(|c| c["method"] == "initialize")
            .count(),
        1
    );
}

#[test]
fn a_crashing_plugin_is_restarted_then_marked_failed_and_tome_carries_on() {
    let mut env = Env::new();
    fast_restarts(&mut env);
    fake_plugin(&env, "  run.finished:\n");
    knob(&env, "die_on_start", "1");
    env.start_daemon();
    common::eventually("failed", || state_of(&env, "fake") == "failed");

    let log = fs::read_to_string(env.home().join("plugins/fake/plugin.log")).unwrap();
    assert!(log.contains("refusing to start"), "stderr is logged: {log}");
    // A bad plugin never breaks tome.
    write_build(&env);
    let (code, v) = env.json(&["run", "build", "--detach"]);
    assert_eq!(code, 0, "{v}");
    let (code, _) = env.json(&["daemon", "status"]);
    assert_eq!(code, 0);
}

#[test]
fn a_plugin_that_dies_mid_run_comes_back() {
    let mut env = Env::new();
    fast_restarts(&mut env);
    fake_plugin(&env, "  run.before_start: {on_error: allow}\n");
    write_build(&env);
    env.start_daemon();
    common::eventually("running", || state_of(&env, "fake") == "running");
    knob(&env, "before_start", "crash");
    // The call in flight fails (and, with on_error allow, the run goes on).
    let (code, v) = env.json(&["run", "build", "--detach"]);
    assert_eq!(code, 0, "{v}");
    knob(&env, "before_start", "allow");
    common::eventually("running again", || state_of(&env, "fake") == "running");
    assert!(
        calls(&env)
            .iter()
            .filter(|c| c["method"] == "initialize")
            .count()
            >= 2,
        "it was initialized again"
    );
}

#[test]
fn plugin_check_walks_the_steps_and_stops_at_the_first_failure() {
    let env = Env::new();
    fake_plugin(&env, "  run.finished:\n");
    let (code, out) = env.json(&["plugin", "check", "fake"]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(out["ok"], true);
    let steps: Vec<_> = out["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["step"].as_str().unwrap())
        .collect();
    assert_eq!(steps, ["manifest", "compatible", "files", "names", "start"]);

    knob(&env, "die_on_start", "1");
    let (code, out) = env.json(&["plugin", "check", "fake"]);
    assert_eq!(code, 2, "{out}");
    assert_eq!(out["ok"], false);
    let last = out["steps"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(
        (last["step"].as_str(), last["ok"].as_bool()),
        (Some("start"), Some(false))
    );
    assert!(
        last["fix"].as_str().unwrap().contains("plugin.log"),
        "{last}"
    );

    knob(&env, "die_on_start", "");
    knob(&env, "protocol", "9");
    let (code, out) = env.json(&["plugin", "check", "fake"]);
    assert_eq!(code, 2, "{out}");
    assert!(out["steps"].to_string().contains("protocol 9"), "{out}");

    let (code, out) = env.json(&["plugin", "check", "nope"]);
    assert_eq!(code, 4, "{out}");
}

#[test]
fn plugin_check_reports_a_name_clash() {
    let env = Env::new();
    fake_plugin(&env, "  run.finished:\n");
    let manifest = env.home().join("plugins/fake/plugin.yaml");
    let text = fs::read_to_string(&manifest).unwrap();
    fs::write(&manifest, format!("{text}workflows: [build]\n")).unwrap();
    fs::create_dir_all(env.home().join("plugins/fake/workflows")).unwrap();
    fs::write(env.home().join("plugins/fake/workflows/build.md"), BUILD).unwrap();
    write_build(&env);
    env.json(&["workflow", "new", "build", "--global"]);
    let (code, out) = env.json(&["plugin", "check", "fake"]);
    assert_eq!(code, 2, "{out}");
    let steps = out["steps"].as_array().unwrap();
    assert_eq!(steps.last().unwrap()["step"], "names");
}

#[test]
fn run_hooks_are_notifications_that_a_hung_plugin_cannot_hold_up() {
    let env = Env::new();
    fake_plugin(&env, "  run.started:\n  run.finished:\n");
    write_build(&env);
    env.start_daemon();
    common::eventually("running", || state_of(&env, "fake") == "running");
    knob(&env, "hang_run.finished", "1");

    let (code, v) = env.json(&["run", "build", "--detach"]);
    assert_eq!(code, 0, "{v}");
    let id = v["id"].as_i64().unwrap();
    common::eventually("run.started", || called(&env, "run.started").is_some());
    let started = called(&env, "run.started").unwrap();
    assert_eq!(started["params"]["run_id"], id);
    assert_eq!(started["params"]["workflow"], "build");

    // The run ends at once, whatever the plugin does with the news.
    let (code, v) = env.json(&["run", "cancel", &id.to_string()]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["status"], "cancelled");
    common::eventually("run.finished", || called(&env, "run.finished").is_some());
    let finished = called(&env, "run.finished").unwrap();
    assert_eq!(finished["params"]["status"], "cancelled");
    assert_eq!(finished["params"]["run_id"], id);
    // And a new run still starts.
    let (code, _) = env.json(&["run", "build", "--detach"]);
    assert_eq!(code, 0);
}

#[test]
fn a_plugin_can_ask_tome_to_notify_and_so_can_the_cli() {
    let env = Env::new();
    fake_plugin(&env, "  run.started:\n");
    write_build(&env);
    env.start_daemon();
    common::eventually("running", || state_of(&env, "fake") == "running");
    knob(&env, "notify_on_run.started", "from the plugin");
    let (code, _) = env.json(&["run", "build", "--detach"]);
    assert_eq!(code, 0);
    let log = || fs::read_to_string(env.home().join("daemon.log")).unwrap_or_default();
    common::eventually("the plugin's notification", || {
        log().contains("notify: fake: hello: from the plugin")
    });

    let (code, out) = env.json(&["notify", "Build", "all done"]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(out["notified"], true);
    common::eventually("the CLI's notification", || {
        log().contains("notify: Build: all done")
    });
    let (code, out) = env.json(&["notify", "", "x"]);
    assert_eq!(code, 2, "{out}");
}

#[test]
fn before_start_can_reject_a_run_with_a_reason() {
    let env = Env::new();
    fake_plugin(&env, "  run.before_start:\n");
    write_build(&env);
    env.start_daemon();
    common::eventually("running", || state_of(&env, "fake") == "running");
    let (code, v) = env.json(&["run", "build", "--detach"]);
    assert_eq!(code, 0, "an allowed run starts: {v}");

    knob(&env, "before_start", "reject");
    let (code, v) = env.json(&["run", "build", "--detach"]);
    assert_eq!(code, 1, "{v}");
    let text = error_text(&v);
    assert!(
        text.contains("over budget") && text.contains("fake"),
        "{text}"
    );
    let (_, list) = env.json(&["runs", "list"]);
    assert_eq!(
        list["runs"].as_array().unwrap().len(),
        1,
        "no run was recorded"
    );
}

#[test]
fn before_start_modifications_are_applied_and_recorded() {
    let env = Env::new();
    fake_plugin(&env, "  run.before_start:\n");
    write_build(&env);
    let out = env.dir.path().join("var.txt");
    env.set_config(&format!(
        "harnesses:\n  claude: [sh, \"-c\", \"echo $MY_VAR > {}; sleep 600\"]\n",
        out.display()
    ));
    env.start_daemon();
    common::eventually("running", || state_of(&env, "fake") == "running");
    knob(&env, "before_start", "modify");

    let (code, v) = env.json(&["run", "build", "--detach"]);
    assert_eq!(code, 0, "{v}");
    let id = v["id"].as_i64().unwrap().to_string();
    let (_, show) = env.json(&["runs", "show", &id, "--snapshot"]);
    assert!(
        show["run"]["workflow_snapshot"]
            .as_str()
            .unwrap()
            .contains("Branch off dev."),
        "the param was changed"
    );
    let hooks = &show["run"]["placement"]["hooks"][0];
    assert_eq!(hooks["plugin"], "fake");
    assert_eq!(
        hooks["params"]["base"],
        json!({"from": "main", "to": "dev"})
    );
    assert_eq!(hooks["env"], json!(["MY_VAR"]));
    assert!(
        !hooks.to_string().contains("hello"),
        "the record of the change holds env names, not values"
    );
    let human = env.run(&["runs", "show", &id]);
    let human = String::from_utf8_lossy(&human.stdout);
    assert!(human.contains("plugin fake changed this run"), "{human}");
    assert!(human.contains("param base: main -> dev"), "{human}");
    assert!(human.contains("env set: MY_VAR"), "{human}");

    common::eventually("the env to reach the session", || {
        fs::read_to_string(&out).is_ok_and(|s| s.trim() == "hello")
    });
}

#[test]
fn before_start_fails_closed_on_timeout_crash_and_nonsense_unless_on_error_allow() {
    let mut env = Env::new();
    fast_restarts(&mut env);
    fake_plugin(&env, "  run.before_start: {timeout: 1s}\n");
    write_build(&env);
    env.start_daemon();
    common::eventually("running", || state_of(&env, "fake") == "running");

    knob(&env, "before_start", "garbage");
    let (code, v) = env.json(&["run", "build", "--detach"]);
    assert_eq!(code, 1, "{v}");
    assert!(error_text(&v).contains("could not approve"), "{v}");

    knob(&env, "hang_run.before_start", "1");
    let started = std::time::Instant::now();
    let (code, v) = env.json(&["run", "build", "--detach"]);
    assert_eq!(code, 1, "{v}");
    assert!(error_text(&v).contains("didn't answer"), "{v}");
    assert!(started.elapsed() < std::time::Duration::from_secs(8));

    knob(&env, "hang_run.before_start", "");
    knob(&env, "before_start", "crash");
    let (code, v) = env.json(&["run", "build", "--detach"]);
    assert_eq!(code, 1, "{v}");
    common::eventually("running again", || state_of(&env, "fake") == "running");

    // With `on_error: allow` the same failures let the run through.
    let (code, _) = env.json(&["plugin", "rm", "fake"]);
    assert_eq!(code, 0);
    fake_plugin(&env, "  run.before_start: {timeout: 1s, on_error: allow}\n");
    common::eventually("running", || state_of(&env, "fake") == "running");
    knob(&env, "before_start", "garbage");
    let (code, v) = env.json(&["run", "build", "--detach"]);
    assert_eq!(code, 0, "{v}");
}

#[test]
fn enabling_and_disabling_a_plugin_reaches_the_running_daemon() {
    let env = Env::new();
    fake_plugin(&env, "  run.finished:\n");
    env.start_daemon();
    common::eventually("running", || state_of(&env, "fake") == "running");
    env.json(&["plugin", "disable", "fake"]);
    assert_eq!(state_of(&env, "fake"), "disabled");
    let (_, st) = (0, env.rpc_ok("plugin.status", json!({})));
    assert_eq!(st["plugins"], json!([]), "a disabled plugin isn't running");
    env.json(&["plugin", "enable", "fake"]);
    common::eventually("running again", || state_of(&env, "fake") == "running");
}

#[test]
fn stopping_the_daemon_stops_its_plugins() {
    let env = Env::new();
    fake_plugin(&env, "  daemon.stopping:\n");
    env.start_daemon();
    common::eventually("running", || state_of(&env, "fake") == "running");
    let (code, _) = env.json(&["daemon", "stop"]);
    assert_eq!(code, 0);
    assert!(called(&env, "daemon.stopping").is_some(), "it was told");
}
