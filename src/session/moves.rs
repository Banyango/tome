//! Moving a live session somewhere else (`tome session move`) without
//! restarting it: its tmux pane or cmux surface moves, keeping its id, so
//! the agent in it carries on.
//!
//! A move that fails partway puts the session back where it was, as far as
//! the backend allows, and is an error.

use super::split::Opening;
use super::{workspace, Cmux, Kind, Layout, Split, Surface, Target, Tmux};
use crate::output::{CliError, CliResult};
use crate::paths;
use crate::store::Session;
use std::path::Path;
use std::process::Output;

/// Where a session moves to.
pub struct Move<'a> {
    /// Window (tmux) or tab/workspace (cmux) title.
    pub title: &'a str,
    pub layout: Layout,
    pub split: &'a Split,
    pub target: &'a Target,
    pub project: Option<&'a Path>,
}

/// Move `s` as `m` says. Returns its record with the new handle and layout
/// (the caller fills in the placement), and any placement warnings.
pub fn move_to(s: &Session, m: &Move) -> CliResult<(Session, Vec<String>)> {
    let pane = s.pane.as_deref().ok_or_else(|| {
        CliError::invalid(format!("session `{}` was started before tome tracked panes, so it can't be moved", s.name))
    })?;
    let mut warnings = Vec::new();
    let handle = match Kind::parse(&s.backend) {
        Some(Kind::Tmux) => Tmux { socket: s.socket.clone() }.move_pane(s, pane, m, &mut warnings)?,
        Some(Kind::Cmux) => Some(Cmux.move_surface(s, pane, m, &mut warnings)?),
        None => return Err(CliError::invalid(format!("session `{}` has an unknown backend `{}`", s.name, s.backend))),
    };
    let moved = Session { handle, layout: Some(m.layout.as_str().to_string()), ..s.clone() };
    Ok((moved, warnings))
}

fn home(m: &Move) -> std::path::PathBuf {
    m.project.map(Path::to_path_buf).unwrap_or_else(paths::user_home)
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).trim().to_string()
}

// --- tmux ------------------------------------------------------------------

impl Tmux {
    /// Move `pane` (session `s`'s): into a tmux session of its own named
    /// like `s`, a new window of the target session, or a new pane there.
    /// Returns the tome session it's in (`None` for its own).
    fn move_pane(&self, s: &Session, pane: &str, m: &Move, warnings: &mut Vec<String>) -> CliResult<Option<String>> {
        let failed = |what: &str, out: &Output| {
            CliError::internal(format!("tmux couldn't move `{}` {what}: {}; it was left where it was", s.name, stderr(out)))
        };
        if m.layout == Layout::Workspace {
            if Layout::of(s) == Layout::Workspace {
                return Ok(None);
            }
            // A placeholder window holds the new session open until the
            // pane arrives.
            let out = self.run(&[
                "new-session", "-d", "-P", "-F", "#{session_id} #{window_id}", "-s", &s.name, "-n", "tome-move", "-x", "200", "-y",
                "50",
            ])?;
            if !out.status.success() {
                return Err(failed("to a session of its own", &out));
            }
            let ids = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let (sid, placeholder) = ids.split_once(' ').unwrap_or((&ids, ""));
            let out = self.run(&["break-pane", "-d", "-s", pane, "-t", &format!("{sid}:"), "-n", m.title])?;
            if !out.status.success() {
                let _ = self.run(&["kill-session", "-t", sid]);
                return Err(failed("to a session of its own", &out));
            }
            let _ = self.run(&["kill-window", "-t", placeholder]);
            self.keep_title(pane);
            return Ok(None);
        }
        let _held = workspace::lock();
        let places = workspace::Places::open();
        let id = self.workspace(&places, m.target, m.project, &home(m), warnings)?;
        let label = format!("to {}", m.target.label(m.project));
        match m.layout {
            Layout::Split => {
                let (out, anchored) = self.split_window(&id, m.split, Opening::Join(pane), warnings)?;
                if !out.status.success() {
                    return Err(failed(&label, &out));
                }
                self.even_out(pane, m.split, anchored);
                let _ = self.run(&["select-pane", "-t", pane, "-T", m.title]);
            }
            _ => {
                let out = self.run(&["break-pane", "-d", "-s", pane, "-t", &format!("{id}:"), "-n", m.title])?;
                if !out.status.success() {
                    return Err(failed(&label, &out));
                }
                self.keep_title(pane);
            }
        }
        Ok(Some(id))
    }

    /// Keep a window's title; agents like to set their own.
    fn keep_title(&self, pane: &str) {
        let _ = self.run(&["set-option", "-w", "-t", pane, "automatic-rename", "off"]);
        let _ = self.run(&["set-option", "-w", "-t", pane, "allow-rename", "off"]);
    }
}

// --- cmux ------------------------------------------------------------------

