//! File triggers: OS file events (FSEvents, inotify, through `notify`) over
//! each armed file trigger's glob, with a polling watcher as the fallback.
//!
//! Only the directories the glob could match in are watched: never `.git/`,
//! `.tome/`, gitignored directories or the trigger's `ignore:` globs. On
//! Linux each directory gets its own watch, so ignored trees don't use up
//! inotify watches; elsewhere one recursive watch covers the glob's base and
//! ignored paths are filtered out of its events.
//!
//! Each trigger keeps an index of file stats, built when it arms (which
//! fires nothing). An event is turned into changes by re-reading the paths
//! it names and diffing them against the index, so the index decides
//! created vs modified. When the OS says it dropped events (an inotify
//! queue overflow, FSEvents MustScanSubDirs), the watched tree is walked
//! once and diffed against the index, so no change is lost. The polling
//! watcher instead walks the whole tree every tick and diffs it the same
//! way. Changes are batched until the
//! debounce window passes with no new ones, then fire as one event.
//!
//! `TOME_FILE_WATCH=poll` puts every trigger on the polling watcher.
//!
//! While the glob's base doesn't exist (yet, or any more), its nearest
//! existing ancestor is watched instead. When the base appears its files are
//! indexed without firing; files created after that fire as usual.

use crate::arming::{Armed, Scan};
use crate::glob::Glob;
use crate::triggers::Event;
use crate::workflow::{FileEvent, FileTrigger, TriggerKind};
use notify::event::{EventKind, ModifyKind};
use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Instant, SystemTime};

/// Never watched, at any depth.
const SKIPPED_DIRS: &[&str] = &[".git", ".tome"];

type Stat = (Option<SystemTime>, u64);

/// The stats of the files a trigger's glob matches. Ordered, so everything
/// under a directory is one range.
type Index = BTreeMap<PathBuf, Stat>;

/// Where and how one trigger's glob is matched.
struct Spec {
    /// Where the walk starts: the glob's literal leading directories.
    base: PathBuf,
    /// Relative globs match paths relative to this (the project root).
    root: Option<PathBuf>,
    glob: Glob,
    ignore: Vec<Glob>,
}

impl Spec {
    fn new(a: &Armed, f: &FileTrigger) -> Option<Spec> {
        let pattern = expand(&f.file);
        let absolute = Glob::is_absolute(&f.file);
        let root = match (&a.project, absolute) {
            (Some(p), false) => Some(p.clone()),
            (None, false) => return None, // rejected by validation
            (_, true) => None,
        };
        let glob = Glob::new(&pattern).ok()?;
        let ignore = f
            .ignore
            .iter()
            .filter_map(|g| Glob::new(&expand(g)).ok())
            .collect();
        let literal: Vec<&str> = pattern.split('/').collect();
        let literal = &literal[..literal.len().saturating_sub(1)];
        let mut base = match &root {
            Some(r) => r.clone(),
            None => PathBuf::from("/"),
        };
        for seg in literal
            .iter()
            .take_while(|s| !s.contains(['*', '?', '[', '{', '\\']))
        {
            if !seg.is_empty() {
                base.push(seg);
            }
        }
        Some(Spec {
            base,
            root,
            glob,
            ignore,
        })
    }

    /// A path as the globs see it.
    fn form(&self, path: &Path) -> String {
        match &self.root {
            Some(r) => path.strip_prefix(r).unwrap_or(path).display().to_string(),
            None => path.display().to_string(),
        }
    }

    fn ignored(&self, form: &str) -> bool {
        self.ignore.iter().any(|g| g.matches(form))
    }

    /// Whether the walk would descend into `dir` from its parent (gitignore
    /// aside).
    fn enters(&self, dir: &Path) -> bool {
        let form = self.form(dir);
        let skipped = dir
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| SKIPPED_DIRS.contains(&n));
        !skipped && self.glob.could_contain(&form) && !self.ignored(&form)
    }

    /// Whether the walk reaches `dir`: it's the base, or every directory
    /// from the base down to it is entered.
    fn reaches(&self, dir: &Path, ignored_dirs: &mut HashMap<PathBuf, bool>) -> bool {
        let Ok(rel) = dir.strip_prefix(&self.base) else {
            return false;
        };
        let mut at = self.base.clone();
        for c in rel.components() {
            let parent = at.clone();
            at.push(c);
            if !self.enters(&at) || gitignored_dir(&parent, &at, ignored_dirs) {
                return false;
            }
        }
        true
    }

    /// Whether `path` is a file the glob counts.
    fn counts(&self, path: &Path) -> bool {
        let form = self.form(path);
        self.glob.matches(&form) && !self.ignored(&form)
    }
}

