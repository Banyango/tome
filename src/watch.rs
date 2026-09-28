//! File triggers: a polling watcher over each armed file trigger's glob.
//!
//! Every tick walks the directories the glob could match in (never `.git/`,
//! `.tome/`, gitignored directories or the trigger's `ignore:` globs), diffs
//! file stats against the previous walk, and turns changes into created /
//! modified events. Changes are batched until the debounce window passes
//! with no new ones, then fire as one event. The first walk after arming is
//! a baseline and fires nothing.

use crate::arming::{Armed, Scan};
use crate::glob::Glob;
use crate::triggers::Event;
use crate::workflow::{FileEvent, FileTrigger, TriggerKind};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Instant, SystemTime};

/// Never watched, at any depth.
const SKIPPED_DIRS: &[&str] = &[".git", ".tome"];

type Stat = (Option<SystemTime>, u64);

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
        let ignore = f.ignore.iter().filter_map(|g| Glob::new(&expand(g)).ok()).collect();
        let literal: Vec<&str> = pattern.split('/').collect();
        let literal = &literal[..literal.len().saturating_sub(1)];
        let mut base = match &root {
            Some(r) => r.clone(),
            None => PathBuf::from("/"),
        };
        for seg in literal.iter().take_while(|s| !s.contains(['*', '?', '[', '{', '\\'])) {
            if !seg.is_empty() {
                base.push(seg);
            }
        }
        Some(Spec { base, root, glob, ignore })
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
}

fn expand(pattern: &str) -> String {
    match pattern.strip_prefix("~/") {
        Some(rest) => format!("{}/{rest}", crate::paths::user_home().display()),
        None => pattern.to_string(),
    }
}

struct State {
    spec: Option<Spec>,
    baseline: bool,
    seen: HashMap<PathBuf, Stat>,
    pending: Vec<(PathBuf, FileEvent)>,
    last_change: Option<Instant>,
    /// Gitignore answers for directories, asked once.
    ignored_dirs: HashMap<PathBuf, bool>,
}

#[derive(Default)]
pub struct Watcher {
    states: HashMap<String, State>,
}

impl Watcher {
    /// Walk every armed file trigger and return those whose batched changes
    /// are ready to fire. `muted` says whether a trigger is muted right now
    /// (`while_running: mute` and its workflow has a run going); changes
    /// while it is are dropped.
    pub fn poll(&mut self, scan: &Scan, now: Instant, muted: impl Fn(&Armed) -> bool) -> Vec<(Armed, Event)> {
        let mut out = Vec::new();
        let mut states = std::mem::take(&mut self.states);
        let mut kept = HashMap::new();
        for a in &scan.armed {
            let TriggerKind::File(f) = &a.trigger.kind else { continue };
            let key = a.key();
            let mut st = states.remove(&key).unwrap_or_else(|| State {
                spec: Spec::new(a, f),
                baseline: false,
                seen: HashMap::new(),
                pending: Vec::new(),
                last_change: None,
                ignored_dirs: HashMap::new(),
            });
            if let Some(event) = st.poll(a, f, scan, now, &muted) {
                out.push((a.clone(), event));
            }
            kept.insert(key, st);
        }
        self.states = kept;
        out
    }
}

