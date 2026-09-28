//! Arming: which triggers are live. The daemon arms the triggers of every
//! registered, enabled project plus the global workflows', re-reads
//! workflow files when they change, and drops projects whose directory is
//! gone.
//!
//! The event sources (cron, file watching) run off the armed set in the
//! trigger loop.

use crate::engine::Engine;
use crate::store::NewFire;
use crate::triggers::{self, outcome, Event, FireRequest};
use chrono::{DateTime, Local};
use crate::workflow::{Library, Scope, Trigger, TriggerKind};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// Trigger index recorded for problems with a whole workflow or project.
pub const WHOLE: i64 = -1;

/// How often the trigger loop looks at the world, overridable for tests.
pub fn tick() -> Duration {
    std::env::var("TOME_TRIGGER_TICK_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .map(Duration::from_millis)
        .unwrap_or(Duration::from_secs(1))
}

/// One armed trigger.
#[derive(Debug, Clone)]
pub struct Armed {
    pub workflow_path: PathBuf,
    pub name: String,
    /// The project root; `None` for a global workflow.
    pub project: Option<PathBuf>,
    pub index: usize,
    pub trigger: Trigger,
}

impl Armed {
    /// Stable across re-arms while the trigger itself is unchanged, so event
    /// sources keep their state (next cron time, seen files).
    pub fn key(&self) -> String {
        format!(
            "{}\u{0}{}\u{0}{}\u{0}{}",
            self.project.as_deref().map(|p| p.display().to_string()).unwrap_or_default(),
            self.workflow_path.display(),
            self.index,
            self.trigger.describe()
        )
    }
}

/// A workflow whose triggers can't be armed.
#[derive(Debug, Clone)]
pub struct Broken {
    pub workflow_path: PathBuf,
    pub name: String,
    pub project: Option<PathBuf>,
    pub message: String,
}

#[derive(Debug, Default)]
pub struct Scan {
    pub armed: Vec<Armed>,
    pub broken: Vec<Broken>,
    /// The workflow names each project defines: a global workflow of the
    /// same name doesn't act inside that project.
    pub project_names: HashMap<PathBuf, HashSet<String>>,
}

impl Scan {
    /// Whether a global workflow `name` is shadowed at `path`.
    pub fn shadowed(&self, name: &str, path: &Path) -> bool {
        self.project_names.iter().any(|(root, names)| path.starts_with(root) && names.contains(name))
    }
}

/// Invalid workflows only count as broken triggers when they (seem to)
/// declare triggers.
fn mentions_triggers(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|t| t.lines().any(|l| l.trim_start().starts_with("triggers:")))
}

/// Read the armed triggers of `projects` (roots) and the global workflows.
pub fn scan(projects: &[PathBuf]) -> Scan {
    let global_dir = crate::paths::tome_home().join("workflows");
    let mut out = Scan::default();
    let mut libraries = vec![(None, Library { global_dir: global_dir.clone(), project_dir: None })];
    for root in projects {
        let lib = Library { global_dir: global_dir.clone(), project_dir: Some(root.join(".tome/workflows")) };
        libraries.push((Some(root.clone()), lib));
    }
    for (project, lib) in libraries {
        let scope = if project.is_some() { Scope::Project } else { Scope::Global };
        for entry in lib.entries().into_iter().filter(|e| e.scope == scope) {
            let name = entry
                .name()
                .map(str::to_string)
                .unwrap_or_else(|| entry.path.file_stem().unwrap_or_default().to_string_lossy().into_owned());
            if let Some(root) = &project {
                out.project_names.entry(root.clone()).or_default().insert(name.clone());
            }
            match entry.result {
                Ok(wf) => {
                    for (index, trigger) in wf.frontmatter.triggers.iter().enumerate() {
                        if matches!(trigger.kind, TriggerKind::Manual) {
                            continue;
                        }
                        out.armed.push(Armed {
                            workflow_path: entry.path.clone(),
                            name: name.clone(),
                            project: project.clone(),
                            index,
                            trigger: trigger.clone(),
                        });
                    }
                }
                Err(inv) if mentions_triggers(&entry.path) => out.broken.push(Broken {
                    workflow_path: entry.path.clone(),
                    name,
                    project: project.clone(),
                    message: triggers::first_errors(&inv),
                }),
                Err(_) => {}
            }
        }
    }
    out
}

