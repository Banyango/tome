//! Workers, groups, worktrees and queues, daemon side: the RPCs behind
//! `tome worker|group|worktree|queue`, and the monitor pass that notices
//! workers whose session ended.
//!
//! A worker's session runs its command (or agent) under a small `sh`
//! wrapper that writes the exit code to `worker-<name>.exit` in the run
//! directory, so a command worker's result survives `--keep-open` and a
//! session that's already gone.

use crate::api::{internal, opt_str, req_id_at, req_str};
use crate::engine::Engine;
use crate::handshake::{self, Agent};
use crate::harness::{self, shell_quote, Vars};
use crate::orchestrator;
use crate::output::{CliError, CliResult};
use crate::paths;
use crate::placement::{self, Inputs, Role, Settings};
use crate::session::{self, Backend, Kind, Launch, Split};
use crate::store::{
    self, NewWorker, NewWorktree, Pulled, Run, RunStatus, Store, Worker, WorkerEnd, WorkerStatus,
};
use crate::workflow::Mode;
use crate::worktree;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The built-in worker preamble; the orchestrator's task follows it.
pub const PROMPT: &str = include_str!("worker_prompt.md");

pub const ROLE: &str = "worker";

/// Why a worker failed when its session ended without a report.
pub const EXITED: &str = "worker_exited";
/// Why a worker failed when its session couldn't be started.
pub const LAUNCH_FAILED: &str = "launch_failed";
/// Why a worker failed when its command exited non-zero.
pub const EXIT_CODE: &str = "exit_code";
/// `tome worker kill`.
pub const KILLED: &str = "killed";

/// Lines of a command worker's output kept in its summary.
const SUMMARY_LINES: usize = 10;
/// How long a worker that reported has to get the reply before its session
/// is closed.
const REPORT_GRACE: Duration = Duration::from_secs(1);

pub const METHODS: &[&str] = &[
    "worker.spawn",
    "worker.report",
    "worker.status",
    "worker.kill",
    "group.create",
    "group.close",
    "group.status",
    "worktree.create",
    "queue.push",
    "queue.pull",
    "queue.peek",
    "queue.ack",
    "queue.close",
    "queue.ls",
];

