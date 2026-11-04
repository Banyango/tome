mod common;

use common::Env;
use serde_json::json;
use std::path::Path;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) {
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

#[test]
fn gc_deletes_old_finished_runs_logs_and_worktrees() {
    let env = Env::new();
    env.start_daemon();
    let path = env.project().join("build.md");
    std::fs::write(&path, "---\nname: build\n---\n## Build\ngo\n").unwrap();
    let create = || {
        env.rpc_ok("run.create", json!({ "workflow_path": path }))["id"]
            .as_i64()
            .unwrap()
    };

    // A real repo with a linked worktree for the finished run.
    let repo = env.dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
    );
    let wt = env.dir.path().join("wt");
    git(
        &repo,
        &["worktree", "add", "-q", "-b", "run-1", wt.to_str().unwrap()],
    );

    let old = create();
    env.rpc_ok(
        "worktree.add",
        json!({ "run_id": old, "path": wt, "repo_path": repo, "branch": "run-1" }),
    );
    env.rpc_ok(
        "step.report",
        json!({ "run_id": old, "step": "Build", "event": "start" }),
    );
    std::fs::write(env.home().join(format!("runs/{old}/Build.log")), "hello\n").unwrap();
    env.rpc_ok("run.finish", json!({ "id": old, "status": "succeeded" }));
    let live = create();

    // Too young: nothing to do.
    let (code, v) = env.json(&["gc", "--older-than", "1h"]);
    assert_eq!(code, 0);
    assert!(v["deleted"].as_array().unwrap().is_empty());

    // Dry run lists without deleting.
    let (_, v) = env.json(&["gc", "--older-than", "0s", "--dry-run"]);
    assert_eq!(v["deleted"][0]["id"], old);
    assert!(wt.is_dir());
    assert_eq!(
        env.json(&["runs", "list"]).1["runs"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    let out = env.run(&["gc", "--older-than", "0s"]);
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains(&format!("deleted run {old} (build, succeeded)"))
            && text.contains("1 run deleted"),
        "{text}"
    );

    assert!(!wt.exists(), "worktree removed");
    assert!(
        !env.home().join(format!("runs/{old}")).exists(),
        "log dir removed"
    );
    let (code, _) = env.json(&["runs", "show", &old.to_string()]);
    assert_eq!(code, 4);
    for table in ["steps", "step_events", "logs", "worktrees"] {
        let (_, v) = env.json(&[
            "query",
            &format!("select count(*) from {table} where run_id = {old}"),
        ]);
        assert_eq!(v["rows"][0][0], 0, "{table}");
    }
    // In-progress runs are never collected.
    let (_, v) = env.json(&["runs", "show", &live.to_string()]);
    assert_eq!(v["run"]["status"], "running");
}

#[test]
fn gc_keeps_runs_whose_worktree_it_cannot_remove() {
    let env = Env::new();
    env.start_daemon();
    let path = env.project().join("build.md");
    std::fs::write(&path, "---\nname: build\n---\ngo\n").unwrap();
    let id = env.rpc_ok("run.create", json!({ "workflow_path": path }))["id"]
        .as_i64()
        .unwrap();
    // A plain directory, not a git worktree: gc must not delete it.
    let dir = env.dir.path().join("plain");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("keep.txt"), "x").unwrap();
    env.rpc_ok("worktree.add", json!({ "run_id": id, "path": dir }));
    env.rpc_ok("run.finish", json!({ "id": id, "status": "failed" }));

    let (code, v) = env.json(&["gc", "--older-than", "0s"]);
    assert_eq!(code, 1, "{v}");
    assert_eq!(v["kept"][0]["id"], id);
    assert!(dir.join("keep.txt").exists());
    assert_eq!(env.json(&["runs", "show", &id.to_string()]).0, 0);
}

#[test]
fn gc_requires_valid_age() {
    let env = Env::new();
    let (code, v) = env.json(&["gc", "--older-than", "soon"]);
    assert_eq!(code, 2);
    assert_eq!(v["error"]["kind"], "invalid");
    assert_eq!(env.run(&["gc"]).status.code(), Some(2));
}