/// A cron time this late (the daemon was stopped, the machine asleep) is
/// skipped rather than fired.
const MISSED_AFTER: chrono::Duration = chrono::Duration::seconds(30);

/// The next time of each armed cron trigger.
#[derive(Debug, Default)]
pub struct CronState {
    next: HashMap<String, DateTime<Local>>,
}

impl CronState {
    /// The cron triggers due at `now`, each with its scheduled time. A newly
    /// armed trigger waits for its next time; a time missed by more than
    /// `MISSED_AFTER` is skipped.
    pub fn due(&mut self, armed: &[Armed], now: DateTime<Local>) -> Vec<(Armed, DateTime<Local>)> {
        let mut out = Vec::new();
        let mut next = HashMap::new();
        for a in armed {
            let TriggerKind::Cron { schedule, .. } = &a.trigger.kind else { continue };
            let key = a.key();
            let Some(at) = self.next.get(&key).copied().or_else(|| schedule.next_local(now)) else { continue };
            if now < at {
                next.insert(key, at);
                continue;
            }
            if now - at <= MISSED_AFTER {
                out.push((a.clone(), at));
            } else {
                eprintln!("tome daemon: skipped missed cron time {} of {} ({})", at.to_rfc3339(), a.name, a.trigger.describe());
            }
            if let Some(n) = schedule.next_local(now) {
                next.insert(key, n);
            }
        }
        self.next = next;
        out
    }
}

/// What the workflow files look like: when it changes, re-arm.
fn signature(projects: &[PathBuf]) -> Vec<(PathBuf, Option<SystemTime>, u64)> {
    let mut dirs = vec![crate::paths::tome_home().join("workflows")];
    dirs.extend(projects.iter().map(|p| p.join(".tome/workflows")));
    let mut sig = Vec::new();
    for dir in dirs {
        sig.push((dir.clone(), None, 0));
        let Ok(read) = std::fs::read_dir(&dir) else { continue };
        for e in read.flatten() {
            let path = e.path();
            if path.extension().is_some_and(|x| x == "md") {
                let meta = e.metadata().ok();
                sig.push((path, meta.as_ref().and_then(|m| m.modified().ok()), meta.map(|m| m.len()).unwrap_or(0)));
            }
        }
    }
    sig.sort();
    sig
}

impl Engine {
    /// Enabled projects, after dropping those whose directory is gone.
    pub(crate) fn live_projects(&self) -> Vec<PathBuf> {
        let Ok(projects) = self.with_store(|store| store.projects()) else { return Vec::new() };
        let mut live = Vec::new();
        for p in projects {
            let root = PathBuf::from(&p.path);
            if !root.is_dir() {
                let message = format!("project directory {} no longer exists; dropped it", p.path);
                let _ = self.with_store(|store| {
                    store.drop_project(&p.path)?;
                    store.record_fire(&NewFire {
                        workflow_path: &p.path,
                        workflow_name: "",
                        project_path: Some(&p.path),
                        trigger_index: WHOLE,
                        trigger: "project",
                        outcome: outcome::ERROR,
                        message: Some(&message),
                        run_ids: &[],
                    })
                });
                eprintln!("tome daemon: {message}");
                self.trigger_failed("project", &message);
                continue;
            }
            if p.enabled {
                live.push(root);
            }
        }
        live
    }

