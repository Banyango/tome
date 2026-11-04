//! Spawning a worker on a node: the run's side of `worker.spawn --node`.
//!
//! In a git project, the worker gets a worktree on the node: the run's base
//! commit is pushed into the node's copy of the project as the worker's
//! branch, and the branch is fetched back when the worker ends.

use crate::engine::Engine;
use crate::features::nodes::{self, Node};
use crate::ids::RunId;
use crate::orchestrator;
use crate::output::{CliError, CliResult, ErrorKind};
use crate::params::internal;
use crate::store::{Run, Worker};
use crate::worktree::{self, Base};
use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;

/// What a remote worker runs.
pub struct RemoteTask<'a> {
    pub name: &'a str,
    pub work: RemoteWork<'a>,
    pub keep_open: bool,
    /// The placement flags it was spawned with.
    pub placement: Option<&'a Value>,
    /// `--base` and `--branch`, for its worktree.
    pub base: Option<&'a str>,
    pub branch: Option<&'a str>,
}

/// A command, or an agent the node starts with its own harness config.
pub enum RemoteWork<'a> {
    Command(&'a [String]),
    Agent {
        harness: &'a str,
        model: Option<&'a str>,
        /// The full prompt, preamble included.
        prompt: &'a str,
    },
}

/// What a remote worker was started with.
pub struct Launched {
    pub session: String,
    /// `<node>:<path>` of its worktree, its branch and base.
    pub worktree: Option<(String, String, String)>,
}

/// Git over the node's SSH.
fn git_ssh(repo: &Path, args: &[&str]) -> Result<(), String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env(
            "GIT_SSH_COMMAND",
            format!("{} -o BatchMode=yes", nodes::ssh_bin()),
        )
        .output()
        .map_err(|e| format!("running git: {e}"))?;
    match out.status.success() {
        true => Ok(()),
        false => Err(String::from_utf8_lossy(&out.stderr).trim().to_string()),
    }
}

impl Engine {
    /// The node `name` names, connected and able to host workers.
    pub(crate) fn remote_node(&self, name: &str) -> CliResult<Node> {
        if self.plugins.provider(name).is_some() {
            return Err(CliError::invalid(format!(
                "`{name}` is a provider, not a node: remote workers need a configured node"
            ))
            .with_hint("add one with `tome node add <name> <ssh-destination>`"));
        }
        let node = nodes::get(name)?;
        match self.open_link(&node) {
            Ok(()) => Ok(node),
            // Say what `tome node check` would: the first failing step's fix.
            Err(e) if e.kind == ErrorKind::Unreachable => {
                let first = nodes::cmd::check_steps(&node, None)
                    .into_iter()
                    .find_map(|s| s.error);
                Err(first
                    .map(|f| {
                        let msg = format!("node {} can't take workers: {}", node.name, f.message);
                        let hint = f.hint.unwrap_or_else(|| node.check_hint());
                        CliError::new(ErrorKind::Unreachable, msg).with_hint(hint)
                    })
                    .unwrap_or(e))
            }
            Err(e) => Err(e),
        }
    }

    /// Start a worker's session on `node`, in a worktree when the run's
    /// project is a git repo.
    pub(crate) fn launch_remote(
        &self,
        run: &Run,
        node: &Node,
        task: &RemoteTask,
    ) -> CliResult<Launched> {
        let local = orchestrator::run_project(run);
        let base = match &local {
            Some(local) if worktree::git(local, &["rev-parse", "--git-dir"]).is_ok() => {
                Some(worktree::resolve_base(local, task.base)?)
            }
            _ if task.base.is_some() || task.branch.is_some() => {
                return Err(CliError::invalid(
                    "--base and --branch need the run's project to be a git repo",
                ))
            }
            _ => None,
        };
        let project = match &local {
            Some(local) => {
                let (n, l, git) = (node.clone(), local.clone(), base.is_some());
                let located = self.with_node(node, move |c| nodes::locate(c, &n, &l, git))?;
                if git && located.git.is_none() {
                    return Err(not_a_repo(node, local, &located.path));
                }
                Some(located.path)
            }
            None => None,
        };
        let mut worktree = None;
        if let (Some(base), Some(path)) = (&base, &project) {
            let branch = match task.branch {
                Some(b) => worktree::check_branch(&base.repo, b)?,
                None => format!("tome/{}/{}", run.id, task.name),
            };
            self.push_base(run.id, node, base, path, &branch, task.name)?;
            worktree = Some(json!({ "branch": branch }));
        }
        let out = self.node_call(
            node,
            "hosted.spawn",
            json!({
                "origin": self.links.origin(),
                "run_id": run.id.get(),
                "name": task.name,
                "workflow": run.workflow_name,
                "command": match &task.work {
                    RemoteWork::Command(cmd) => json!(cmd),
                    RemoteWork::Agent { .. } => Value::Null,
                },
                "agent": match &task.work {
                    RemoteWork::Agent { harness, model, prompt } => {
                        json!({ "harness": harness, "model": model, "prompt": prompt })
                    }
                    RemoteWork::Command(_) => Value::Null,
                },
                "keep_open": task.keep_open,
                "project": project,
                "worktree": worktree,
                "placement": task.placement,
            }),
        )?;
        let session = out["session"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| CliError::internal(format!("{} didn't name the session", node.name)))?;
        let worktree = match (out["worktree"].as_str(), worktree, base) {
            (Some(path), Some(wt), Some(base)) => Some((
                format!("{}:{path}", node.name),
                wt["branch"].as_str().unwrap_or_default().to_string(),
                base.name,
            )),
            _ => None,
        };
        Ok(Launched { session, worktree })
    }

