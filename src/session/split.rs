//! How a new pane opens: which way it splits, how big it is, and which of
//! the run's panes it opens from (`split.direction`, `split.size`, `from`).
//!
//! Size is best effort. tmux sets it as it splits; cmux resizes the new pane
//! afterwards. A size that can't be honoured is clamped or skipped, with a
//! warning recorded on the session; only failing to open the pane is an
//! error.

use super::{Cmux, Tmux};
use crate::placement::{Direction, From, Size};
use crate::store::Session;
use serde_json::Value;

/// The largest and smallest share of the space a split may take.
const MAX_PERCENT: u32 = 90;
const MIN_PERCENT: u32 = 10;

/// A pane of the run that a new one may open from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    /// The workspace (cmux) or tome session (tmux) it's in.
    pub handle: String,
    /// Its tmux pane or cmux surface.
    pub pane: String,
}

/// How a new pane opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split {
    pub direction: Direction,
    pub size: Option<Size>,
    pub from: Option<From>,
    /// With `from`, the run's panes to open from, best first: the one it
    /// names, then the last, then the first. The first one still open in
    /// the target workspace is used.
    pub anchors: Vec<Anchor>,
    /// All the run's panes, newest first (for a tab in a workspace tome
    /// doesn't keep a tab split in).
    pub recent: Vec<Anchor>,
}

impl Default for Split {
    fn default() -> Split {
        Split {
            direction: Direction::Right,
            size: None,
            from: None,
            anchors: Vec::new(),
            recent: Vec::new(),
        }
    }
}

impl Split {
    /// A split opening from `from` among the run's recorded `sessions`
    /// (oldest first).
    pub fn new(
        direction: Direction,
        size: Option<Size>,
        from: Option<From>,
        sessions: &[Session],
    ) -> Split {
        let placed: Vec<&Session> = sessions
            .iter()
            .filter(|s| s.handle.is_some() && s.pane.is_some())
            .collect();
        let anchor = |s: &&Session| Anchor {
            handle: s.handle.clone().unwrap(),
            pane: s.pane.clone().unwrap(),
        };
        let mut anchors: Vec<Anchor> = Vec::new();
        // The caller's pane isn't one of the run's; launches add it.
        if let Some(from) = from.filter(|f| *f != From::Caller) {
            if from == From::Orchestrator {
                anchors.extend(
                    placed
                        .iter()
                        .filter(|s| crate::orchestrator::is_main(&s.role))
                        .map(anchor),
                );
            }
            // `last` falls back to earlier ones, down to the first.
            let ordered: Box<dyn Iterator<Item = &&Session>> = if from == From::First {
                Box::new(placed.iter())
            } else {
                Box::new(placed.iter().rev())
            };
            for a in ordered.map(anchor) {
                if !anchors.contains(&a) {
                    anchors.push(a);
                }
            }
        }
        let recent = placed.iter().rev().map(anchor).collect();
        Split {
            direction,
            size,
            from,
            anchors,
            recent,
        }
    }

    pub(super) fn horizontal(&self) -> bool {
        matches!(self.direction, Direction::Right | Direction::Left)
    }

    /// Note that `from` found no pane to open from.
    pub(super) fn missing_anchor(&self, found: bool, warnings: &mut Vec<String>) {
        if let (Some(From::Caller), false) = (self.from, found) {
            warnings.push("from: caller: the caller's pane isn't open there any more; opened in the default place".to_string());
        } else if let (Some(from), false) = (self.from, found) {
            warnings.push(format!("from: {from}: none of the run's panes is open in this workspace; opened in the default place"));
        }
    }
}

/// A percentage kept within bounds.
fn percent(n: u32, warnings: &mut Vec<String>) -> u32 {
    let kept = n.clamp(MIN_PERCENT, MAX_PERCENT);
    if kept != n {
        warnings.push(format!(
            "split.size {n}% is outside {MIN_PERCENT}%..{MAX_PERCENT}%; clamped to {kept}%"
        ));
    }
    kept
}

/// A size in cells kept within the `available` cells, leaving room for the
/// pane it splits off; `None` (skipped) if there's no room at all.
fn cells(n: u32, available: u32, warnings: &mut Vec<String>) -> Option<u32> {
    let max = available.saturating_sub(2);
    if max == 0 {
        warnings.push(format!(
            "split.size {n}: there are only {available} cells to split; skipped"
        ));
        return None;
    }
    if n > max {
        warnings.push(format!(
            "split.size {n} is more than the {available} cells there are; clamped to {max}"
        ));
        return Some(max);
    }
    Some(n)
}

// --- tmux ------------------------------------------------------------------