#[test]
fn gc_deletes_merged_worker_branches_and_keeps_unmerged_ones() {
    let env = Env::new();
    env.start_daemon();
    let path = env.project().join("build.md");
    std::fs::write(&path, "---\nname: build\n---\ngo\n").unwrap();
    let id = env.rpc_ok("run.create", json!({ "workflow_path": path }))["id"]
        .as_i64()
        .unwrap();

    let repo = env.dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    let commit = |dir: &Path, msg: &str| {
        git(
            dir,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                msg,
            ],
        )
    };
    commit(&repo, "init");
    // `merged` has nothing main lacks; `unmerged` has a commit of its own.
    for name in ["merged", "unmerged"] {
        let wt = env.dir.path().join(name);
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                &format!("tome/{id}/{name}"),
                wt.to_str().unwrap(),
            ],
        );
        env.rpc_ok(
            "worktree.add",
            json!({ "run_id": id, "path": wt, "repo_path": repo, "branch": format!("tome/{id}/{name}"), "base": "main" }),
        );
    }
    commit(&env.dir.path().join("unmerged"), "work");
    env.rpc_ok("run.finish", json!({ "id": id, "status": "succeeded" }));

    let (_, v) = env.json(&["gc", "--older-than", "0s", "--dry-run"]);
    let branches = &v["deleted"][0]["branches"];
    assert_eq!(branches[0]["deleted"], true, "{v}");
    assert_eq!(branches[1]["deleted"], false, "{v}");

    let out = env.run(&["gc", "--older-than", "0s"]);
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains(&format!(
            "deleted branch tome/{id}/merged (merged into main)"
        )),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "kept branch tome/{id}/unmerged: not merged into main"
        )),
        "{text}"
    );

    let branches = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["branch", "--list"])
        .output()
        .unwrap();
    let branches = String::from_utf8_lossy(&branches.stdout);
    assert!(
        !branches
            .lines()
            .any(|l| l.trim() == format!("tome/{id}/merged")),
        "{branches}"
    );
    assert!(
        branches
            .lines()
            .any(|l| l.trim() == format!("tome/{id}/unmerged")),
        "{branches}"
    );
}

/// A repo on `main` with a `tome/<id>/<name>` worktree per name; `unmerged`
/// gets a commit of its own.
fn repo_with_worktrees(env: &Env, id: i64, names: &[&str]) -> std::path::PathBuf {
    let repo = env.dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    let commit = |dir: &Path| {
        git(
            dir,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "c",
            ],
        )
    };
    commit(&repo);
    for name in names {
        let wt = env.dir.path().join(name);
        let branch = format!("tome/{id}/{name}");
        git(
            &repo,
            &["worktree", "add", "-q", "-b", &branch, wt.to_str().unwrap()],
        );
        env.rpc_ok(
            "worktree.add",
            json!({ "run_id": id, "path": wt, "repo_path": repo, "branch": branch, "base": "main" }),
        );
        if *name == "unmerged" {
            commit(&wt);
        }
    }
    repo
}

