//! Herdr's local newline-delimited JSON socket adapter.

use super::{Launch, Layout, Move, Target};
use crate::{
    config::Config,
    output::{CliError, CliResult},
};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub struct Herdr {
    pub socket: PathBuf,
}

impl Herdr {
    pub fn from_env() -> Self {
        Self {
            socket: socket_path(),
        }
    }
    pub fn from_socket(path: Option<&str>) -> CliResult<Self> {
        Ok(Self {
            socket: path.map(PathBuf::from).unwrap_or_else(socket_path),
        })
    }

    fn call(&self, method: &str, params: Value) -> CliResult<Value> {
        let unavailable = || {
            CliError::internal("backend_unavailable: herdr isn't available")
                .with_hint("start herdr or set `herdr.session` in the tome config")
        };
        let mut stream = UnixStream::connect(&self.socket).map_err(|_| unavailable())?;
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(3)))
            .map_err(|_| unavailable())?;
        stream
            .set_write_timeout(Some(std::time::Duration::from_secs(3)))
            .map_err(|_| unavailable())?;
        let id = format!("tome-{}", NEXT_ID.fetch_add(1, Ordering::Relaxed));
        serde_json::to_writer(
            &mut stream,
            &json!({"id":id,"method":method,"params":params}),
        )
        .map_err(|e| CliError::internal(format!("encoding herdr request: {e}")))?;
        stream.write_all(b"\n").map_err(|_| unavailable())?;
        let mut line = String::new();
        BufReader::new(stream)
            .read_line(&mut line)
            .map_err(|_| unavailable())?;
        let response: Value = serde_json::from_str(&line)
            .map_err(|e| CliError::internal(format!("invalid herdr response: {e}")))?;
        if !response["error"].is_null() {
            return Err(CliError::internal(format!(
                "herdr {}: {}",
                response["error"]["code"].as_str().unwrap_or("error"),
                response["error"]["message"]
                    .as_str()
                    .unwrap_or("request failed")
            )));
        }
        Ok(response["result"].clone())
    }

    pub fn notify(title: &str, body: &str) {
        let _ = Self::from_env().call("notification.show", json!({"title":title,"body":body}));
    }

    pub fn focused() -> Result<String, String> {
        Self::from_env()
            .call("session.snapshot", json!({}))
            .map_err(|e| e.message)?["focused"]["workspace_id"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| "herdr has no focused workspace".into())
    }

    pub fn caller_anchor(pane: &str) -> Result<(super::split::Anchor, String), String> {
        let h = Self::from_env();
        let info = h
            .call("pane.get", json!({"pane_id":pane}))
            .map_err(|e| e.message)?;
        let ws = info["workspace_id"]
            .as_str()
            .ok_or("the caller's herdr pane is gone")?
            .to_string();
        let label = info["label"].as_str().unwrap_or(pane).to_owned();
        Ok((
            super::split::Anchor {
                handle: ws,
                pane: pane.to_owned(),
            },
            label,
        ))
    }

    pub fn launch(&self, l: &Launch, warnings: &mut Vec<String>) -> CliResult<(String, String)> {
        self.call("ping", json!({}))?;
        let command = json!(["sh", l.script.to_string_lossy()]);
        let env: serde_json::Map<String, Value> =
            l.env.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
        let focus = false;
        let (workspace, pane) = if l.layout == Layout::Workspace {
            let created = self.call("workspace.create", json!({"label":l.name,"cwd":l.cwd,"command":command.clone(),"env":env.clone(),"focus":focus}))?;
            let ws = created["workspace"]["workspace_id"]
                .as_str()
                .or(created["workspace"]["id"].as_str())
                .ok_or_else(|| CliError::internal("herdr didn't return a workspace id"))?
                .to_owned();
            let pane = created["root_pane"]["pane_id"]
                .as_str()
                .ok_or_else(|| CliError::internal("herdr didn't return a root pane id"))?
                .to_owned();
            (ws, pane)
        } else if l.layout == Layout::Tab {
            let ws = match l.target {
                Target::Focused(id) | Target::Caller(id) => id.clone(),
                _ => self.find_or_create_workspace(
                    &workspace_label(l.project, l.target),
                    l.cwd,
                    focus,
                    warnings,
                )?,
            };
            let response = self.call("tab.create", json!({"workspace_id":ws,"label":l.title,"cwd":l.cwd,"command":command.clone(),"env":env.clone(),"focus":focus}))?;
            let pane = response["root_pane"]["pane_id"]
                .as_str()
                .or(response["pane"]["pane_id"].as_str())
                .ok_or_else(|| CliError::internal("herdr didn't return a pane id for the new tab"))?
                .to_owned();
            (ws, pane)
        } else {
            let ws = match l.target {
                Target::Focused(id) | Target::Caller(id) => id.clone(),
                _ => self.find_or_create_workspace(
                    &workspace_label(l.project, l.target),
                    l.cwd,
                    focus,
                    warnings,
                )?,
            };
            let snapshot = self.call("session.snapshot", json!({}))?;
            let panes = snapshot["panes"].as_array().cloned().unwrap_or_default();
            let anchor = match l.target {
                Target::Caller(p) => p.clone(),
                _ => l
                    .split
                    .anchors
                    .iter()
                    .find(|a| a.handle == ws && panes.iter().any(|p| p["pane_id"] == a.pane))
                    .map(|a| a.pane.clone())
                    .or_else(|| {
                        l.split
                            .recent
                            .iter()
                            .find(|a| {
                                a.handle == ws && panes.iter().any(|p| p["pane_id"] == a.pane)
                            })
                            .map(|a| a.pane.clone())
                    })
                    .or_else(|| {
                        panes
                            .iter()
                            .find(|p| p["workspace_id"] == ws)
                            .and_then(|p| p["pane_id"].as_str())
                            .map(str::to_owned)
                    })
                    .unwrap_or_default(),
            };
            if anchor.is_empty() {
                return Err(CliError::internal(
                    "herdr workspace has no pane to split into",
                ));
            }
            let direction = match l.split.direction {
                crate::placement::Direction::Down | crate::placement::Direction::Up => "down",
                _ => "right",
            };
            let mut params = json!({"pane_id":anchor,"direction":direction,"command":command.clone(),"cwd":l.cwd,"env":env.clone(),"label":l.name,"focus":focus});
            if let Some(ratio) =
                self.split_ratio(&anchor, l.split.size, l.split.direction, warnings)
            {
                params["ratio"] = json!(ratio);
            }
            let response = self.call("pane.split", params)?;
            let pane = response["pane"]["pane_id"]
                .as_str()
                .ok_or_else(|| CliError::internal("herdr didn't return a pane id for the split"))?
                .to_owned();
            if matches!(
                l.split.direction,
                crate::placement::Direction::Left | crate::placement::Direction::Up
            ) {
                if self
                    .call(
                        "pane.swap",
                        json!({"source_pane_id":pane,"target_pane_id":anchor}),
                    )
                    .is_err()
                {
                    warnings.push(format!(
                        "split.direction {}: herdr couldn't swap the new pane; left it {direction}",
                        l.split.direction
                    ));
                }
            }
            (ws, pane)
        };
        let _ = workspace;
        let _ = self.call("pane.rename", json!({"pane_id":pane,"label":l.name}));
        Ok((workspace, pane))
    }

    fn find_or_create_workspace(
        &self,
        label: &str,
        cwd: &Path,
        _focus: bool,
        warnings: &mut Vec<String>,
    ) -> CliResult<String> {
        let result = self.call("workspace.list", json!({}))?;
        let matches: Vec<&Value> = result["workspaces"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|w| w["label"] == label)
            .collect();
        if matches.len() > 1 {
            warnings.push(format!(
                "workspace label `{label}` is duplicated in herdr; using the first"
            ));
        }
        if let Some(ws) = matches.first().copied() {
            if let Some(id) = ws["workspace_id"].as_str().or(ws["id"].as_str()) {
                return Ok(id.to_owned());
            }
        }
        let created = self.call(
            "workspace.create",
            json!({"label":label,"cwd":cwd,"focus":false}),
        )?;
        created["workspace"]["workspace_id"]
            .as_str()
            .or(created["workspace"]["id"].as_str())
            .map(str::to_owned)
            .ok_or_else(|| CliError::internal("herdr didn't return a workspace id"))
    }

    pub fn move_pane(
        &self,
        s: &crate::store::Session,
        pane: &str,
        m: &Move,
        warnings: &mut Vec<String>,
    ) -> CliResult<(Option<String>, String)> {
        let destination = if m.layout == Layout::Workspace {
            json!({"type":"new_workspace","label":s.name,"tab_label":m.title})
        } else {
            let ws = match m.target {
                Target::Focused(id) | Target::Caller(id) => id.clone(),
                _ => self.find_or_create_workspace(
                    &workspace_label(m.project, m.target),
                    &m.project
                        .map(Path::to_path_buf)
                        .unwrap_or_else(crate::paths::user_home),
                    false,
                    warnings,
                )?,
            };
            if m.layout == Layout::Tab {
                json!({"type":"new_tab","workspace_id":ws,"label":m.title})
            } else {
                let snapshot = self.call("session.snapshot", json!({}))?;
                let tab = snapshot["tabs"]
                    .as_array()
                    .and_then(|tabs| tabs.iter().find(|t| t["workspace_id"] == ws))
                    .and_then(|t| t["tab_id"].as_str())
                    .ok_or_else(|| CliError::internal("herdr target workspace has no tab"))?;
                let panes = snapshot["panes"].as_array().cloned().unwrap_or_default();
                let target = panes
                    .iter()
                    .find(|p| p["workspace_id"] == ws)
                    .and_then(|p| p["pane_id"].as_str())
                    .unwrap_or(pane);
                let split = if matches!(
                    m.split.direction,
                    crate::placement::Direction::Right | crate::placement::Direction::Left
                ) {
                    "right"
                } else {
                    "down"
                };
                let mut destination =
                    json!({"type":"tab","tab_id":tab,"target_pane_id":target,"split":split});
                if let Some(ratio) =
                    self.split_ratio(target, m.split.size, m.split.direction, warnings)
                {
                    destination["ratio"] = json!(ratio);
                }
                destination
            }
        };
        let response = self.call(
            "pane.move",
            json!({"pane_id":pane,"destination":destination,"focus":false}),
        )?;
        let moved = response["pane"]["pane_id"]
            .as_str()
            .unwrap_or(pane)
            .to_owned();
        let info = self.call("pane.get", json!({"pane_id":moved}))?;
        let ws = info["workspace_id"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| CliError::internal("herdr didn't return the moved pane's workspace"))?;
        let _ = warnings;
        Ok((Some(ws), moved))
    }

    /// Whether the pane is open; `None` if herdr didn't answer.
    pub fn alive(&self, pane: &str) -> Option<bool> {
        match self.call("pane.get", json!({"pane_id":pane})) {
            Ok(_) => Some(true),
            Err(e) if e.message.contains("backend_unavailable") => None,
            Err(_) => Some(false),
        }
    }

    fn split_ratio(
        &self,
        pane: &str,
        size: Option<crate::placement::Size>,
        direction: crate::placement::Direction,
        warnings: &mut Vec<String>,
    ) -> Option<f64> {
        use crate::placement::Size;
        let size = size?;
        match size {
            Size::Percent(n) => Some(f64::from(n.clamp(10, 90)) / 100.0),
            Size::Cells(n) => {
                let info = self.call("pane.layout", json!({"pane_id":pane})).ok();
                let extent = if matches!(
                    direction,
                    crate::placement::Direction::Right | crate::placement::Direction::Left
                ) {
                    "width"
                } else {
                    "height"
                };
                let available = info
                    .as_ref()
                    .and_then(|v| {
                        v["area"][extent]
                            .as_f64()
                            .or(v["layout"]["area"][extent].as_f64())
                    })
                    .filter(|n| *n > 0.0);
                match available {
                    Some(available) => Some((f64::from(n) / available).clamp(0.1, 0.9)),
                    None => {
                        warnings.push(format!(
                            "split.size {size}: herdr couldn't read pane geometry; skipped"
                        ));
                        None
                    }
                }
            }
        }
    }
    pub fn agent_status(&self, pane: &str) -> Option<String> {
        let info = self.call("pane.get", json!({"pane_id":pane})).ok()?;
        info["agent_status"]
            .as_str()
            .or(info["agent"]["status"].as_str())
            .map(str::to_owned)
    }
    pub fn close(&self, pane: &str) -> bool {
        self.call("pane.close", json!({"pane_id":pane})).is_ok()
    }
    pub fn send_line(&self, pane: &str, text: &str) -> bool {
        self.call("pane.send_text", json!({"pane_id":pane,"text":text}))
            .is_ok()
            && self
                .call("pane.send_keys", json!({"pane_id":pane,"keys":["enter"]}))
                .is_ok()
    }
}

fn socket_path() -> PathBuf {
    let home = crate::paths::user_home();
    let sessions = home.join(".config/herdr/sessions");
    // Inside herdr, its own environment names the live server.
    if let Some(path) = std::env::var_os("HERDR_SOCKET_PATH").filter(|p| !p.is_empty()) {
        return PathBuf::from(path);
    }
    if let Some(session) = std::env::var("HERDR_SESSION")
        .ok()
        .filter(|s| !s.is_empty())
    {
        return sessions.join(session).join("herdr.sock");
    }
    if let Ok(config) = Config::load(None) {
        if let Some((value, _)) = config.get("herdr") {
            if let Some(session) = value.get("session").and_then(serde_yaml::Value::as_str) {
                return sessions.join(session).join("herdr.sock");
            }
        }
    }
    home.join(".config/herdr/herdr.sock")
}

fn workspace_label(project: Option<&Path>, target: &Target) -> String {
    let base = project
        .and_then(Path::file_name)
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "global".into());
    let base: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let tail = match target {
        Target::Named(n) => n.as_str(),
        _ => "orchestrator",
    };
    format!("{base}-{tail}")
}