/// What a split opens: a new pane running `argv`, or a pane already open
/// elsewhere, moved there.
#[derive(Clone, Copy)]
pub(super) enum Opening<'a> {
    New { cwd: &'a str, argv: &'a [&'a str] },
    Join(&'a str),
}

impl Tmux {
    /// The first of `split`'s anchors that's still a pane of session `id`.
    pub(super) fn anchor<'a>(&self, id: &str, split: &'a Split) -> Option<&'a str> {
        split
            .anchors
            .iter()
            .find(|a| a.handle == id && self.pane_exists(&a.pane))
            .map(|a| a.pane.as_str())
    }

    /// Open a pane in session `id` as `split` says: off the anchor pane,
    /// else along the whole first window's edge. Returns the `split-window`
    /// (or `join-pane`) output and whether it opened off an anchor.
    pub(super) fn split_window(
        &self,
        id: &str,
        split: &Split,
        opening: Opening,
        warnings: &mut Vec<String>,
    ) -> std::io::Result<(std::process::Output, bool)> {
        let anchor = self.anchor(id, split);
        split.missing_anchor(anchor.is_some(), warnings);
        let (target, whole) = match anchor {
            Some(pane) => (pane.to_string(), false),
            None => (format!("{id}:^"), true),
        };
        let size = split.size.and_then(|size| match size {
            Size::Percent(n) => Some(format!("{}%", percent(n, warnings))),
            Size::Cells(n) => {
                let available = self.dimension(&target, whole, split.horizontal())?;
                cells(n, available, warnings).map(|n| n.to_string())
            }
        });
        let run = |size: Option<&str>| {
            let verb = match opening {
                Opening::New { .. } => "split-window",
                Opening::Join(_) => "join-pane",
            };
            let mut args = vec![verb, "-d", if split.horizontal() { "-h" } else { "-v" }];
            if matches!(split.direction, Direction::Left | Direction::Up) {
                args.push("-b");
            }
            if whole {
                args.push("-f");
            }
            args.extend(["-t", &target]);
            if let Some(size) = size {
                args.extend(["-l", size]);
            }
            match opening {
                Opening::New { cwd, argv } => {
                    args.extend(["-c", cwd, "-P", "-F", "#{pane_id}"]);
                    args.extend_from_slice(argv);
                }
                Opening::Join(pane) => args.extend(["-s", pane]),
            }
            self.run(&args)
        };
        let out = run(size.as_deref())?;
        let out = match size {
            Some(size) if !out.status.success() => {
                warnings.push(format!(
                    "split.size {size} couldn't be set ({}); skipped",
                    String::from_utf8_lossy(&out.stderr).trim()
                ));
                run(None)?
            }
            _ => out,
        };
        Ok((out, anchor.is_some()))
    }

    /// Spread the panes of `pane`'s window evenly along the split's axis
    /// (008's `split`), unless the split was sized or anchored.
    pub(super) fn even_out(&self, pane: &str, split: &Split, anchored: bool) {
        if split.size.is_none() && !anchored {
            let layout = if split.horizontal() {
                "even-horizontal"
            } else {
                "even-vertical"
            };
            let _ = self.run(&["select-layout", "-t", pane, layout]);
        }
    }

    /// The width (or height) of `target`'s window, or of the pane itself.
    fn dimension(&self, target: &str, whole: bool, horizontal: bool) -> Option<u32> {
        let format = match (whole, horizontal) {
            (true, true) => "#{window_width}",
            (true, false) => "#{window_height}",
            (false, true) => "#{pane_width}",
            (false, false) => "#{pane_height}",
        };
        let out = self
            .run(&["display-message", "-p", "-t", target, format])
            .ok()
            .filter(|o| o.status.success())?;
        String::from_utf8_lossy(&out.stdout).trim().parse().ok()
    }
}

// --- cmux ------------------------------------------------------------------

/// A cmux pane's size along one axis, in points.
struct Extent {
    /// The whole workspace's.
    container: f64,
    pane: f64,
    /// One cell's.
    cell: f64,
}