fn expand(pattern: &str) -> String {
    match pattern.strip_prefix("~/") {
        Some(rest) => format!("{}/{rest}", crate::paths::user_home().display()),
        None => pattern.to_string(),
    }
}

/// How file triggers get their changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Os,
    Poll,
}

impl Mode {
    /// From `TOME_FILE_WATCH`: `poll` forces polling; anything else but
    /// `os` is warned about. OS events are the default.
    fn parse(value: Option<&str>) -> Mode {
        match value {
            Some("poll") => Mode::Poll,
            None | Some("" | "os") => Mode::Os,
            Some(other) => {
                eprintln!("tome daemon: unknown TOME_FILE_WATCH={other:?} (expected `poll`); using OS file events");
                Mode::Os
            }
        }
    }
}

const FORCED: &str = "forced by TOME_FILE_WATCH=poll";

/// Where one trigger's changes come from.
enum Source {
    /// Walk the tree every tick, and why the trigger isn't on OS events.
    Poll(Option<String>),
    Os(Os),
}

/// A trigger's OS watch.
struct Os {
    watcher: RecommendedWatcher,
    events: Receiver<notify::Result<notify::Event>>,
    /// The watched directory, as the spec sees it and as the OS reports it
    /// (FSEvents reports resolved paths: `/private/tmp` for `/tmp`).
    root: PathBuf,
    real: PathBuf,
    /// One watch per directory rather than one recursive watch.
    per_dir: bool,
    /// The directories walked so far (each watched, when `per_dir`).
    dirs: BTreeSet<PathBuf>,
    /// The base is missing, so `root` is its nearest existing ancestor,
    /// watched on its own until the base appears.
    waiting: bool,
}

impl Os {
    fn start(base: &Path, per_dir: bool) -> notify::Result<Os> {
        let (tx, events) = mpsc::channel();
        let watcher = notify::recommended_watcher(tx)?;
        let mut os = Os {
            watcher,
            events,
            root: base.to_path_buf(),
            real: base.to_path_buf(),
            per_dir,
            dirs: BTreeSet::new(),
            waiting: false,
        };
        os.aim(base)?;
        Ok(os)
    }

    /// Watch `base`, or while it's missing, its nearest existing ancestor.
    /// Drops every earlier watch.
    fn aim(&mut self, base: &Path) -> notify::Result<()> {
        for d in std::mem::take(&mut self.dirs) {
            let _ = self.watcher.unwatch(&d);
        }
        let nearest = || {
            base.ancestors()
                .find(|p| p.is_dir())
                .unwrap_or(base)
                .to_path_buf()
        };
        loop {
            let target = nearest();
            let waiting = target != base;
            let mode = if self.per_dir || waiting {
                RecursiveMode::NonRecursive
            } else {
                RecursiveMode::Recursive
            };
            self.watcher.watch(&target, mode)?;
            // A directory on the way to the base may have appeared before
            // the watch did; aim again if so.
            if nearest() == target {
                self.real = target.canonicalize().unwrap_or_else(|_| target.clone());
                self.dirs = BTreeSet::from([target.clone()]);
                self.root = target;
                self.waiting = waiting;
                return Ok(());
            }
            let _ = self.watcher.unwatch(&target);
        }
    }

    /// Note a directory the walk is about to read, watching it first when
    /// watches are per directory.
    fn add_dir(&mut self, dir: &Path) -> Result<(), String> {
        if self.dirs.insert(dir.to_path_buf()) && self.per_dir {
            self.watcher
                .watch(dir, RecursiveMode::NonRecursive)
                .map_err(|e| watch_failed(dir, e))?;
        }
        Ok(())
    }

    /// Forget the directories at and under `path` (it's gone).
    fn remove_dirs(&mut self, path: &Path) {
        let gone: Vec<PathBuf> = self
            .dirs
            .range(path.to_path_buf()..)
            .take_while(|d| d.starts_with(path))
            .cloned()
            .collect();
        for d in gone {
            self.dirs.remove(&d);
            if self.per_dir {
                let _ = self.watcher.unwatch(&d);
            }
        }
    }

    /// An event path as the spec sees it.
    fn shown(&self, path: PathBuf) -> PathBuf {
        match path.strip_prefix(&self.real) {
            Ok(rest) if self.real != self.root => self.root.join(rest),
            _ => path,
        }
    }

