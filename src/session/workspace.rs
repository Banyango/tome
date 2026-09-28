//! The project's tome workspace: one cmux workspace (or tmux session) per
//! project, named `<project>-orchestrator` (`global-orchestrator` for runs
//! outside a project), that the `tab` and `split` layouts put sessions in.
//!
//! It's created on first use with a plain shell in the project directory,
//! which keeps it open, and in its place in the sidebar, when no sessions
//! are left. tome records its id in `~/.tome/workspaces.json` and finds it
//! again by that, not by its title, so a rename doesn't lose it; if that id
//! is gone (the user closed it) a new one is created. Crash recovery kills
//! sessions by their own pane or tab, never the workspace.

use super::{Cmux, Tmux};
use crate::output::{CliError, CliResult};
use crate::paths;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

/// The tmux session option that marks a session as a tome workspace (its
/// value is [`Places::tag`]), so a recorded id that tmux handed out again
/// after a server restart isn't mistaken for it.
pub const TMUX_OPTION: &str = "@tome-workspace";

static LOCK: Mutex<()> = Mutex::new(());

/// Hold while finding (or creating) a tome workspace and placing a session
/// in it, so that launches at the same time don't each create one.
pub fn lock() -> MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

/// A tome workspace as recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// `tmux` or `cmux`.
    pub backend: String,
    /// The tmux server (`tmux -L`); `None` is the default one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socket: Option<String>,
    /// The project directory; `None` for runs outside a project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// The cmux workspace id, or the tmux session id (`$<n>`).
    pub id: String,
    /// cmux: the pane that holds the `tab` layout's tabs, once there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tabs: Option<String>,
}

impl Record {
    fn is_for(&self, backend: &str, socket: Option<&str>, project: Option<&Path>) -> bool {
        self.backend == backend
            && self.socket.as_deref() == socket
            && self.project.as_deref() == project.map(|p| p.to_string_lossy()).as_deref()
    }
}

/// The recorded tome workspaces.
pub struct Places {
    file: PathBuf,
}

impl Places {
    /// `~/.tome/workspaces.json`
    pub fn open() -> Places {
        Places { file: paths::tome_home().join("workspaces.json") }
    }

    fn load(&self) -> Vec<Record> {
        fs::read(&self.file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn find(&self, backend: &str, socket: Option<&str>, project: Option<&Path>) -> Option<Record> {
        self.load().into_iter().find(|r| r.is_for(backend, socket, project))
    }

    /// Record `record`, replacing the one for the same backend and project.
    pub fn put(&self, record: &Record) -> CliResult<()> {
        let mut all = self.load();
        all.retain(|r| !r.is_for(&record.backend, record.socket.as_deref(), record.project.as_deref().map(Path::new)));
        all.push(record.clone());
        if let Some(dir) = self.file.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = self.file.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(&all).map_err(|e| CliError::internal(e.to_string()))?)?;
        fs::rename(&tmp, &self.file)?;
        Ok(())
    }

    /// What marks a tmux session as this tome home's workspace for `project`.
    fn tag(&self, project: Option<&Path>) -> String {
        let project = project.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        format!("{}:{project}", self.file.display())
    }

    /// The tome session for `project` on `tmux`, created (with a plain shell
    /// in `cwd`) if it's gone. Returns its id.
    pub fn tmux(&self, tmux: &Tmux, project: Option<&Path>, cwd: &Path) -> CliResult<String> {
        let tag = self.tag(project);
        let socket = tmux.socket.as_deref();
        if let Some(r) = self.find("tmux", socket, project) {
            if tmux.tagged_sessions().iter().any(|(id, t)| *id == r.id && *t == tag) {
                return Ok(r.id);
            }
        }
        let id = tmux.new_tome_session(&name(project), cwd, &tag)?;
        let record = Record {
            backend: "tmux".into(),
            socket: socket.map(str::to_string),
            project: project.map(|p| p.to_string_lossy().into_owned()),
            id: id.clone(),
            tabs: None,
        };
        self.put(&record)?;
        Ok(id)
    }

    /// The tome workspace for `project` in cmux, opened (unfocused, with a
    /// plain shell in `cwd`) if it's gone.
    pub fn cmux(&self, cmux: &Cmux, project: Option<&Path>, cwd: &Path) -> CliResult<Record> {
        if let Some(r) = self.find("cmux", None, project) {
            if cmux.is_alive(&r.id) == Some(true) {
                return Ok(r);
            }
        }
        let record = Record {
            backend: "cmux".into(),
            socket: None,
            project: project.map(|p| p.to_string_lossy().into_owned()),
            id: cmux.new_tome_workspace(&name(project), cwd)?,
            tabs: None,
        };
        self.put(&record)?;
        Ok(record)
    }
}

/// `<project>-orchestrator`, or `global-orchestrator` outside a project.
/// tmux doesn't allow `.` or `:` in names, so anything but letters, digits,
/// `_` and `-` becomes `-`.
pub fn name(project: Option<&Path>) -> String {
    let base = project.and_then(Path::file_name).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "global".into());
    let slug: String = base.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '-' }).collect();
    format!("{slug}-orchestrator")
}