impl Cmux {
    /// Resize the pane holding `surface` to `split.size`, as far as cmux
    /// allows. cmux resizes by an amount rather than to a size, so this
    /// measures, resizes, and corrects once.
    pub(super) fn size_pane(
        &self,
        workspace: &str,
        surface: &str,
        split: &Split,
        warnings: &mut Vec<String>,
    ) {
        let Some(size) = split.size else { return };
        let Some(pane) = self
            .surfaces()
            .and_then(|all| all.into_iter().find(|s| s.id == surface)?.pane)
        else {
            warnings.push(format!(
                "split.size {size}: cmux didn't report the new pane; skipped"
            ));
            return;
        };
        let horizontal = split.horizontal();
        let Some(before) = self.extent(workspace, &pane, horizontal) else {
            warnings.push(format!("split.size {size}: cmux has no geometry for this workspace until it's been shown; skipped"));
            return;
        };
        let target = match size {
            Size::Percent(n) => before.container * f64::from(percent(n, warnings)) / 100.0,
            Size::Cells(n) => match cells(n, (before.container / before.cell) as u32, warnings) {
                Some(n) => f64::from(n) * before.cell,
                None => return,
            },
        };
        // The flag that moves the new pane's inner edge outwards.
        let (grow, shrink) = match split.direction {
            Direction::Right => ("-L", "-R"),
            Direction::Left => ("-R", "-L"),
            Direction::Down => ("-U", "-D"),
            Direction::Up => ("-D", "-U"),
        };
        let resize = |grow_by: f64| {
            let flag = if grow_by > 0.0 { grow } else { shrink };
            let amount = (grow_by.abs().round() as u64).max(1).to_string();
            self.run(&[
                "resize-pane",
                "--workspace",
                workspace,
                "--pane",
                &pane,
                flag,
                "--amount",
                &amount,
            ])
            .is_ok_and(|o| o.status.success())
        };
        let wanted = target - before.pane;
        if wanted.abs() < before.cell {
            return;
        }
        let after = if resize(wanted) {
            self.extent(workspace, &pane, horizontal)
        } else {
            None
        };
        let Some(after) = after.filter(|a| (a.pane - before.pane).abs() >= 1.0) else {
            warnings.push(format!(
                "split.size {size}: cmux didn't resize the pane; skipped"
            ));
            return;
        };
        // How far one unit of `grow` moved it: corrects both the unit and a
        // wrong guess at the direction.
        let per_unit = (after.pane - before.pane) / wanted;
        let remaining = target - after.pane;
        let last = if remaining.abs() >= before.cell && per_unit.abs() > f64::EPSILON {
            resize(remaining / per_unit);
            self.extent(workspace, &pane, horizontal).unwrap_or(after)
        } else {
            after
        };
        if (last.pane - target).abs() >= 2.0 * before.cell {
            warnings.push(format!(
                "split.size {size}: cmux sized the pane to {} cells instead of {}",
                (last.pane / before.cell).round(),
                (target / before.cell).round()
            ));
        }
    }

    /// `pane`'s size along the split axis; `None` if cmux didn't answer or
    /// has no geometry for the workspace (it's never been shown).
    fn extent(&self, workspace: &str, pane: &str, horizontal: bool) -> Option<Extent> {
        let out = self
            .run(&[
                "--id-format",
                "uuids",
                "--json",
                "list-panes",
                "--workspace",
                workspace,
            ])
            .ok()
            .filter(|o| o.status.success())?;
        let v: Value = serde_json::from_slice(&out.stdout).ok()?;
        let (length, cell) = if horizontal {
            ("width", "cell_width_points")
        } else {
            ("height", "cell_height_points")
        };
        let container = v["container_frame"][length].as_f64().filter(|c| *c > 0.0)?;
        let p = v["panes"]
            .as_array()?
            .iter()
            .find(|p| p["id"].as_str() == Some(pane))?;
        Some(Extent {
            container,
            pane: p["pixel_frame"][length].as_f64()?,
            cell: p[cell].as_f64().filter(|c| *c > 0.0)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(role: &str, pane: &str) -> Session {
        Session {
            run_id: 1,
            name: format!("tome-1-x-{role}"),
            role: role.into(),
            backend: "tmux".into(),
            socket: None,
            handle: Some("$1".into()),
            pane: Some(pane.into()),
            layout: None,
            harness: None,
            placement: None,
            agent_status: None,
            blocked_at: None,
            created_at: String::new(),
        }
    }

    #[test]
    fn anchors_fall_back_to_the_last_then_the_first() {
        let all = [
            session("orchestrator", "%1"),
            session("worker", "%2"),
            session("worker", "%3"),
        ];
        let panes = |from| {
            Split::new(Direction::Right, None, Some(from), &all)
                .anchors
                .into_iter()
                .map(|a| a.pane)
                .collect::<Vec<_>>()
        };
        assert_eq!(panes(From::Orchestrator), ["%1", "%3", "%2"]);
        assert_eq!(panes(From::Last), ["%3", "%2", "%1"]);
        assert_eq!(panes(From::First), ["%1", "%2", "%3"]);
        assert!(Split::new(Direction::Right, None, None, &all)
            .anchors
            .is_empty());
    }

    #[test]
    fn sizes_are_clamped_with_a_warning() {
        let mut w = Vec::new();
        assert_eq!(percent(30, &mut w), 30);
        assert!(w.is_empty());
        assert_eq!(percent(95, &mut w), 90);
        assert_eq!(cells(80, 200, &mut w), Some(80));
        assert_eq!(cells(500, 200, &mut w), Some(198));
        assert_eq!(cells(5, 2, &mut w), None);
        assert_eq!(w.len(), 3, "{w:?}");
    }
}