    /// The paths events named since the last drain, each with whether an
    /// event could have put something new there (a create or a rename).
    /// `None` when the OS dropped events, so the whole tree needs a rescan.
    fn drain(&mut self) -> Result<Option<BTreeMap<PathBuf, bool>>, String> {
        let mut touched: BTreeMap<PathBuf, bool> = BTreeMap::new();
        let mut rescan = false;
        loop {
            match self.events.try_recv() {
                Ok(Ok(ev)) if ev.need_rescan() => rescan = true,
                Ok(Ok(ev)) => {
                    let arrived = matches!(
                        ev.kind,
                        EventKind::Create(_)
                            | EventKind::Modify(ModifyKind::Name(_))
                            | EventKind::Any
                            | EventKind::Other
                    );
                    for p in ev.paths {
                        *touched.entry(self.shown(p)).or_default() |= arrived;
                    }
                }
                Ok(Err(e)) => return Err(format!("the OS watch failed: {e}")),
                Err(TryRecvError::Empty) => return Ok((!rescan).then_some(touched)),
                Err(TryRecvError::Disconnected) => return Err("the OS watch stopped".into()),
            }
        }
    }
}

struct State {
    spec: Option<Spec>,
    source: Source,
    index: Index,
    pending: Vec<(PathBuf, FileEvent)>,
    last_change: Option<Instant>,
    /// Gitignore answers for directories, asked once.
    ignored_dirs: HashMap<PathBuf, bool>,
}

pub struct Watcher {
    states: HashMap<String, State>,
    mode: Mode,
    /// One OS watch per directory (inotify) rather than one recursive watch
    /// (FSEvents).
    per_dir: bool,
}

impl Default for Watcher {
    fn default() -> Watcher {
        let mode = Mode::parse(std::env::var("TOME_FILE_WATCH").ok().as_deref());
        Watcher {
            states: HashMap::new(),
            mode,
            per_dir: cfg!(target_os = "linux"),
        }
    }
}

impl Watcher {
    /// Collect every armed file trigger's changes and return those whose
    /// batched changes are ready to fire. `muted` says whether a trigger is
    /// muted right now (`while_running: mute` and its workflow has a run
    /// going); changes while it is are dropped.
    pub fn poll(
        &mut self,
        scan: &Scan,
        now: Instant,
        muted: impl Fn(&Armed) -> bool,
    ) -> Vec<(Armed, Event)> {
        let mut out = Vec::new();
        let mut states = std::mem::take(&mut self.states);
        let mut kept = HashMap::new();
        for a in &scan.armed {
            let TriggerKind::File(f) = &a.trigger.kind else {
                continue;
            };
            let key = a.key();
            let (mut st, was_polling) = match states.remove(&key) {
                Some(st) => {
                    let polling = st.polling().is_some();
                    (st, polling)
                }
                None => (State::arm(a, f, self.mode, self.per_dir), false),
            };
            if let Some(event) = st.poll(a, f, scan, now, &muted) {
                out.push((a.clone(), event));
            }
            if let (false, Some(reason)) = (was_polling, st.polling()) {
                eprintln!(
                    "tome daemon: {} of {} is polling: {reason}",
                    a.trigger.describe(),
                    a.name
                );
            }
            kept.insert(key, st);
        }
        self.states = kept;
        out
    }

    /// The armed file triggers that are polling, by key, with why. They
    /// stay polling until re-armed.
    pub fn polling(&self) -> HashMap<String, String> {
        self.states
            .iter()
            .filter_map(|(k, st)| Some((k.clone(), st.polling()?.to_string())))
            .collect()
    }
}

impl State {
    /// Why the trigger is polling rather than on OS events, if it is.
    fn polling(&self) -> Option<&str> {
        match &self.source {
            Source::Poll(reason) => reason.as_deref(),
            Source::Os(_) => None,
        }
    }

    /// Set up the trigger's watch, then index what's there (firing
    /// nothing). The watch goes first so no change slips in between.
    fn arm(a: &Armed, f: &FileTrigger, mode: Mode, per_dir: bool) -> State {
        let mut st = State {
            spec: Spec::new(a, f),
            source: Source::Poll(None),
            index: Index::new(),
            pending: Vec::new(),
            last_change: None,
            ignored_dirs: HashMap::new(),
        };
        let Some(spec) = &st.spec else { return st };
        let (mut os, mut failed) = match mode {
            Mode::Os => match Os::start(&spec.base, per_dir) {
                Ok(os) => (Some(os), None),
                Err(e) => (None, Some(watch_failed(&spec.base, e))),
            },
            Mode::Poll => (None, Some(FORCED.to_string())),
        };
        st.index = walk(
            spec,
            &spec.base,
            &mut st.ignored_dirs,
            |dir| match &mut os {
                Some(os) if failed.is_none() && !os.waiting => failed = os.add_dir(dir).err(),
                _ => {}
            },
        );
        st.source = match (os, failed) {
            (Some(os), None) => Source::Os(os),
            (_, reason) => Source::Poll(reason),
        };
        st
    }

