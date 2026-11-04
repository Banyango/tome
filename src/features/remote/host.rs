//! Node side: sessions this daemon hosts for workers of runs owned by other
//! daemons, and the relay that carries their tome calls back up the link.
//!
//! The run's daemon polls (`hosted.poll`); each poll picks up the calls the
//! hosted workers made and the workers whose sessions ended, and hands back
//! the replies to earlier calls. Nothing here owns a worker's status.

use super::{session_name, tag, LOST_AFTER, RELAY_TIMEOUT};
use crate::config;
use crate::engine::Engine;
use crate::features::plugins::harnesses;
use crate::harness::Vars;
use crate::ids::RunId;
use crate::orchestrator;
use crate::output::{CliError, CliResult, ErrorKind};
use crate::params::{opt_str, req_str};
use crate::paths;
use crate::placement::{Inputs, Role, Settings};
use crate::placementcmd;
use crate::rpc::Response;
use crate::session::{self, Backend, Kind, Launch, Split, Target};
use crate::store::{self, HostedStatus, HostedWorker, NewHosted};
use crate::workers::{self, SUMMARY_LINES};
use crate::worktree;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// The run daemons polling this one, by origin.
#[derive(Default)]
pub struct Host {
    upstreams: Mutex<HashMap<String, Upstream>>,
    wake: Condvar,
}

struct Upstream {
    /// Calls waiting to be picked up.
    outbox: VecDeque<Value>,
    /// Callers waiting for their reply, by relay id.
    waiting: HashMap<u64, SyncSender<Value>>,
    next: u64,
    polled: Instant,
    /// The connection it polls on.
    conn: u64,
}

impl Host {
    /// Send a hosted worker's call up to its run's daemon and wait for the
    /// reply.
    pub fn relay(
        &self,
        origin: &str,
        method: &str,
        params: &Value,
        caller: &Value,
    ) -> CliResult<Value> {
        let (tx, rx) = mpsc::sync_channel(1);
        {
            let mut ups = self.upstreams.lock().unwrap_or_else(|p| p.into_inner());
            let up = ups.get_mut(origin).ok_or_else(upstream_gone)?;
            let id = up.next;
            up.next += 1;
            up.outbox.push_back(
                json!({ "id": id, "method": method, "params": params, "caller": caller }),
            );
            up.waiting.insert(id, tx);
        }
        self.wake.notify_all();
        let reply = rx
            .recv_timeout(RELAY_TIMEOUT)
            .map_err(|_| upstream_gone())?;
        let resp: Response = serde_json::from_value(reply)
            .map_err(|e| CliError::internal(format!("malformed relayed reply: {e}")))?;
        match (resp.result, resp.error) {
            (_, Some(err)) => Err(err.into_cli_error()),
            (Some(result), None) => Ok(result),
            (None, None) => Ok(Value::Null),
        }
    }

    /// Deliver `replies`, then wait up to `wait` for calls and take them.
    fn poll(&self, origin: &str, conn: u64, wait: Duration, replies: &[Value]) -> Vec<Value> {
        let mut ups = self.upstreams.lock().unwrap_or_else(|p| p.into_inner());
        let up = ups.entry(origin.to_string()).or_insert_with(|| Upstream {
            outbox: VecDeque::new(),
            waiting: HashMap::new(),
            next: 1,
            polled: Instant::now(),
            conn,
        });
        up.polled = Instant::now();
        up.conn = conn;
        for reply in replies {
            if let Some(tx) = reply["id"].as_u64().and_then(|id| up.waiting.remove(&id)) {
                let _ = tx.try_send(reply.clone());
            }
        }
        if up.outbox.is_empty() {
            let (guard, _) = self
                .wake
                .wait_timeout_while(ups, wait, |ups| {
                    ups.get(origin).is_some_and(|u| u.outbox.is_empty())
                })
                .unwrap_or_else(|p| p.into_inner());
            ups = guard;
        }
        ups.get_mut(origin)
            .map(|u| u.outbox.drain(..).collect())
            .unwrap_or_default()
    }