    /// Push the run's base commit into the node's copy as `branch`. Changes
    /// that aren't committed stay here; the run gets a note saying so.
    fn push_base(
        &self,
        run_id: RunId,
        node: &Node,
        base: &Base,
        path: &str,
        branch: &str,
        name: &str,
    ) -> CliResult<()> {
        let dirty = worktree::git(&base.repo, &["status", "--porcelain"])
            .is_ok_and(|s| !s.trim().is_empty());
        if dirty && base.name != base.commit {
            let note = format!(
                "worker {name} on {}: uncommitted changes in {} weren't sent; it starts from {} ({})",
                node.name,
                base.repo.display(),
                base.name,
                &base.commit[..base.commit.len().min(12)]
            );
            self.with_store(|store| store.add_run_note(run_id, &note).map_err(internal))?;
        }
        let refspec = format!("+{}:refs/heads/{branch}", base.commit);
        git_ssh(
            &base.repo,
            &["push", "--quiet", &format!("{}:{path}", node.ssh), &refspec],
        )
        .map_err(|e| {
            CliError::new(
                ErrorKind::Unreachable,
                format!("couldn't push {} to {}: {e}", base.name, node.name),
            )
            .with_hint(node.check_hint())
        })
    }

    /// Fetch a remote worker's branch back into the run's repo, in the
    /// background. If that fails, the run gets a note naming the branch on
    /// the node.
    pub(crate) fn fetch_back(&self, run_id: RunId, w: &Worker) {
        let (Some(node), Some(wt), Some(branch)) = (&w.node, &w.worktree, &w.branch) else {
            return;
        };
        let Ok(run) = self.with_store(|store| store.require_run(run_id)) else {
            return;
        };
        let Some(repo) = orchestrator::run_project(&run) else {
            return;
        };
        let Some(engine) = self.me() else { return };
        let (Ok(n), path) = (
            nodes::get(node),
            wt.split_once(':').map(|(_, p)| p.to_string()),
        ) else {
            return;
        };
        let (name, branch) = (w.name.clone(), branch.clone());
        std::thread::spawn(move || {
            let Some(path) = path else { return };
            let refspec = format!("+refs/heads/{branch}:refs/heads/{branch}");
            let fetched = git_ssh(
                &repo,
                &["fetch", "--quiet", &format!("{}:{path}", n.ssh), &refspec],
            );
            if let Err(e) = fetched {
                let note = format!(
                    "couldn't fetch worker {name}'s branch {branch} back from {}: {e}; it's still there in {path}",
                    n.name
                );
                eprintln!("tome daemon: run {run_id}: {note}");
                let _ =
                    engine.with_store(|store| store.add_run_note(run_id, &note).map_err(internal));
            }
        });
    }
}

fn not_a_repo(node: &Node, local: &Path, path: &str) -> CliError {
    let name = local
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    CliError::not_found(format!(
        "{path} on {} isn't a git repo, so the worker can't get a worktree there",
        node.name
    ))
    .with_hint(format!(
        "clone the project there, or set `nodes.{}.projects.{name}` in ~/.tome/config.yaml to its clone",
        node.name
    ))
}