    fn poll(
        &mut self,
        a: &Armed,
        f: &FileTrigger,
        scan: &Scan,
        now: Instant,
        muted: &impl Fn(&Armed) -> bool,
    ) -> Option<Event> {
        let mut changes = self.changes();
        let spec = self.spec.as_ref()?;
        changes.retain(|(p, e)| {
            f.on.contains(e) && !(a.project.is_none() && scan.shadowed(&a.name, p))
        });
        if !changes.is_empty() {
            let ignored = git_ignored(&spec.base, changes.iter().map(|(p, _)| p.as_path()));
            changes.retain(|(p, _)| !ignored.contains(p));
        }
        if !changes.is_empty() {
            if muted(a) {
                // The index already has them, so they don't fire later.
                eprintln!(
                    "tome daemon: {} change(s) for {} dropped: its workflow is running",
                    changes.len(),
                    a.trigger.describe()
                );
            } else {
                changes.sort_by(|a, b| a.0.cmp(&b.0));
                for (p, e) in changes {
                    if !self.pending.iter().any(|(q, _)| *q == p) {
                        self.pending.push((p, e));
                    }
                }
                self.last_change = Some(now);
            }
        }
        let quiet = self
            .last_change
            .is_some_and(|t| now.duration_since(t) >= f.debounce);
        if self.pending.is_empty() || !quiet {
            return None;
        }
        let paths = std::mem::take(&mut self.pending)
            .into_iter()
            .map(|(p, e)| (spec.form(&p), e))
            .collect();
        Some(Event {
            paths,
            ..Default::default()
        })
    }

    /// The changes since the last tick, with the index brought up to date.
    fn changes(&mut self) -> Vec<(PathBuf, FileEvent)> {
        let Some(spec) = &self.spec else {
            return Vec::new();
        };
        let os = match &mut self.source {
            Source::Poll(_) => {
                let files = walk(spec, &spec.base, &mut self.ignored_dirs, |_| {});
                return diff(&mut self.index, &spec.base, files);
            }
            Source::Os(os) => os,
        };
        // `None`: the OS dropped events.
        let touched = match os.drain() {
            Ok(t) => t,
            Err(reason) => {
                // Changes the OS watch missed are caught by the next walk,
                // which diffs against the index.
                self.source = Source::Poll(Some(reason));
                return self.changes();
            }
        };
        if os.waiting {
            // Only the base appearing matters, and only an event in the
            // ancestor can say it has.
            if touched.is_some_and(|t| t.is_empty()) {
                return Vec::new();
            }
            if let Err(e) = os.aim(&spec.base) {
                self.source = Source::Poll(Some(watch_failed(&spec.base, e)));
                return self.changes();
            }
            if !os.waiting {
                // The base is here: index what's in it, firing nothing.
                let mut failed = None;
                self.index = walk(spec, &spec.base, &mut self.ignored_dirs, |dir| {
                    if failed.is_none() {
                        failed = os.add_dir(dir).err();
                    }
                });
                if let Some(reason) = failed {
                    self.source = Source::Poll(Some(reason));
                }
            }
            return Vec::new();
        }
        if !spec.base.is_dir() {
            // The base is gone. Its files leave the index (deletes never
            // fire), and its nearest ancestor is watched until it's back.
            self.index.clear();
            if let Err(e) = os.aim(&spec.base) {
                self.source = Source::Poll(Some(watch_failed(&spec.base, e)));
            }
            return Vec::new();
        }
        let Some(touched) = touched else {
            // Walk the watched tree once, so no change is lost.
            let mut failed = None;
            let mut seen = BTreeSet::new();
            let files = walk(spec, &spec.base, &mut self.ignored_dirs, |dir| {
                seen.insert(dir.to_path_buf());
                if failed.is_none() {
                    failed = os.add_dir(dir).err();
                }
            });
            for gone in os.dirs.difference(&seen).cloned().collect::<Vec<_>>() {
                os.remove_dirs(&gone);
            }
            if let Some(reason) = failed {
                self.source = Source::Poll(Some(reason));
            }
            return diff(&mut self.index, &spec.base, files);
        };
        let mut changes = Vec::new();
        let mut failed = None;
        for (path, arrived) in touched {
            if !path.starts_with(&spec.base) {
                continue;
            }
            let Some(parent) = path.parent().filter(|_| path != spec.base) else {
                // The base itself: its contents report their own events.
                continue;
            };
            if !spec.reaches(parent, &mut self.ignored_dirs) {
                continue;
            }
            let fresh = match std::fs::symlink_metadata(&path) {
                Err(_) => {
                    os.remove_dirs(&path);
                    Index::new()
                }
                Ok(m) if m.is_dir() => {
                    if !(arrived || !os.dirs.contains(&path)) {
                        continue;
                    }
                    if !spec.enters(&path) || gitignored_dir(parent, &path, &mut self.ignored_dirs)
                    {
                        continue;
                    }
                    // A new (or moved-in) directory: everything in it is new.
                    walk(spec, &path, &mut self.ignored_dirs, |dir| {
                        if failed.is_none() {
                            failed = os.add_dir(dir).err();
                        }
                    })
                }
                Ok(_) => stat_file(spec, &path).into_iter().collect(),
            };
            changes.extend(diff(&mut self.index, &path, fresh));
        }
        if let Some(reason) = failed {
            self.source = Source::Poll(Some(reason));
        }
        changes
    }
}