impl Tmux {
    /// Sessions marked as tome workspaces: `(id, tag)`.
    pub(super) fn tagged_sessions(&self) -> Vec<(String, String)> {
        let format = format!("#{{session_id}}\t#{{{TMUX_OPTION}}}");
        match self.run(&["list-sessions", "-F", &format]) {
            Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter_map(|l| l.split_once('\t'))
                .filter(|(_, tag)| !tag.is_empty())
                .map(|(id, tag)| (id.to_string(), tag.to_string()))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// A new detached session running a plain shell in `cwd`, named `name`
    /// (or `name-2`, ... if that's taken) and marked with `tag`. Returns its id.
    fn new_tome_session(&self, name: &str, cwd: &Path, tag: &str) -> CliResult<String> {
        if !Tmux::available() {
            return Err(CliError::internal("tmux isn't installed").with_hint("install tmux, or use `backend: cmux`"));
        }
        let cwd = cwd.to_string_lossy();
        let mut last = String::new();
        for n in 1..=20 {
            let candidate = if n == 1 { name.to_string() } else { format!("{name}-{n}") };
            let out = self.run(&[
                "new-session", "-d", "-P", "-F", "#{session_id}", "-s", &candidate, "-c", &cwd, "-x", "200", "-y", "50",
            ])?;
            if out.status.success() {
                let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let _ = self.run(&["set-option", "-t", &id, TMUX_OPTION, tag]);
                return Ok(id);
            }
            last = String::from_utf8_lossy(&out.stderr).trim().to_string();
            if !last.contains("duplicate session") {
                break;
            }
        }
        Err(CliError::internal(format!("tmux couldn't start session `{name}`: {last}")))
    }
}

impl Cmux {
    /// Open a workspace (in the background) with a plain shell in `cwd`, and
    /// return its id.
    fn new_tome_workspace(&self, name: &str, cwd: &Path) -> CliResult<String> {
        if !self.available() {
            return Err(self.unavailable());
        }
        let out = self.run(&["new-workspace", "--name", name, "--cwd", &cwd.to_string_lossy(), "--focus", "false"])?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        let reference = stdout.trim().strip_prefix("OK ").filter(|_| out.status.success()).ok_or_else(|| {
            CliError::internal(format!(
                "cmux couldn't open the `{name}` workspace: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        })?;
        match self.surfaces().and_then(|all| all.into_iter().find(|s| s.workspace_ref == reference)) {
            Some(s) => Ok(s.workspace),
            None => {
                let _ = self.run(&["close-workspace", "--workspace", reference]);
                Err(CliError::internal(format!("cmux opened {reference} but tome couldn't find its id")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::Server;
    use super::*;

    #[test]
    fn names_come_from_the_project_directory() {
        assert_eq!(name(Some(Path::new("/src/tome-cli"))), "tome-cli-orchestrator");
        assert_eq!(name(Some(Path::new("/src/my.app"))), "my-app-orchestrator");
        assert_eq!(name(None), "global-orchestrator");
    }

    #[test]
    fn the_tmux_session_is_created_once_and_found_by_id() {
        let Some(server) = Server::new("place") else { return };
        let tmux = &server.0;
        let dir = tempfile::tempdir().unwrap();
        let places = Places { file: dir.path().join("workspaces.json") };
        let project = dir.path().join("my.proj");
        fs::create_dir_all(&project).unwrap();

        let id = places.tmux(tmux, Some(&project), &project).unwrap();
        assert!(tmux.list().contains(&"my-proj-orchestrator".to_string()), "{:?}", tmux.list());
        assert_eq!(places.tmux(tmux, Some(&project), &project).unwrap(), id);

        // A rename doesn't lose it.
        assert!(tmux.run(&["rename-session", "-t", &id, "mine-now"]).unwrap().status.success());
        assert_eq!(places.tmux(tmux, Some(&project), &project).unwrap(), id);
        // Other projects (and runs outside one) get their own.
        let global = places.tmux(tmux, None, dir.path()).unwrap();
        assert_ne!(global, id);
        assert!(tmux.list().contains(&"global-orchestrator".to_string()));

        // Closed by the user: a new one is made.
        assert!(tmux.run(&["kill-session", "-t", &id]).unwrap().status.success());
        let again = places.tmux(tmux, Some(&project), &project).unwrap();
        assert_ne!(again, id);
        assert!(tmux.list().contains(&"my-proj-orchestrator".to_string()));
        assert_eq!(places.load().len(), 2);
    }

    #[test]
    fn a_session_that_merely_has_the_recorded_id_is_not_it() {
        let Some(server) = Server::new("reuse") else { return };
        let tmux = &server.0;
        let dir = tempfile::tempdir().unwrap();
        let places = Places { file: dir.path().join("workspaces.json") };
        let id = places.tmux(tmux, None, dir.path()).unwrap();
        // As if the server restarted and the user made a session that got
        // the same id.
        tmux.run(&["set-option", "-u", "-t", &id, TMUX_OPTION]).unwrap();
        let again = places.tmux(tmux, None, dir.path()).unwrap();
        assert_ne!(again, id);
        assert_eq!(tmux.list().len(), 2);
    }

    #[test]
    fn killing_a_run_s_leftover_sessions_spares_the_workspace() {
        let Some(server) = Server::new("spare") else { return };
        let tmux = &server.0;
        let dir = tempfile::tempdir().unwrap();
        let places = Places { file: dir.path().join("workspaces.json") };
        // A project whose name looks like run 7's sessions.
        let project = dir.path().join("tome-7-x");
        fs::create_dir_all(&project).unwrap();
        let id = places.tmux(tmux, Some(&project), &project).unwrap();
        assert!(tmux.kill_prefix(&super::super::run_prefix(7)).is_empty());
        assert!(tmux.tagged_sessions().iter().any(|(i, _)| *i == id));
    }
}