    /// The daemon's trigger loop: keep the armed set current and drive the
    /// event sources off it.
    pub fn trigger_loop(self: Arc<Self>) {
        let mut sig = Vec::new();
        let mut current = Scan::default();
        // Broken workflows already recorded, with their error.
        let mut reported: HashMap<(PathBuf, Option<PathBuf>), String> = HashMap::new();
        let mut cron = CronState::default();
        let mut files = crate::watch::Watcher::default();
        loop {
            let projects = self.live_projects();
            let now_sig = signature(&projects);
            if now_sig != sig {
                sig = now_sig;
                let next = scan(&projects);
                let was_armed: HashSet<(PathBuf, Option<PathBuf>)> =
                    current.armed.iter().map(|a| (a.workflow_path.clone(), a.project.clone())).collect();
                let mut still = HashMap::new();
                for b in &next.broken {
                    let key = (b.workflow_path.clone(), b.project.clone());
                    if reported.get(&key) != Some(&b.message) {
                        self.record_broken(b, was_armed.contains(&key));
                    }
                    still.insert(key, b.message.clone());
                }
                reported = still;
                current = next;
                eprintln!("tome daemon: {} trigger(s) armed", current.armed.len());
            }
            for (armed, at) in cron.due(&current.armed, Local::now()) {
                let event = Event { scheduled: Some(at), ..Default::default() };
                self.fire_armed(&armed, event);
            }
            let muted = |a: &Armed| {
                a.trigger.mutes_while_running() && self.active_runs(&a.name, &a.workflow_path).is_ok_and(|r| !r.is_empty())
            };
            for (armed, event) in files.poll(&current, std::time::Instant::now(), muted) {
                self.fire_armed(&armed, event);
            }
            std::thread::sleep(tick());
        }
    }

    /// Fire an armed trigger off the loop's thread.
    fn fire_armed(self: &Arc<Self>, armed: &Armed, event: Event) {
        let req = FireRequest {
            workflow_path: armed.workflow_path.clone(),
            project: armed.project.clone(),
            index: armed.index,
            event,
            dry_run: false,
        };
        let engine = Arc::clone(self);
        std::thread::spawn(move || engine.fire(&req));
    }

    fn record_broken(&self, b: &Broken, was_armed: bool) {
        let project = b.project.as_ref().map(|p| p.display().to_string());
        let recorded = self.with_store(|store| {
            store.record_fire(&NewFire {
                workflow_path: &b.workflow_path.display().to_string(),
                workflow_name: &b.name,
                project_path: project.as_deref(),
                trigger_index: WHOLE,
                trigger: "workflow",
                outcome: outcome::ERROR,
                message: Some(&b.message),
                run_ids: &[],
            })
        });
        if let Err(e) = recorded {
            eprintln!("tome daemon: recording a broken workflow failed: {e}");
        }
        let verb = if was_armed { "disarmed" } else { "not armed" };
        eprintln!("tome daemon: triggers of {} {verb}: {}", b.name, b.message);
        self.trigger_failed(&b.name, &b.message);
    }

    /// A trigger couldn't act: notify.
    pub(crate) fn trigger_failed(&self, what: &str, message: &str) {
        crate::orchestrator::notify_trigger(what, message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn armed(cron: &str) -> Armed {
        let text = format!("---\nname: w\ntriggers:\n  - cron: \"{cron}\"\n---\n");
        let wf = crate::workflow::parse(Path::new("w.md"), &text).unwrap();
        Armed { workflow_path: "w.md".into(), name: "w".into(), project: None, index: 0, trigger: wf.frontmatter.triggers[0].clone() }
    }

    fn at(h: u32, m: u32, s: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 3, 2, h, m, s).unwrap()
    }

    #[test]
    fn cron_fires_on_schedule_and_skips_missed_times() {
        let a = [armed("*/10 9-17 * * *")];
        let mut state = CronState::default();
        assert!(state.due(&a, at(9, 1, 0)).is_empty(), "arming waits for the next time");
        assert!(state.due(&a, at(9, 9, 59)).is_empty());
        let due = state.due(&a, at(9, 10, 1));
        assert_eq!(due.iter().map(|(_, t)| *t).collect::<Vec<_>>(), [at(9, 10, 0)]);
        assert!(state.due(&a, at(9, 10, 2)).is_empty(), "fires once");
        // Asleep from 9:15 to 9:45: 9:20..9:40 are skipped, not caught up.
        assert!(state.due(&a, at(9, 45, 0)).is_empty());
        assert_eq!(state.due(&a, at(9, 50, 0)).len(), 1);
        // Disarmed and re-armed: starts over from the next time.
        state.due(&[], at(9, 51, 0));
        assert!(state.due(&a, at(10, 5, 0)).is_empty());
        assert_eq!(state.due(&a, at(10, 10, 0))[0].1, at(10, 10, 0));
    }
}