fn watch_failed(dir: &Path, e: notify::Error) -> String {
    format!("couldn't watch {}: {e}", dir.display())
}

/// A file's stat, if the glob counts it. Symlinked files count.
fn stat_file(spec: &Spec, path: &Path) -> Option<(PathBuf, Stat)> {
    let meta = std::fs::metadata(path).ok()?;
    (meta.is_file() && spec.counts(path))
        .then(|| (path.to_path_buf(), (meta.modified().ok(), meta.len())))
}

/// Replace the index entries at and under `under` with `fresh`, returning
/// the files that are new or changed. Files that are gone just leave the
/// index: deletes never fire.
fn diff(index: &mut Index, under: &Path, fresh: Index) -> Vec<(PathBuf, FileEvent)> {
    let gone: Vec<PathBuf> = index
        .range(under.to_path_buf()..)
        .map(|(p, _)| p)
        .take_while(|p| p.starts_with(under))
        .filter(|p| !fresh.contains_key(*p))
        .cloned()
        .collect();
    for p in gone {
        index.remove(&p);
    }
    let mut changes = Vec::new();
    for (path, stat) in fresh {
        match index.insert(path.clone(), stat) {
            None => changes.push((path, FileEvent::Created)),
            Some(old) if old != stat => changes.push((path, FileEvent::Modified)),
            _ => {}
        }
    }
    changes
}

/// The files under `start` (a directory the walk reaches) the glob matches,
/// with their stats. `on_dir` sees each directory before it's read.
fn walk(
    spec: &Spec,
    start: &Path,
    ignored_dirs: &mut HashMap<PathBuf, bool>,
    mut on_dir: impl FnMut(&Path),
) -> Index {
    let mut out = Index::new();
    let mut dirs = vec![start.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        on_dir(&dir);
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut subdirs = Vec::new();
        for entry in read.flatten() {
            let path = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                if spec.enters(&path) {
                    subdirs.push(path);
                }
                continue;
            }
            // Symlinked directories aren't followed.
            if let Some((path, stat)) = stat_file(spec, &path) {
                out.insert(path, stat);
            }
        }
        let unknown: Vec<&Path> = subdirs
            .iter()
            .filter(|d| !ignored_dirs.contains_key(*d))
            .map(PathBuf::as_path)
            .collect();
        if !unknown.is_empty() {
            let ignored = git_ignored(&dir, unknown.iter().copied());
            for d in unknown {
                ignored_dirs.insert(d.to_path_buf(), ignored.contains(d));
            }
        }
        dirs.extend(
            subdirs
                .into_iter()
                .filter(|d| ignored_dirs.get(d) != Some(&true)),
        );
    }
    out
}

/// Whether git ignores directory `dir` (in `parent`), asked once.
fn gitignored_dir(parent: &Path, dir: &Path, ignored_dirs: &mut HashMap<PathBuf, bool>) -> bool {
    if let Some(known) = ignored_dirs.get(dir) {
        return *known;
    }
    let ignored = git_ignored(parent, std::iter::once(dir)).contains(dir);
    ignored_dirs.insert(dir.to_path_buf(), ignored);
    ignored
}