    /// Forget upstreams that polled on `conn` (now closed) or haven't polled
    /// for [`LOST_AFTER`]; their waiting callers get an error. Returns them.
    fn drop_lost(&self, conn: Option<u64>) -> Vec<String> {
        let mut ups = self.upstreams.lock().unwrap_or_else(|p| p.into_inner());
        let lost: Vec<String> = ups
            .iter()
            .filter(|(_, u)| match conn {
                Some(c) => u.conn == c,
                None => u.polled.elapsed() > LOST_AFTER,
            })
            .map(|(o, _)| o.clone())
            .collect();
        for o in &lost {
            ups.remove(o);
        }
        lost
    }
}

fn upstream_gone() -> CliError {
    CliError::new(
        ErrorKind::Unreachable,
        "lost the link to the daemon that owns this worker's run",
    )
}

/// Where a hosted worker's launcher, log and exit code go.
fn hosted_dir(origin: &str, run_id: i64, name: &str) -> PathBuf {
    paths::tome_home()
        .join("hosted")
        .join(tag(origin))
        .join(format!("{run_id}-{name}"))
}

fn session_of(h: &HostedWorker) -> Option<store::Session> {
    serde_json::from_value(h.session.clone()?).ok()
}

/// Check out `branch` (just pushed by the run's daemon) in a worktree of
/// `repo` for a hosted worker. One left by an earlier spawn is replaced.
fn add_worktree(
    repo: &Path,
    origin: &str,
    run_id: i64,
    name: &str,
    branch: &str,
) -> CliResult<PathBuf> {
    let path = repo
        .join(".tome/worktrees")
        .join(format!("hosted-{}-{run_id}-{name}", tag(origin)));
    worktree::ensure_ignored(repo)?;
    if path.exists() {
        let _ = worktree::git(
            repo,
            &["worktree", "remove", "--force", &path.to_string_lossy()],
        );
    }
    worktree::git(
        repo,
        &[
            "worktree",
            "add",
            "--quiet",
            &path.to_string_lossy(),
            branch,
        ],
    )
    .map_err(|e| {
        CliError::invalid(format!(
            "couldn't add a worktree for {branch} in {}: {e}",
            repo.display()
        ))
    })?;
    Ok(path)
}

/// `tome gc` on a node: drop hosted workers that ended before `cutoff`,
/// with their files, worktrees and branches (fetched back by then, or named
/// in the run's notes). One whose session is still open (kept open) stays.
pub fn gc_hosted(
    store: &mut store::Store,
    cutoff: chrono::NaiveDateTime,
    dry_run: bool,
) -> CliResult<Vec<Value>> {
    let mut out = Vec::new();
    for h in store.hosted_ended_before(cutoff)? {
        let open = h
            .session
            .clone()
            .and_then(|s| serde_json::from_value::<store::Session>(s).ok())
            .is_some_and(|s| session::is_alive(&s) == Some(true));
        if h.status.is_live() || open {
            continue;
        }
        out.push(json!({
            "origin": h.origin, "run_id": h.run_id, "name": h.name,
            "worktree": h.worktree, "branch": h.branch,
        }));
        if dry_run {
            continue;
        }
        if let (Some(repo), Some(wt)) = (&h.repo, &h.worktree) {
            let repo = Path::new(repo);
            let _ = worktree::git(repo, &["worktree", "remove", "--force", wt]);
            if let Some(branch) = &h.branch {
                let _ = worktree::git(repo, &["branch", "-D", branch]);
            }
        }
        let _ = fs::remove_dir_all(&h.dir);
        store.delete_hosted(&h.origin, h.run_id, &h.name)?;
    }
    Ok(out)
}