impl Cmux {
    /// Move `surface` (session `s`'s): to a workspace of its own, a tab of
    /// a pane in the target workspace, or a new split there. Returns the
    /// workspace it's in. A workspace of its own it leaves empty is closed.
    fn move_surface(&self, s: &Session, surface: &str, m: &Move, warnings: &mut Vec<String>) -> CliResult<String> {
        let all = self.surfaces().ok_or_else(|| CliError::internal("cmux isn't answering"))?;
        let me = all
            .iter()
            .find(|x| x.id == surface)
            .ok_or_else(|| CliError::invalid(format!("session `{}` isn't open in cmux", s.name)))?;
        let (old_workspace, old_pane) = (me.workspace.clone(), me.pane.clone());
        let own = Layout::of(s) == Layout::Workspace;
        if own && m.layout == Layout::Workspace {
            return Ok(old_workspace);
        }
        let moved = self.move_surface_to(surface, &old_workspace, m, warnings);
        match &moved {
            Ok(workspace) if own && *workspace != old_workspace => {
                let empty = self.surfaces().is_some_and(|all| all.iter().all(|x| x.workspace != old_workspace || x.id.is_empty()));
                if empty {
                    self.kill(&old_workspace);
                }
            }
            Ok(_) => {}
            // Put it back.
            Err(_) => {
                if let Some(pane) = old_pane {
                    let _ = self.run(&["move-surface", "--surface", surface, "--pane", &pane, "--focus", "false"]);
                }
            }
        }
        moved.map_err(|e| CliError { message: format!("{}; it was put back where it was", e.message), ..e })
    }

    fn move_surface_to(&self, surface: &str, old_workspace: &str, m: &Move, warnings: &mut Vec<String>) -> CliResult<String> {
        let step = |what: &str, args: &[&str]| -> CliResult<Output> {
            let out = self.run(args)?;
            if !out.status.success() {
                return Err(CliError::internal(format!("cmux couldn't move the session {what}: {}", stderr(&out))));
            }
            Ok(out)
        };
        if m.layout == Layout::Workspace {
            step(
                "to a workspace of its own",
                &[
                    "--id-format", "uuids", "move-tab-to-new-workspace", "--surface", surface, "--workspace", old_workspace, "--title",
                    m.title, "--focus", "false",
                ],
            )?;
            return self
                .surfaces()
                .and_then(|all| all.into_iter().find(|x| x.id == surface))
                .map(|x| x.workspace)
                .ok_or_else(|| CliError::internal("cmux moved the session but tome couldn't find its new workspace"));
        }
        let _held = workspace::lock();
        let places = workspace::Places::open();
        let (mut record, kept) = self.workspace(&places, m.target, m.project, &home(m), warnings)?;
        let what = format!("to {}", m.target.label(m.project));
        let split = m.split;
        let here: Vec<Surface> = self
            .surfaces()
            .unwrap_or_default()
            .into_iter()
            .filter(|x| x.workspace == record.id && !x.id.is_empty() && x.id != surface)
            .collect();
        let pane_of = |id: &str| here.iter().find(|x| x.id == id).and_then(|x| x.pane.clone());
        let tabs = match kept {
            true => record.tabs.clone().filter(|p| here.iter().any(|x| x.pane.as_deref() == Some(p.as_str()))),
            false => split.recent.iter().find_map(|a| pane_of(&a.pane)),
        };
        let anchor = split.anchors.iter().find_map(|a| here.iter().find(|x| a.handle == record.id && x.id == a.pane));
        split.missing_anchor(anchor.is_some(), warnings);
        let join = anchor.and_then(|a| a.pane.clone()).filter(|_| m.layout == Layout::Tab);
        match (m.layout, join.or(tabs)) {
            (Layout::Tab, Some(pane)) => {
                step(&what, &["move-surface", "--surface", surface, "--pane", &pane, "--focus", "false"])?;
            }
            _ => {
                // Into the anchor's pane (else the last one), then out of it
                // as a new split.
                match anchor.or(here.last()).and_then(|x| x.pane.as_deref()) {
                    Some(pane) => step(&what, &["move-surface", "--surface", surface, "--pane", pane, "--focus", "false"])?,
                    None => step(&what, &["move-surface", "--surface", surface, "--workspace", &record.id, "--focus", "false"])?,
                };
                step(
                    &what,
                    &[
                        "drag-surface-to-split", "--surface", surface, split.direction.as_str(), "--workspace", &record.id, "--focus",
                        "false",
                    ],
                )?;
                self.size_pane(&record.id, surface, split, warnings);
                if m.layout == Layout::Tab && kept {
                    record.tabs = self.surfaces().and_then(|all| all.into_iter().find(|x| x.id == surface)?.pane);
                    places.put(&record)?;
                }
            }
        }
        let _ = self.run(&["rename-tab", "--workspace", &record.id, "--surface", surface, m.title]);
        Ok(record.id)
    }
}