impl State {
    fn poll(&mut self, a: &Armed, f: &FileTrigger, scan: &Scan, now: Instant, muted: &impl Fn(&Armed) -> bool) -> Option<Event> {
        let spec = self.spec.as_ref()?;
        let files = walk(spec, &mut self.ignored_dirs);
        let before = std::mem::replace(&mut self.seen, files);
        if !self.baseline {
            self.baseline = true;
            return None;
        }
        let mut changes: Vec<(PathBuf, FileEvent)> = Vec::new();
        for (path, stat) in &self.seen {
            match before.get(path) {
                None => changes.push((path.clone(), FileEvent::Created)),
                Some(old) if old != stat => changes.push((path.clone(), FileEvent::Modified)),
                _ => {}
            }
        }
        changes.retain(|(p, e)| f.on.contains(e) && !(a.project.is_none() && scan.shadowed(&a.name, p)));
        if !changes.is_empty() {
            let ignored = git_ignored(&spec.base, changes.iter().map(|(p, _)| p.as_path()));
            changes.retain(|(p, _)| !ignored.contains(p));
        }
        if !changes.is_empty() {
            if muted(a) {
                eprintln!("tome daemon: {} change(s) for {} dropped: its workflow is running", changes.len(), a.trigger.describe());
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
        let quiet = self.last_change.is_some_and(|t| now.duration_since(t) >= f.debounce);
        if self.pending.is_empty() || !quiet {
            return None;
        }
        let paths = std::mem::take(&mut self.pending).into_iter().map(|(p, e)| (spec.form(&p), e)).collect();
        Some(Event { paths, ..Default::default() })
    }
}

/// The files under `spec.base` the glob matches, with their stats.
fn walk(spec: &Spec, ignored_dirs: &mut HashMap<PathBuf, bool>) -> HashMap<PathBuf, Stat> {
    let mut out = HashMap::new();
    let mut dirs = vec![spec.base.clone()];
    while let Some(dir) = dirs.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else { continue };
        let mut subdirs = Vec::new();
        for entry in read.flatten() {
            let path = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            let form = spec.form(&path);
            if ft.is_dir() {
                let skipped = entry.file_name().to_str().is_some_and(|n| SKIPPED_DIRS.contains(&n));
                if !skipped && spec.glob.could_contain(&form) && !spec.ignored(&form) {
                    subdirs.push(path);
                }
                continue;
            }
            // Symlinked files count; symlinked directories aren't followed.
            let Ok(meta) = std::fs::metadata(&path) else { continue };
            if meta.is_file() && spec.glob.matches(&form) && !spec.ignored(&form) {
                out.insert(path, (meta.modified().ok(), meta.len()));
            }
        }
        let unknown: Vec<&Path> = subdirs.iter().filter(|d| !ignored_dirs.contains_key(*d)).map(PathBuf::as_path).collect();
        if !unknown.is_empty() {
            let ignored = git_ignored(&dir, unknown.iter().copied());
            for d in unknown {
                ignored_dirs.insert(d.to_path_buf(), ignored.contains(d));
            }
        }
        dirs.extend(subdirs.into_iter().filter(|d| ignored_dirs.get(d) != Some(&true)));
    }
    out
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
    let Ok(out) = child.wait_with_output() else { return HashSet::new() };
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
        Scan { armed: vec![a], ..Default::default() }
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn paths(fired: &[(Armed, Event)]) -> Vec<String> {
        fired.iter().flat_map(|(_, e)| e.paths.iter().map(|(p, e)| format!("{p} {}", e.as_str()))).collect()
    }

    #[test]
    fn changes_are_batched_filtered_and_debounced() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        write(&root.join("specs/old.md"), "x");
        let s = scan(armed(&root, "file: \"specs/**/*.md\"\n    debounce: 2\n    ignore: [\"specs/drafts/**\"]"));
        let mut w = Watcher::default();
        let t0 = Instant::now();
        let at = |secs: u64| t0 + Duration::from_secs(secs);
        assert!(w.poll(&s, at(0), |_| false).is_empty(), "baseline");

        write(&root.join("specs/a.md"), "a");
        write(&root.join("specs/deep/b.md"), "b");
        write(&root.join("specs/drafts/c.md"), "c");
        write(&root.join("specs/notes.txt"), "n");
        write(&root.join(".tome/worktrees/1/specs/d.md"), "d");
        assert!(w.poll(&s, at(1), |_| false).is_empty(), "debouncing");
        std::thread::sleep(Duration::from_millis(20));
        write(&root.join("specs/old.md"), "changed");
        assert!(w.poll(&s, at(2), |_| false).is_empty(), "a new change restarts the window");
        let fired = w.poll(&s, at(4), |_| false);
        assert_eq!(paths(&fired), ["specs/a.md created", "specs/deep/b.md created", "specs/old.md modified"]);
        assert!(w.poll(&s, at(10), |_| false).is_empty(), "fires once");

        // Muted: changes while the workflow runs are dropped.
        write(&root.join("specs/e.md"), "e");
        assert!(w.poll(&s, at(11), |_| true).is_empty());
        assert!(w.poll(&s, at(20), |_| false).is_empty());
    }

    #[test]
    fn on_filters_events_and_gitignored_paths_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let git = Command::new("git").arg("-C").arg(&root).args(["init", "-q"]).status();
        let has_git = git.is_ok_and(|s| s.success());
        write(&root.join(".gitignore"), "gen/\n*.tmp.md\n");
        write(&root.join("a.md"), "a");
        let s = scan(armed(&root, "file: \"**/*.md\"\n    on: created\n    debounce: 0"));
        let mut w = Watcher::default();
        let now = Instant::now();
        w.poll(&s, now, |_| false);
        std::thread::sleep(Duration::from_millis(20));
        write(&root.join("a.md"), "modified");
        write(&root.join("b.md"), "b");
        write(&root.join("gen/c.md"), "c");
        write(&root.join("x.tmp.md"), "x");
        let fired = w.poll(&s, now, |_| false);
        if has_git {
            assert_eq!(paths(&fired), ["b.md created"]);
        } else {
            assert!(paths(&fired).contains(&"b.md created".to_string()));
        }
    }
}