impl Engine {
    /// `hosted.spawn {origin, run_id, name, workflow, command, keep_open,
    /// project, placement?}`: start a worker's session here for a run on
    /// `origin`'s daemon.
    pub(crate) fn hosted_spawn(&self, p: &Value) -> CliResult<Value> {
        let origin = req_str(p, "origin")?;
        let run_id = p["run_id"]
            .as_i64()
            .ok_or_else(|| CliError::invalid("missing `run_id`"))?;
        let name = req_str(p, "name")?;
        store::check_name("worker", name)?;
        let workflow = opt_str(p, "workflow").unwrap_or("run");
        let keep_open = p["keep_open"].as_bool().unwrap_or(false);
        let project = opt_str(p, "project").map(PathBuf::from);
        let agent = &p["agent"];
        // Check the harness before touching the repo.
        let harness = match agent["harness"].as_str() {
            Some(h) => {
                let harness = harnesses::resolve(h, project.as_deref()).map_err(|e| {
                    let hint = format!(
                        "configure harness `{h}` on this node under `harnesses:` in {}",
                        config::global_path().display()
                    );
                    e.with_hint(hint)
                })?;
                harness.check_model(agent["model"].as_str())?;
                Some(harness)
            }
            None => None,
        };
        let branch = p["worktree"]["branch"].as_str();
        let worktree = match (branch, &project) {
            (Some(branch), Some(repo)) => Some(add_worktree(repo, origin, run_id, name, branch)?),
            (Some(_), None) => return Err(CliError::invalid("a worktree needs a project")),
            _ => None,
        };
        let cwd = worktree
            .clone()
            .or_else(|| opt_str(p, "cwd").map(PathBuf::from))
            .or_else(|| project.clone())
            .unwrap_or_else(paths::user_home);
        let dir = hosted_dir(origin, run_id, name);
        fs::create_dir_all(&dir)?;
        let argv: Vec<String> = match &harness {
            Some(harness) => {
                let prompt = agent["prompt"].as_str().unwrap_or_default();
                let prompt_file = dir.join("prompt.md");
                fs::write(&prompt_file, prompt)?;
                harness.command(&Vars {
                    prompt,
                    prompt_file: &prompt_file.to_string_lossy(),
                    run_id: RunId::new(run_id),
                    session: &session_name(origin, run_id, name),
                    cwd: &cwd.to_string_lossy(),
                    model: agent["model"].as_str(),
                })
            }
            None => p["command"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect(),
        };
        if argv.is_empty() {
            return Err(CliError::invalid(
                "a hosted worker needs a command or an agent",
            ));
        }
        let flags = p
            .get("placement")
            .filter(|v| !v.is_null())
            .map(Settings::from_json)
            .transpose()?;
        let exit = dir.join("exit");
        let _ = fs::remove_file(&exit);
        let dir_str = dir.to_string_lossy().into_owned();
        self.with_store(|store| {
            store.add_hosted(&NewHosted {
                origin,
                run_id,
                name,
                kind: if harness.is_some() {
                    "agent"
                } else {
                    "command"
                },
                keep_open,
                dir: &dir_str,
                repo: project.as_deref().and_then(Path::to_str),
                worktree: worktree.as_deref().and_then(Path::to_str),
                branch,
            })
        })?;
        let launched = self.launch_hosted(
            origin,
            run_id,
            name,
            workflow,
            &argv,
            &cwd,
            &dir,
            keep_open,
            project.as_deref(),
            flags.as_ref(),
        );
        let s = match launched {
            Ok(s) => s,
            Err(e) => {
                let _ = self.with_store(|store| store.delete_hosted(origin, run_id, name));
                return Err(e);
            }
        };
        self.with_store(|store| store.set_hosted_session(origin, run_id, name, &json!(s)))?;
        Ok(json!({ "session": s.name, "backend": s.backend, "worktree": worktree }))
    }

    #[allow(clippy::too_many_arguments)]
    fn launch_hosted(
        &self,
        origin: &str,
        run_id: i64,
        name: &str,
        workflow: &str,
        argv: &[String],
        cwd: &Path,
        dir: &Path,
        keep_open: bool,
        project: Option<&Path>,
        flags: Option<&Settings>,
    ) -> CliResult<store::Session> {
        let kind = Kind::choose(None, project)?;
        let placement = placementcmd::resolve(
            &Inputs {
                role: Role::Worker(name),
                flags,
                run_flags: None,
                spec: None,
                split_workers: kind == Kind::Tmux,
            },
            project,
        )?;
        let split = Split::new(placement.direction, placement.size, None, &[]);
        let mut env = orchestrator::session_env(RunId::new(run_id));
        env.push(("TOME_WORKER_ID".to_string(), name.to_string()));
        env.push((super::HOSTED_ENV.to_string(), origin.to_string()));
        let (s, _) = Backend::new(kind).launch(&Launch {
            name: &session_name(origin, run_id, name),
            title: &format!("tome: {workflow} #{run_id} / {name}"),
            cwd,
            argv: &workers::wrap(argv, &dir.join("exit"), keep_open),
            env: &env,
            script: &dir.join("worker.sh"),
            log: &dir.join("worker.log"),
            layout: placement.layout,
            split: &split,
            target: &Target::Project,
            project,
            workspace_label: None,
        })?;
        Ok(store::Session {
            role: workers::ROLE.to_string(),
            layout: Some(placement.layout.as_str().to_string()),
            placement: Some(placement.to_json()),
            ..s
        })
    }

    /// `hosted.poll {origin, wait_ms?, replies?}` → `{calls, ended}`.
    pub(crate) fn hosted_poll(&self, p: &Value, conn: u64) -> CliResult<Value> {
        let origin = req_str(p, "origin")?;
        let wait = Duration::from_millis(p["wait_ms"].as_u64().unwrap_or(0).min(5_000));
        let replies = p["replies"].as_array().cloned().unwrap_or_default();
        let calls = self.host.poll(origin, conn, wait, &replies);
        let ended = self.with_store(|store| {
            let mut ended = Vec::new();
            for h in store.hosted_workers(Some(origin))? {
                if h.status == HostedStatus::Ended {
                    store.set_hosted_status(
                        origin,
                        h.run_id,
                        &h.name,
                        HostedStatus::Reported,
                        None,
                        None,
                    )?;
                    ended.push(json!({
                        "run_id": h.run_id,
                        "name": h.name,
                        "exit_code": h.exit_code,
                        "tail": h.summary,
                    }));
                }
            }
            Ok(ended)
        })?;
        Ok(json!({ "calls": calls, "ended": ended }))
    }

    /// `hosted.close {origin, run_id, name?, kill}`: the run's daemon is done
    /// with a hosted worker (or all of a run's); `kill` closes its session.
    /// `hosted.attach {origin, run_id, name}` → `{name, backend, command}`:
    /// how to attach to a hosted worker's live session on this machine.
    pub(crate) fn hosted_attach(&self, p: &Value) -> CliResult<Value> {
        let origin = req_str(p, "origin")?;
        let run_id = p["run_id"]
            .as_i64()
            .ok_or_else(|| CliError::invalid("missing `run_id`"))?;
        let name = req_str(p, "name")?;
        let h = self
            .with_store(|store| store.hosted(origin, run_id, name))?
            .ok_or_else(|| {
                CliError::not_found(format!("no hosted worker {name} of run {run_id}"))
            })?;
        let s = h
            .session
            .and_then(|s| serde_json::from_value::<store::Session>(s).ok())
            .filter(|s| session::is_alive(s) != Some(false))
            .ok_or_else(|| CliError::invalid(format!("worker {name}'s session isn't running")))?;
        Ok(json!({
            "name": s.name,
            "backend": s.backend,
            "command": session::attach_command(&s),
        }))
    }

    pub(crate) fn hosted_close(&self, p: &Value) -> CliResult<Value> {
        let origin = req_str(p, "origin")?;
        let run_id = p["run_id"].as_i64();
        let name = opt_str(p, "name");
        let kill = p["kill"].as_bool().unwrap_or(true);
        let closed = self.with_store(|store| {
            let mut closed = Vec::new();
            for h in store.hosted_workers(Some(origin))? {
                if run_id.is_some_and(|r| r != h.run_id) || name.is_some_and(|n| n != h.name) {
                    continue;
                }
                if h.status.is_live() {
                    store.set_hosted_status(
                        origin,
                        h.run_id,
                        &h.name,
                        HostedStatus::Closed,
                        None,
                        None,
                    )?;
                }
                closed.push(h);
            }
            Ok(closed)
        })?;
        if kill {
            let sessions: Vec<store::Session> = closed.iter().filter_map(session_of).collect();
            // Let a worker that just reported get its reply first.
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(1));
                for s in &sessions {
                    session::kill(s);
                }
            });
        }
        Ok(json!({ "closed": closed.len() }))
    }

    /// Mark hosted workers whose sessions are over as ended, and close the
    /// ones whose run's daemon is gone. Called by the monitor.
    pub(crate) fn check_hosted(&self) {
        for origin in self.host.drop_lost(None) {
            self.upstream_lost(&origin);
        }
        let Ok(hosted) = self.with_store(|store| store.hosted_workers(None)) else {
            return;
        };
        for h in hosted.iter().filter(|h| h.status == HostedStatus::Running) {
            let dir = Path::new(&h.dir);
            let exit = fs::read_to_string(dir.join("exit"))
                .ok()
                .and_then(|s| s.trim().parse::<i32>().ok());
            if exit.is_none() {
                match session_of(h) {
                    Some(s) if session::is_alive(&s) != Some(false) => continue,
                    None => continue,
                    _ => {}
                }
            }
            let exit = exit.or_else(|| {
                fs::read_to_string(dir.join("exit"))
                    .ok()
                    .and_then(|s| s.trim().parse().ok())
            });
            let tail = store::read_tail(&dir.join("worker.log"), SUMMARY_LINES);
            let _ = self.with_store(|store| {
                store.set_hosted_status(
                    &h.origin,
                    h.run_id,
                    &h.name,
                    HostedStatus::Ended,
                    exit,
                    Some(&tail),
                )
            });
        }
    }

    /// A connection closed: upstreams that polled on it are gone.
    pub(crate) fn hosted_conn_closed(&self, conn: u64) {
        for origin in self.host.drop_lost(Some(conn)) {
            self.upstream_lost(&origin);
        }
    }

    /// The run's daemon is gone: its running workers here are orphaned, and
    /// their sessions closed unless they were to be kept open.
    fn upstream_lost(&self, origin: &str) {
        let Ok(hosted) = self.with_store(|store| store.hosted_workers(Some(origin))) else {
            return;
        };
        for h in hosted.iter().filter(|h| h.status.is_live()) {
            eprintln!(
                "tome daemon: lost the link for hosted worker {} of run {}",
                h.name, h.run_id
            );
            let _ = self.with_store(|store| {
                store.set_hosted_status(
                    origin,
                    h.run_id,
                    &h.name,
                    HostedStatus::Orphaned,
                    None,
                    None,
                )
            });
            if !h.keep_open {
                if let Some(s) = session_of(h) {
                    session::kill(&s);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relayed_call_gets_its_reply() {
        let host = std::sync::Arc::new(Host::default());
        assert!(host.poll("o", 1, Duration::ZERO, &[]).is_empty());
        let relay = std::thread::spawn({
            let host = host.clone();
            move || host.relay("o", "worker.report", &json!({}), &json!({}))
        });
        let calls = host.poll("o", 1, Duration::from_secs(5), &[]);
        assert_eq!(calls.len(), 1);
        let id = calls[0]["id"].clone();
        host.poll(
            "o",
            1,
            Duration::ZERO,
            &[json!(Response::ok(id, json!({"ok": 1})))],
        );
        assert_eq!(relay.join().unwrap().unwrap(), json!({"ok": 1}));
    }

    #[test]
    fn a_call_without_an_upstream_is_unreachable() {
        let host = Host::default();
        let err = host
            .relay("o", "worker.report", &json!({}), &json!({}))
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Unreachable);
        host.poll("o", 7, Duration::ZERO, &[]);
        assert_eq!(host.drop_lost(Some(7)), vec!["o".to_string()]);
    }
}