fn branches(repo: &Path) -> Vec<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["branch", "--format=%(refname:short)"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn runs_rm_removes_one_run_and_keeps_unmerged_branches() {
    let env = Env::new();
    env.start_daemon();
    let path = env.project().join("build.md");
    std::fs::write(&path, "---\nname: build\n---\ngo\n").unwrap();
    let create = || {
        env.rpc_ok("run.create", json!({ "workflow_path": path }))["id"]
            .as_i64()
            .unwrap()
    };
    let id = create();
    let other = create();
    let repo = repo_with_worktrees(&env, id, &["merged", "unmerged"]);
    std::fs::create_dir_all(env.home().join(format!("runs/{id}"))).unwrap();
    env.rpc_ok("run.finish", json!({ "id": id, "status": "succeeded" }));
    env.rpc_ok("run.finish", json!({ "id": other, "status": "succeeded" }));

    let (code, v) = env.json(&["runs", "rm", &id.to_string(), "--dry-run"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["run"]["branches"][0]["deleted"], true, "{v}");
    assert_eq!(v["run"]["branches"][1]["deleted"], false, "{v}");
    assert!(env.dir.path().join("merged").exists());
    assert_eq!(env.json(&["runs", "show", &id.to_string()]).0, 0);

    let out = env.run(&["runs", "rm", &id.to_string()]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(
        text.contains(&format!("removed run {id} (build, succeeded)"))
            && text.contains(&format!(
                "kept branch tome/{id}/unmerged: not merged into main"
            )),
        "{text}"
    );
    assert!(!env.dir.path().join("merged").exists());
    assert!(!env.dir.path().join("unmerged").exists());
    assert!(!env.home().join(format!("runs/{id}")).exists());
    assert_eq!(env.json(&["runs", "show", &id.to_string()]).0, 4);
    assert_eq!(env.json(&["runs", "show", &other.to_string()]).0, 0);
    let b = branches(&repo);
    assert!(!b.contains(&format!("tome/{id}/merged")), "{b:?}");
    assert!(b.contains(&format!("tome/{id}/unmerged")), "{b:?}");

    // Unknown id.
    let (code, v) = env.json(&["runs", "rm", &id.to_string()]);
    assert_eq!(code, 4, "{v}");
}

#[test]
fn runs_rm_force_deletes_unmerged_branches() {
    let env = Env::new();
    env.start_daemon();
    let path = env.project().join("build.md");
    std::fs::write(&path, "---\nname: build\n---\ngo\n").unwrap();
    let id = env.rpc_ok("run.create", json!({ "workflow_path": path }))["id"]
        .as_i64()
        .unwrap();
    let repo = repo_with_worktrees(&env, id, &["unmerged"]);
    env.rpc_ok("run.finish", json!({ "id": id, "status": "failed" }));

    let (_, v) = env.json(&["runs", "rm", &id.to_string(), "--force", "--dry-run"]);
    assert_eq!(v["run"]["branches"][0]["deleted"], true, "{v}");
    assert!(branches(&repo).contains(&format!("tome/{id}/unmerged")));

    let out = env.run(&["runs", "rm", &id.to_string(), "--force"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(
        text.contains(&format!(
            "deleted branch tome/{id}/unmerged (not merged into main, --force)"
        )),
        "{text}"
    );
    assert!(!branches(&repo).contains(&format!("tome/{id}/unmerged")));
}

#[test]
fn runs_rm_cancels_a_live_run_first() {
    let env = Env::new();
    env.start_daemon();
    let path = env.project().join("build.md");
    std::fs::write(&path, "---\nname: build\n---\ngo\n").unwrap();
    let id = env.rpc_ok("run.create", json!({ "workflow_path": path }))["id"]
        .as_i64()
        .unwrap();

    let out = env.run(&["runs", "rm", &id.to_string(), "--dry-run"]);
    assert!(
        String::from_utf8_lossy(&out.stdout).contains(&format!("would cancel and remove run {id}")),
        "{out:?}"
    );
    let (_, v) = env.json(&["runs", "show", &id.to_string()]);
    assert_eq!(v["run"]["status"], "running");

    let out = env.run(&["runs", "rm", &id.to_string()]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(
        text.contains(&format!(
            "cancelled and removed run {id} (build, cancelled)"
        )),
        "{text}"
    );
    assert_eq!(env.json(&["runs", "show", &id.to_string()]).0, 4);
}

#[test]
fn runs_rm_keeps_a_run_whose_worktree_git_cannot_remove() {
    let env = Env::new();
    env.start_daemon();
    let path = env.project().join("build.md");
    std::fs::write(&path, "---\nname: build\n---\ngo\n").unwrap();
    let id = env.rpc_ok("run.create", json!({ "workflow_path": path }))["id"]
        .as_i64()
        .unwrap();
    let dir = env.dir.path().join("plain");
    std::fs::create_dir_all(&dir).unwrap();
    env.rpc_ok("worktree.add", json!({ "run_id": id, "path": dir }));
    env.rpc_ok("run.finish", json!({ "id": id, "status": "failed" }));

    let (code, v) = env.json(&["runs", "rm", &id.to_string()]);
    assert_eq!(code, 1, "{v}");
    assert!(
        v["error"].as_str().unwrap().contains("not a git worktree"),
        "{v}"
    );
    assert!(dir.exists());
    assert_eq!(env.json(&["runs", "show", &id.to_string()]).0, 0);
}