/// Which of `paths` git ignores (none outside a repo).
fn git_ignored<'a>(dir: &Path, paths: impl Iterator<Item = &'a Path>) -> HashSet<PathBuf> {
    let paths: Vec<&Path> = paths.collect();
    let Ok(mut child) = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["check-ignore", "--stdin", "-z"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return HashSet::new();
    };
    if let Some(mut stdin) = child.stdin.take() {
        for p in &paths {
            let _ = stdin.write_all(p.as_os_str().as_encoded_bytes());
            let _ = stdin.write_all(b"\0");
        }
    }
    let Ok(out) = child.wait_with_output() else {
        return HashSet::new();
    };
    out.stdout
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| PathBuf::from(String::from_utf8_lossy(s).into_owned()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Duration;

    fn armed(root: &Path, trigger: &str) -> Armed {
        let text = format!("---\nname: w\ntriggers:\n  - {trigger}\n---\n");
        let wf = crate::workflow::parse(Path::new("w.md"), &text).unwrap();
        Armed {
            workflow_path: "w.md".into(),
            name: "w".into(),
            project: Some(root.to_path_buf()),
            index: 0,
            trigger: wf.frontmatter.triggers[0].clone(),
        }
    }

    fn scan(a: Armed) -> Scan {
        Scan {
            armed: vec![a],
            ..Default::default()
        }
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn paths(fired: &[(Armed, Event)]) -> Vec<String> {
        fired
            .iter()
            .flat_map(|(_, e)| e.paths.iter().map(|(p, e)| format!("{p} {}", e.as_str())))
            .collect()
    }

    /// Every way a trigger can watch: polling, one recursive OS watch, and
    /// one OS watch per directory.
    fn watchers() -> Vec<(&'static str, Watcher)> {
        let w = |mode, per_dir| Watcher {
            states: HashMap::new(),
            mode,
            per_dir,
        };
        vec![
            ("poll", w(Mode::Poll, false)),
            ("os", w(Mode::Os, false)),
            ("os per dir", w(Mode::Os, true)),
        ]
    }

    /// Give OS events time to arrive.
    fn settle() {
        std::thread::sleep(Duration::from_millis(500));
    }

    fn watching(w: &Watcher) -> bool {
        w.states
            .values()
            .all(|s| matches!(s.source, Source::Os(_)) == (w.mode == Mode::Os))
    }

    #[test]
    fn changes_are_batched_filtered_and_debounced() {
        for (name, mut w) in watchers() {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().canonicalize().unwrap();
            write(&root.join("specs/old.md"), "x");
            let s = scan(armed(
                &root,
                "file: \"specs/**/*.md\"\n    debounce: 2\n    ignore: [\"specs/drafts/**\"]",
            ));
            let t0 = Instant::now();
            let at = |secs: u64| t0 + Duration::from_secs(secs);
            assert!(
                w.poll(&s, at(0), |_| false).is_empty(),
                "{name}: arming fires nothing"
            );
            assert!(watching(&w), "{name}");

            write(&root.join("specs/a.md"), "a");
            write(&root.join("specs/deep/b.md"), "b");
            write(&root.join("specs/drafts/c.md"), "c");
            write(&root.join("specs/notes.txt"), "n");
            write(&root.join(".tome/worktrees/1/specs/d.md"), "d");
            settle();
            assert!(
                w.poll(&s, at(1), |_| false).is_empty(),
                "{name}: debouncing"
            );
            write(&root.join("specs/old.md"), "changed");
            settle();
            assert!(
                w.poll(&s, at(2), |_| false).is_empty(),
                "{name}: a new change restarts the window"
            );
            let fired = w.poll(&s, at(4), |_| false);
            assert_eq!(
                paths(&fired),
                [
                    "specs/a.md created",
                    "specs/deep/b.md created",
                    "specs/old.md modified"
                ],
                "{name}"
            );
            assert!(
                w.poll(&s, at(10), |_| false).is_empty(),
                "{name}: fires once"
            );

            // Muted: changes while the workflow runs are dropped, and don't
            // fire later.
            write(&root.join("specs/e.md"), "e");
            settle();
            assert!(w.poll(&s, at(11), |_| true).is_empty(), "{name}");
            settle();
            assert!(w.poll(&s, at(20), |_| false).is_empty(), "{name}");

            // Deletes never fire; a file moved into the glob is created.
            fs::remove_file(root.join("specs/a.md")).unwrap();
            write(&root.join("elsewhere/f.md"), "f");
            fs::rename(root.join("elsewhere/f.md"), root.join("specs/f.md")).unwrap();
            settle();
            w.poll(&s, at(21), |_| false);
            assert_eq!(
                paths(&w.poll(&s, at(30), |_| false)),
                ["specs/f.md created"],
                "{name}"
            );
            assert!(watching(&w), "{name}");
        }
    }

    #[test]
    fn moved_in_directories_count_as_created() {
        for (name, mut w) in watchers() {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().canonicalize().unwrap();
            fs::create_dir_all(root.join("docs")).unwrap();
            let s = scan(armed(&root, "file: \"docs/**/*.md\"\n    debounce: 0"));
            let now = Instant::now();
            w.poll(&s, now, |_| false);
            write(&root.join("staging/sub/a.md"), "a");
            write(&root.join("staging/sub/deeper/b.md"), "b");
            fs::rename(root.join("staging"), root.join("docs/new")).unwrap();
            settle();
            assert_eq!(
                paths(&w.poll(&s, now, |_| false)),
                [
                    "docs/new/sub/a.md created",
                    "docs/new/sub/deeper/b.md created"
                ],
                "{name}"
            );
            // Files in the new directories are watched too.
            write(&root.join("docs/new/sub/deeper/c.md"), "c");
            settle();
            assert_eq!(
                paths(&w.poll(&s, now, |_| false)),
                ["docs/new/sub/deeper/c.md created"],
                "{name}"
            );
            assert!(watching(&w), "{name}");
        }
    }

    #[test]
    fn dropped_os_events_are_caught_by_a_rescan() {
        for (name, mut w) in watchers().into_iter().filter(|(_, w)| w.mode == Mode::Os) {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().canonicalize().unwrap();
            write(&root.join("docs/old.md"), "x");
            write(&root.join("docs/gone/g.md"), "g");
            let s = scan(armed(&root, "file: \"docs/**/*.md\"\n    debounce: 0"));
            let now = Instant::now();
            w.poll(&s, now, |_| false);
            // Swap in a channel the OS never writes to, so every real event
            // is lost, then report the loss.
            let (tx, rx) = mpsc::channel();
            let Some(Source::Os(os)) = w.states.values_mut().next().map(|s| &mut s.source) else {
                panic!("{name}: not watching")
            };
            os.events = rx;
            std::thread::sleep(Duration::from_millis(20));
            write(&root.join("docs/old.md"), "changed");
            write(&root.join("docs/new/a.md"), "a");
            fs::remove_dir_all(root.join("docs/gone")).unwrap();
            tx.send(Ok(
                notify::Event::new(EventKind::Other).set_flag(notify::event::Flag::Rescan)
            ))
            .unwrap();
            assert_eq!(
                paths(&w.poll(&s, now, |_| false)),
                ["docs/new/a.md created", "docs/old.md modified"],
                "{name}"
            );
            let Some(Source::Os(os)) = w.states.values().next().map(|s| &s.source) else {
                panic!("{name}: not watching")
            };
            assert!(
                os.dirs.contains(&root.join("docs/new")),
                "{name}: new dirs are watched"
            );
            assert!(
                !os.dirs.contains(&root.join("docs/gone")),
                "{name}: gone dirs are forgotten"
            );
        }
    }

    #[test]
    fn tome_file_watch_picks_the_mode() {
        assert_eq!(Mode::parse(None), Mode::Os);
        assert_eq!(Mode::parse(Some("os")), Mode::Os);
        assert_eq!(Mode::parse(Some("poll")), Mode::Poll);
        assert_eq!(
            Mode::parse(Some("inotify")),
            Mode::Os,
            "unknown values use OS events"
        );
        let dir = tempfile::tempdir().unwrap();
        let s = scan(armed(dir.path(), "file: \"*.md\""));
        let mut w = Watcher {
            states: HashMap::new(),
            mode: Mode::Poll,
            per_dir: false,
        };
        w.poll(&s, Instant::now(), |_| false);
        assert_eq!(w.polling().into_values().collect::<Vec<_>>(), [FORCED]);
    }

    #[test]
    fn a_failed_os_watch_falls_back_to_polling() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        fs::create_dir_all(root.join("docs")).unwrap();
        let s = scan(armed(&root, "file: \"docs/*.md\"\n    debounce: 0"));
        let mut w = Watcher::default();
        let now = Instant::now();
        w.poll(&s, now, |_| false);
        assert!(w.polling().is_empty());
        // The OS watch stops: its sender is gone.
        let Some(Source::Os(os)) = w.states.values_mut().next().map(|s| &mut s.source) else {
            panic!("not watching")
        };
        os.events = mpsc::channel().1;
        write(&root.join("docs/a.md"), "a");
        assert_eq!(
            paths(&w.poll(&s, now, |_| false)),
            ["docs/a.md created"],
            "changes the OS missed are caught"
        );
        assert_eq!(
            w.polling().into_values().collect::<Vec<_>>(),
            ["the OS watch stopped"]
        );
        write(&root.join("docs/b.md"), "b");
        assert_eq!(
            paths(&w.poll(&s, now, |_| false)),
            ["docs/b.md created"],
            "polling keeps working"
        );
        assert_eq!(w.polling().len(), 1, "no retries");
    }

    #[test]
    fn a_missing_base_is_waited_for() {
        for (name, mut w) in watchers() {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().canonicalize().unwrap();
            let s = scan(armed(&root, "file: \"docs/specs/*.md\"\n    debounce: 0"));
            let now = Instant::now();
            assert!(w.poll(&s, now, |_| false).is_empty(), "{name}");
            if let Some(Source::Os(os)) = w.states.values().next().map(|s| &s.source) {
                assert!(
                    os.waiting && os.root == root,
                    "{name}: the nearest ancestor is watched"
                );
            }
            fs::create_dir_all(root.join("docs")).unwrap();
            settle();
            assert!(w.poll(&s, now, |_| false).is_empty(), "{name}");
            // The base appears with a file already in it.
            write(&root.join("staging/old.md"), "old");
            fs::rename(root.join("staging"), root.join("docs/specs")).unwrap();
            settle();
            let fired = paths(&w.poll(&s, now, |_| false));
            if w.mode == Mode::Os {
                assert!(
                    fired.is_empty(),
                    "{name}: the base's files are indexed without firing"
                );
            }
            write(&root.join("docs/specs/a.md"), "a");
            settle();
            assert_eq!(
                paths(&w.poll(&s, now, |_| false)),
                ["docs/specs/a.md created"],
                "{name}"
            );
            assert!(watching(&w), "{name}");

            // Deleted, then back again.
            fs::remove_dir_all(root.join("docs")).unwrap();
            settle();
            assert!(
                w.poll(&s, now, |_| false).is_empty(),
                "{name}: deletes never fire"
            );
            if let Some(Source::Os(os)) = w.states.values().next().map(|s| &s.source) {
                assert!(
                    os.waiting && os.root == root,
                    "{name}: the nearest ancestor is watched"
                );
            }
            fs::create_dir_all(root.join("docs/specs")).unwrap();
            settle();
            w.poll(&s, now, |_| false);
            write(&root.join("docs/specs/b.md"), "b");
            settle();
            assert_eq!(
                paths(&w.poll(&s, now, |_| false)),
                ["docs/specs/b.md created"],
                "{name}"
            );
            assert!(watching(&w), "{name}");
        }
    }

    #[test]
    fn on_filters_events_and_gitignored_paths_are_skipped() {
        for (name, mut w) in watchers() {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().canonicalize().unwrap();
            let git = Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["init", "-q"])
                .status();
            let has_git = git.is_ok_and(|s| s.success());
            write(&root.join(".gitignore"), "gen/\n*.tmp.md\n");
            write(&root.join("a.md"), "a");
            fs::create_dir_all(root.join("gen")).unwrap();
            let s = scan(armed(
                &root,
                "file: \"**/*.md\"\n    on: created\n    debounce: 0",
            ));
            let now = Instant::now();
            w.poll(&s, now, |_| false);
            std::thread::sleep(Duration::from_millis(20));
            write(&root.join("a.md"), "modified");
            write(&root.join("b.md"), "b");
            write(&root.join("gen/c.md"), "c");
            write(&root.join("x.tmp.md"), "x");
            settle();
            let fired = w.poll(&s, now, |_| false);
            if has_git {
                assert_eq!(paths(&fired), ["b.md created"], "{name}");
                if let Some(Source::Os(os)) = w.states.values().next().map(|s| &s.source) {
                    assert!(
                        !os.dirs.contains(&root.join("gen")),
                        "{name}: gitignored dirs aren't watched"
                    );
                }
            } else {
                assert!(
                    paths(&fired).contains(&"b.md created".to_string()),
                    "{name}"
                );
            }
        }
    }
}