fn opt_bool(p: &Value, key: &str) -> bool {
    p.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// Who's calling: a worker (`TOME_WORKER_ID`, sent as `caller`) or the
/// orchestrator.
fn caller(p: &Value) -> Option<&str> {
    opt_str(p, "caller").filter(|c| !c.is_empty())
}

fn run_dir(run_id: i64) -> PathBuf {
    paths::runs_dir().join(run_id.to_string())
}

fn exit_file(run_id: i64, name: &str) -> PathBuf {
    run_dir(run_id).join(format!("worker-{name}.exit"))
}

/// Where an agent worker's prompt is written.
fn prompt_file(run_id: i64, name: &str) -> PathBuf {
    run_dir(run_id).join(format!("worker-{name}-prompt.md"))
}

fn log_file(run_id: i64, name: &str) -> PathBuf {
    run_dir(run_id).join(format!("worker-{name}.log"))
}

/// The exit code a worker's wrapper recorded, once it has.
fn recorded_exit(run_id: i64, name: &str) -> Option<i32> {
    fs::read_to_string(exit_file(run_id, name))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Run `argv`, record its exit code in `exit`, then (with `keep_open`)
/// leave a shell in the session.
fn wrap(argv: &[String], exit: &Path, keep_open: bool) -> Vec<String> {
    let mut script = format!("\"$@\"; echo $? > {}", shell_quote(&exit.to_string_lossy()));
    if keep_open {
        script.push_str("; exec \"${SHELL:-sh}\"");
    }
    let mut out = vec![
        "sh".to_string(),
        "-c".to_string(),
        script,
        "tome-worker".to_string(),
    ];
    out.extend(argv.iter().cloned());
    out
}

/// The repo a run's worktrees come from: its project, else where the
/// command was run.
fn repo_dir(run: &Run, p: &Value) -> PathBuf {
    run.project_path
        .as_ref()
        .map(PathBuf::from)
        .filter(|d| d.is_dir())
        .or_else(|| opt_str(p, "cwd").map(PathBuf::from))
        .unwrap_or_else(paths::user_home)
}

/// How a spawned worker runs.
enum Task {
    Agent {
        harness: harness::Harness,
        model: Option<String>,
        prompt: String,
    },
    Command(Vec<String>),
}

impl Engine {
    pub(crate) fn dispatch_primitive(&self, method: &str, p: &Value) -> CliResult<Value> {
        if method.starts_with("queue.") {
            return self.with_store(|store| queue(store, method, p));
        }
        let run_id = req_id_at(p, "run_id")?;
        // A single run's agent does the work itself: no workers, groups or
        // worktrees.
        let mode = self.with_store(|store| Ok(store.require_run(run_id)?.mode))?;
        if mode == Mode::Single {
            let what = match method.split_once('.') {
                Some(("worktree", _)) => "`tome worktree create`".to_string(),
                Some((noun, _)) => format!("`tome {noun}`"),
                None => format!("`{method}`"),
            };
            return Err(orchestrator::single_refusal(&what));
        }
        match method {
            "worker.spawn" => self.spawn_worker(run_id, p),
            "worker.report" => self.report_worker(run_id, p),
            "worker.status" => self.with_store(|store| match opt_str(p, "name") {
                Some(name) => {
                    let worker = store.require_worker(run_id, name)?;
                    let session = store
                        .sessions(run_id)?
                        .into_iter()
                        .find(|s| s.name == worker.session.as_deref().unwrap_or(""));
                    let mut value = json!(worker);
                    if let Some(session) = session {
                        if let Some(status) = session.agent_status {
                            value["agent_status"] = json!(status);
                        }
                        if let Some(at) = session.blocked_at {
                            value["blocked_at"] = json!(at);
                        }
                    }
                    Ok(value)
                }
                None => {
                    store.require_run(run_id)?;
                    let mut workers = store.workers(run_id)?;
                    let sessions = store.sessions(run_id)?;
                    let values: Vec<Value> = workers
                        .drain(..)
                        .map(|w| {
                            let mut value = json!(w);
                            if let Some(session) = sessions
                                .iter()
                                .find(|s| s.name == w.session.as_deref().unwrap_or(""))
                            {
                                if let Some(status) = &session.agent_status {
                                    value["agent_status"] = json!(status);
                                }
                                if let Some(at) = &session.blocked_at {
                                    value["blocked_at"] = json!(at);
                                }
                            }
                            value
                        })
                        .collect();
                    Ok(json!({ "workers": values }))
                }
            }),
            "worker.kill" => self.kill_worker(run_id, req_str(p, "name")?),
            "group.create" => self.with_store(|store| {
                let group =
                    store.create_group(run_id, req_str(p, "name")?, opt_bool(p, "fail_fast"))?;
                Ok(json!(group))
            }),
            "group.close" => self.group_status(run_id, req_str(p, "name")?, true, false),
            "group.status" => self.group_status(
                run_id,
                req_str(p, "name")?,
                opt_bool(p, "wait"),
                opt_bool(p, "wait"),
            ),
            "worktree.create" => self.create_worktree(run_id, p),
            _ => Err(CliError::invalid(format!("unknown method `{method}`"))),
        }
    }

    /// `worker.spawn {run_id, name?, group?, worktree?, base?, harness?, model?,
    /// keep_open?, prompt? | command?, placement?, caller?, cwd?}`
    fn spawn_worker(&self, run_id: i64, p: &Value) -> CliResult<Value> {
        if let Some(me) = caller(p) {
            return Err(CliError::invalid(format!(
                "worker `{me}` can't spawn workers; only the orchestrator can"
            ))
            .with_hint("finish your task, or report what else is needed in your summary"));
        }
        let prompt = opt_str(p, "prompt").filter(|s| !s.trim().is_empty());
        let command: Option<Vec<String>> = p
            .get("command")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .filter(|c| !c.is_empty());
        let with_worktree = opt_bool(p, "worktree");
        let base = opt_str(p, "base");
        if base.is_some() && !with_worktree {
            return Err(CliError::invalid("--base only applies with --worktree"));
        }
        let branch = opt_str(p, "branch");
        if branch.is_some() && !with_worktree {
            return Err(CliError::invalid("--branch only applies with --worktree"));
        }
        let run = self.with_store(|store| {
            store
                .get_run(run_id, true)
                .map_err(internal)?
                .ok_or_else(|| CliError::not_found(format!("run {run_id} not found")))
        })?;
        if run.status != RunStatus::Running {
            return Err(CliError::invalid(format!(
                "run {run_id} isn't running ({})",
                run.status.as_str()
            )));
        }
        let wf = orchestrator::snapshot(&run)?;
        let task = match (prompt, command) {
            (Some(prompt), None) => {
                let project = orchestrator::run_project(&run);
                // Without an explicit choice, workers run the harness their
                // orchestrator runs.
                let harness =
                    match opt_str(p, "harness").or(wf.frontmatter().defaults.harness.as_deref()) {
                        Some(name) => harness::resolve(name, project.as_deref())?,
                        None => orchestrator::harness_for(
                            &wf.frontmatter(),
                            wf.frontmatter().mode,
                            project.as_deref(),
                        )?,
                    };
                let model = opt_str(p, "model")
                    .or(wf.frontmatter().defaults.model.as_deref())
                    .map(str::to_string);
                harness.check_model(model.as_deref())?;
                Task::Agent {
                    harness,
                    model,
                    prompt: prompt.to_string(),
                }
            }
            (None, Some(cmd)) => {
                for flag in ["harness", "model"] {
                    if opt_str(p, flag).is_some() {
                        return Err(CliError::invalid(format!(
                            "--{flag} only applies to agent workers (--prompt)"
                        )));
                    }
                }
                Task::Command(cmd)
            }
            _ => return Err(CliError::invalid(
                "give a worker either a task (--prompt/--prompt-file) or a command (after `--`)",
            )
            .with_hint(
                "e.g. `tome worker spawn --prompt \"...\"` or `tome worker spawn -- cargo test`",
            )),
        };
        let flags = p
            .get("placement")
            .filter(|v| !v.is_null())
            .map(Settings::from_json)
            .transpose()?;
        if let Some(flags) = &flags {
            if flags.from == Some(placement::From::Caller) {
                return Err(CliError::invalid(format!(
                    "--from: {}",
                    placement::CALLER_IS_FOR_THE_ORCHESTRATOR
                ))
                .with_hint("use --from orchestrator, last or first"));
            }
            placement::check_flag_preset(
                flags,
                "`tome worker spawn` flags",
                orchestrator::run_project(&run).as_deref(),
            )?;
        }
        // Resolve the base before recording anything, so a bad one spawns nothing.
        let base = if with_worktree {
            Some(worktree::resolve_base(&repo_dir(&run, p), base)?)
        } else {
            None
        };

        let (kind, harness_name, cmd) = match &task {
            Task::Agent { harness, .. } => ("agent", Some(harness.name.as_str()), None),
            Task::Command(cmd) => ("command", None, Some(cmd.as_slice())),
        };
        let worker = self.with_store(|store| {
            store.reserve_worker(
                run_id,
                &NewWorker {
                    name: opt_str(p, "name"),
                    kind,
                    group: opt_str(p, "group"),
                    harness: harness_name,
                    command: cmd,
                    keep_open: opt_bool(p, "keep_open"),
                },
            )
        })?;
        let name = worker.name.clone();

        let created = match &base {
            Some(base) => match worktree::create(base, run_id, &name, branch) {
                Ok(c) => Some(c),
                Err(e) => {
                    let _ = self.with_store(|store| store.delete_worker(run_id, &name));
                    return Err(e);
                }
            },
            None => None,
        };
        self.with_store(|store| {
            if let (Some(base), Some(c)) = (&base, &created) {
                store
                    .add_worktree(
                        run_id,
                        &NewWorktree {
                            path: &c.path,
                            repo_path: Some(&base.repo),
                            branch: Some(&c.branch),
                            base: Some(&base.name),
                            worker: Some(&name),
                        },
                    )
                    .map_err(internal)?;
                store.set_worker_worktree(
                    run_id,
                    &name,
                    &c.path.to_string_lossy(),
                    &c.branch,
                    &base.name,
                )?;
            }
            store.worker_event(
                run_id,
                Some(&name),
                worker.group.as_deref(),
                "spawned",
                Some(kind),
            )?;
            self.sync(store, run_id);
            Ok(())
        })?;

        let cwd = created
            .as_ref()
            .map(|c| c.path.clone())
            .unwrap_or_else(|| orchestrator::run_cwd(&run));
        // Agent workers must show they've started; command workers report
        // by exit code.
        let agent: Agent = (run_id, Some(name.clone()));
        if matches!(task, Task::Agent { .. }) {
            self.expect_start(
                agent.clone(),
                handshake::timeout(&wf.frontmatter()),
                prompt_file(run_id, &name),
            );
        }
        match self.launch_worker(
            &run,
            &name,
            &task,
            &cwd,
            created.as_ref().map(|c| c.path.as_path()),
            worker.keep_open,
            flags.as_ref(),
        ) {
            Ok((s, note)) => {
                let started = self.with_store(|store| {
                    store.add_session(&s).map_err(internal)?;
                    if let Some(note) = &note {
                        store.add_run_note(run_id, note).map_err(internal)?;
                    }
                    let started = store.start_worker(run_id, &name, &s.name)?;
                    self.sync(store, run_id);
                    Ok(started)
                });
                // Ended while it was starting (e.g. the run was cancelled).
                if !started.unwrap_or(false) {
                    session::kill(&s);
                }
            }
            Err(e) => {
                self.forget_start(&agent);
                let end = self.with_store(|store| {
                    let end = store.finish_worker(
                        run_id,
                        &name,
                        WorkerStatus::Failed,
                        Some(LAUNCH_FAILED),
                        Some(&e.message),
                        None,
                        true,
                    );
                    self.sync(store, run_id);
                    end
                });
                if let Ok(end) = end {
                    self.after_end(run_id, &end, None);
                }
                return Err(e);
            }
        }
        self.with_store(|store| Ok(json!(store.require_worker(run_id, &name)?)))
    }

    /// Start a worker's session on the run's backend (where its orchestrator
    /// is). Its launcher, prompt, output and exit code go in the run's
    /// directory.
    fn launch_worker(
        &self,
        run: &Run,
        name: &str,
        task: &Task,
        cwd: &Path,
        worktree: Option<&Path>,
        keep_open: bool,
        flags: Option<&Settings>,
    ) -> CliResult<(store::Session, Option<String>)> {
        let recorded = self.recorded_sessions(run.id);
        let orch = recorded.iter().find(|s| orchestrator::is_main(&s.role));
        let wf = orchestrator::snapshot(run)?;
        let project = orchestrator::run_project(run);
        let kind = match orch.and_then(|s| Kind::parse(&s.backend)) {
            Some(kind) => kind,
            None => Kind::choose(
                wf.frontmatter().defaults.backend.as_deref(),
                project.as_deref(),
            )?,
        };
        let run_flags = orchestrator::run_flags(run)?;
        let inputs = Inputs {
            role: Role::Worker(name),
            flags,
            run_flags: run_flags.as_ref(),
            spec: wf.frontmatter().defaults.layout.as_ref(),
        };
        let mut placement = placement::resolve(&inputs, project.as_deref())?;
        let layout = placement.layout;
        let split = Split::new(
            placement.direction,
            placement.size,
            placement.from,
            &recorded,
        );
        let (target, note) = orchestrator::target(run, &placement);
        let dir = run_dir(run.id);
        fs::create_dir_all(&dir)?;
        let exit = exit_file(run.id, name);
        let _ = fs::remove_file(&exit);
        let session_name = session::run_session_name(run.id, &run.workflow_name, name);
        let (argv, harness) = match task {
            Task::Agent {
                harness,
                model,
                prompt,
            } => {
                let full = format!("{}\n\n{}\n", PROMPT.trim_end(), prompt.trim());
                let prompt_file = prompt_file(run.id, name);
                fs::write(&prompt_file, &full)?;
                let argv = harness.command(&Vars {
                    prompt: &full,
                    prompt_file: &prompt_file.to_string_lossy(),
                    run_id: run.id,
                    session: &session_name,
                    cwd: &cwd.to_string_lossy(),
                    model: model.as_deref(),
                });
                (argv, Some(harness.name.clone()))
            }
            Task::Command(cmd) => (cmd.clone(), None),
        };
        let mut env = orchestrator::session_env(run.id);
        env.push(("TOME_WORKER_ID".to_string(), name.to_string()));
        if let Some(wt) = worktree {
            env.push((
                "TOME_WORKTREE".to_string(),
                wt.to_string_lossy().into_owned(),
            ));
        }
        let (s, warnings) = Backend::new(kind).launch(&Launch {
            name: &session_name,
            title: &format!("tome: {} #{} / {name}", run.workflow_name, run.id),
            cwd,
            argv: &wrap(&argv, &exit, keep_open),
            env: &env,
            script: &dir.join(format!("worker-{name}.sh")),
            log: &log_file(run.id, name),
            layout,
            split: &split,
            target: &target,
            project: orchestrator::run_project(run).as_deref(),
        })?;
        placement.warnings = warnings;
        let session = store::Session {
            run_id: run.id,
            role: ROLE.to_string(),
            layout: Some(layout.as_str().to_string()),
            harness,
            placement: Some(placement.to_json()),
            ..s
        };
        Ok((session, note))
    }

    /// `worker.report {run_id, name, event: done|fail, summary?}`
    fn report_worker(&self, run_id: i64, p: &Value) -> CliResult<Value> {
        let name = opt_str(p, "name").or(caller(p)).ok_or_else(|| {
            CliError::invalid(
                "which worker? `tome worker done|fail` is for workers (TOME_WORKER_ID isn't set)",
            )
        })?;
        let status = match req_str(p, "event")? {
            "done" => WorkerStatus::Done,
            "fail" => WorkerStatus::Failed,
            other => {
                return Err(CliError::invalid(format!(
                    "invalid event `{other}` (use done or fail)"
                )))
            }
        };
        let end = self.with_store(|store| {
            let end = store.finish_worker(
                run_id,
                name,
                status,
                None,
                opt_str(p, "summary"),
                None,
                true,
            )?;
            self.sync(store, run_id);
            Ok(end)
        })?;
        // The reporter is running in that session: let it get the reply.
        self.after_end(run_id, &end, Some(REPORT_GRACE));
        Ok(json!(end.worker))
    }

    /// `worker.kill {run_id, name}`: stop it now, `cancelled`.
    fn kill_worker(&self, run_id: i64, name: &str) -> CliResult<Value> {
        let end = self.with_store(|store| {
            let end = store.finish_worker(
                run_id,
                name,
                WorkerStatus::Cancelled,
                Some(KILLED),
                None,
                None,
                true,
            )?;
            self.sync(store, run_id);
            Ok(end)
        })?;
        if let Some(s) = self.worker_session(run_id, &end.worker) {
            session::kill(&s);
        }
        self.after_end(run_id, &end, None);
        Ok(json!(end.worker))
    }

    /// `group.status {run_id, name, wait?}`; `close` stops it taking members
    /// (the first `wait` does, and so does `group.close`).
    fn group_status(
        &self,
        run_id: i64,
        name: &str,
        close: bool,
        waiting: bool,
    ) -> CliResult<Value> {
        self.with_store(|store| {
            let mut group = store.require_group(run_id, name)?;
            let workers = store.members(run_id, name)?;
            if waiting && workers.is_empty() {
                return Err(CliError::invalid(format!(
                    "group `{name}` has no members to wait for"
                ))
                .with_hint("spawn workers into it with `tome worker spawn --group`"));
            }
            if close {
                group = store.close_group(run_id, name)?.0;
                self.sync(store, run_id);
            }
            Ok(json!({ "group": group, "workers": workers }))
        })
    }

    /// `worktree.create {run_id, name, base?, branch?, cwd?}`: a worktree not tied to
    /// a worker.
    fn create_worktree(&self, run_id: i64, p: &Value) -> CliResult<Value> {
        let name = req_str(p, "name")?;
        store::check_name("worktree", name)?;
        let run = self.with_store(|store| store.require_run(run_id))?;
        if run.status != RunStatus::Running {
            return Err(CliError::invalid(format!(
                "run {run_id} isn't running ({})",
                run.status.as_str()
            )));
        }
        let base = worktree::resolve_base(&repo_dir(&run, p), opt_str(p, "base"))?;
        let created = worktree::create(&base, run_id, name, opt_str(p, "branch"))?;
        self.with_store(|store| {
            store
                .add_worktree(
                    run_id,
                    &NewWorktree {
                        path: &created.path,
                        repo_path: Some(&base.repo),
                        branch: Some(&created.branch),
                        base: Some(&base.name),
                        worker: None,
                    },
                )
                .map_err(internal)
        })?;
        Ok(
            json!({ "name": name, "path": created.path, "branch": created.branch, "base": base.name }),
        )
    }

    pub(crate) fn worker_session(&self, run_id: i64, w: &Worker) -> Option<store::Session> {
        let name = w.session.as_deref()?;
        self.recorded_sessions(run_id)
            .into_iter()
            .find(|s| s.name == name)
    }

    /// After a worker ended: close its session (after `grace`, unless it
    /// was kept open), kill any members fail-fast cut off, and tell the
    /// orchestrator.
    pub(crate) fn after_end(&self, run_id: i64, end: &WorkerEnd, grace: Option<Duration>) {
        let sessions = self.recorded_sessions(run_id);
        let session_of = |w: &Worker| {
            w.session
                .as_deref()
                .and_then(|n| sessions.iter().find(|s| s.name == n).cloned())
        };
        if !end.worker.keep_open {
            if let Some(s) = session_of(&end.worker) {
                match grace {
                    Some(grace) => {
                        std::thread::spawn(move || {
                            std::thread::sleep(grace);
                            session::kill(&s);
                        });
                    }
                    None => {
                        session::kill(&s);
                    }
                }
            }
        }
        for w in &end.cut {
            if let Some(s) = session_of(w) {
                session::kill(&s);
            }
        }
        let running = self
            .with_store(|store| store.require_run(run_id))
            .is_ok_and(|r| r.status == RunStatus::Running);
        let Some(orch) = sessions
            .iter()
            .find(|s| orchestrator::is_main(&s.role))
            .filter(|_| running)
        else {
            return;
        };
        let w = &end.worker;
        let mut lines = vec![format!(
            "[tome] worker {} {}. Details: tome worker status {}",
            w.name,
            w.status.as_str(),
            w.name
        )];
        if let Some(g) = &end.group_finished {
            lines.push(format!(
                "[tome] group {} finished. Details: tome group status {}",
                g.name, g.name
            ));
        }
        for line in lines {
            // A missing pane just drops the nudge.
            session::send_line(orch, &line);
        }
    }

    /// End running workers whose session is over: a command worker by its
    /// exit code, anything else as `worker_exited`. Called by the monitor.
    pub(crate) fn check_workers(&self) {
        let Ok(workers) = self.with_store(|store| store.running_workers()) else {
            return;
        };
        for w in workers {
            let exited = recorded_exit(w.run_id, &w.name);
            if exited.is_none() {
                let Some(s) = self.worker_session(w.run_id, &w) else {
                    continue;
                };
                // Alive, or can't tell right now: look again next time.
                if session::is_alive(&s) != Some(false) {
                    continue;
                }
            }
            // It may have written its exit code as it went.
            let exited = exited.or_else(|| recorded_exit(w.run_id, &w.name));
            let (status, reason, summary) = match (w.kind.as_str(), exited) {
                ("command", Some(code)) => {
                    let tail = store::read_tail(&log_file(w.run_id, &w.name), SUMMARY_LINES);
                    let tail = strip_ansi(&tail);
                    let mut summary = format!("exit code {code}");
                    if !tail.trim().is_empty() {
                        summary.push_str(&format!("\n{}", tail.trim_end()));
                    }
                    if code == 0 {
                        (WorkerStatus::Done, None, summary)
                    } else {
                        (WorkerStatus::Failed, Some(EXIT_CODE), summary)
                    }
                }
                (_, code) => {
                    let summary = match code {
                        Some(code) => format!("exited (code {code}) without reporting"),
                        None => "its session ended without reporting".to_string(),
                    };
                    (WorkerStatus::Failed, Some(EXITED), summary)
                }
            };
            let end = self.with_store(|store| {
                let end = store.finish_worker(
                    w.run_id,
                    &w.name,
                    status,
                    reason,
                    Some(&summary),
                    exited,
                    true,
                )?;
                self.sync(store, w.run_id);
                Ok(end)
            });
            // Err: it reported (or was ended) meanwhile; that stands.
            if let Ok(end) = end {
                eprintln!(
                    "tome daemon: run {} worker {} {}",
                    w.run_id,
                    w.name,
                    status.as_str()
                );
                self.after_end(w.run_id, &end, None);
            }
        }
    }
}

/// The queue RPCs: `{queue?, body?, id?, limit?, run_id?, project_path?,
/// caller?}`. A queue belongs to a project: the run's (`run_id`), else
/// `project_path`. The sender or claimer is `user` outside a run, else the
/// run or its `caller` worker.
fn queue(store: &mut Store, method: &str, p: &Value) -> CliResult<Value> {
    let run = match p.get("run_id").filter(|v| !v.is_null()) {
        Some(_) => Some(store.require_run(req_id_at(p, "run_id")?)?),
        None => None,
    };
    let project = match (&run, opt_str(p, "project_path")) {
        (Some(run), _) => run.project_path.clone().ok_or_else(|| {
            CliError::invalid(format!("run {} has no project", run.id))
                .with_hint("queues belong to a project: run the workflow inside one")
        })?,
        (None, Some(path)) => path.to_string(),
        (None, None) => {
            return Err(CliError::invalid("not inside a project")
                .with_hint("run `tome queue` in a project (a directory with .tome/)"))
        }
    };
    let me = match &run {
        Some(run) => crate::bus::run_sender(run.id, caller(p)),
        None => "user".to_string(),
    };
    match method {
        "queue.push" => Ok(json!(store.push_message(
            &project,
            req_str(p, "queue")?,
            req_str(p, "body")?,
            &me
        )?)),
        "queue.pull" => Ok(
            match store.pull_message(&project, req_str(p, "queue")?, &me)? {
                Pulled::Message(m) => json!({ "status": "message", "message": m }),
                Pulled::Empty => json!({ "status": "empty" }),
                Pulled::Closed => json!({ "status": "closed" }),
            },
        ),
        "queue.peek" => {
            let limit = p.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize;
            let messages = store.peek_messages(&project, req_str(p, "queue")?, limit)?;
            Ok(json!({ "queue": req_str(p, "queue")?, "messages": messages }))
        }
        "queue.ack" => Ok(json!(store.ack_message(&project, req_id_at(p, "id")?)?)),
        "queue.close" => Ok(json!(store.close_queue(&project, req_str(p, "queue")?)?)),
        "queue.ls" => Ok(json!({ "project": project, "queues": store.queues(&project)? })),
        _ => Err(CliError::invalid(format!("unknown method `{method}`"))),
    }
}

/// Drop terminal escape sequences (colours, cursor moves) from captured
/// output.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.peek() {
                Some('[') => {
                    chars.next();
                    while let Some(c) = chars.next() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    // OSC: up to BEL or ESC \.
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' || (c == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) {
                            break;
                        }
                    }
                }
                _ => {
                    chars.next();
                }
            },
            '\r' => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_are_stripped() {
        assert_eq!(
            strip_ansi("\u{1b}[31mred\u{1b}[0m\r\n\u{1b}]0;title\u{7}ok"),
            "red\nok"
        );
    }

    #[test]
    fn wrapper_records_the_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let exit = dir.path().join("it's.exit");
        let argv = wrap(&["sh".into(), "-c".into(), "exit 3".into()], &exit, false);
        let status = std::process::Command::new(&argv[0])
            .args(&argv[1..])
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(fs::read_to_string(&exit).unwrap().trim(), "3");
    }
}
