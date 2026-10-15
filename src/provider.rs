//! Node providers: plugins that make a machine on demand for `tome run
//! --on <provider>`, and the local record of the runs on machines since
//! released.
//!
//! The rules here are deterministic (reading `--provider-opt`, a `provide`
//! reply, the name of the node it becomes, a stub record's contents). The
//! files at the end are the edge: the registry of kept nodes and the stubs.

use crate::node::{self, Node};
use crate::paths;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use serde_yaml::Value as Yaml;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The most of a log kept in a stub, per step.
pub const TAIL_LINES: usize = 40;

/// `key=value` options, as `--provider-opt` and `defaults.provider_opts`
/// give them.
pub type Opts = BTreeMap<String, String>;

/// Read repeated `--provider-opt key=value`. Later ones win.
pub fn parse_opts(args: &[String]) -> Result<Opts, String> {
    let mut out = Opts::new();
    for arg in args {
        let (key, value) = arg
            .split_once('=')
            .ok_or_else(|| format!("invalid --provider-opt `{arg}`: use key=value"))?;
        check_key(key)?;
        out.insert(key.to_string(), value.to_string());
    }
    Ok(out)
}

fn check_key(key: &str) -> Result<(), String> {
    let ok = !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if ok {
        Ok(())
    } else {
        Err(format!(
            "invalid provider option name `{key}`: use letters, digits, `_`, `-` and `.`"
        ))
    }
}

/// `defaults.provider_opts`: a mapping of name to a string, number or bool.
pub fn opts_from_yaml(value: &Yaml) -> Result<Opts, String> {
    let map = match value {
        Yaml::Null => return Ok(Opts::new()),
        Yaml::Mapping(m) => m,
        _ => return Err("`provider_opts` must be a mapping of option name to value".into()),
    };
    let mut out = Opts::new();
    for (k, v) in map {
        let key = k.as_str().ok_or("`provider_opts` names must be strings")?;
        check_key(key)?;
        let value = match v {
            Yaml::String(s) => s.clone(),
            Yaml::Number(n) => n.to_string(),
            Yaml::Bool(b) => b.to_string(),
            _ => {
                return Err(format!(
                    "`provider_opts.{key}` must be a string, number or boolean"
                ))
            }
        };
        out.insert(key.to_string(), value);
    }
    Ok(out)
}

/// Options from the lowest layer to the highest: later layers win per key.
pub fn merge_opts(layers: &[Opts]) -> Opts {
    layers
        .iter()
        .flat_map(|l| l.iter())
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// What `provide` returns: a machine tome can reach over SSH.
///
/// ```json
/// {"id": "i-0abc", "ssh": "ubuntu@10.0.0.7", "tome": "/opt/tome", "projects": {"app": "~/app"}}
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provided {
    /// The plugin's name for the machine, given back to `release`.
    pub id: String,
    pub ssh: String,
    pub tome: Option<String>,
    pub projects: BTreeMap<String, String>,
}

impl Provided {
    pub fn from_reply(reply: &Value) -> Result<Provided, String> {
        let text = |key: &str| match reply.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) if !s.trim().is_empty() => Ok(Some(s.trim().to_string())),
            Some(_) => Err(format!("`{key}` must be a non-empty string")),
        };
        let id = text("id")?.ok_or("the reply has no `id` (the machine's name, for `release`)")?;
        if id.len() > 64
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        {
            return Err(format!(
                "`id` `{id}` is not usable: up to 64 letters, digits, `_`, `-` and `.`"
            ));
        }
        let ssh = text("ssh")?.ok_or("the reply has no `ssh` (where to reach the machine)")?;
        let mut projects = BTreeMap::new();
        match reply.get("projects") {
            None | Some(Value::Null) => {}
            Some(Value::Object(m)) => {
                for (k, v) in m {
                    let path = v
                        .as_str()
                        .ok_or_else(|| format!("`projects.{k}` must be a path"))?;
                    projects.insert(k.clone(), path.to_string());
                }
            }
            Some(_) => return Err("`projects` must be an object of directory name to path".into()),
        }
        Ok(Provided {
            id,
            ssh,
            tome: text("tome")?,
            projects,
        })
    }

    /// The ephemeral node this machine is while the run lasts.
    pub fn node(&self, provider: &str) -> Node {
        Node {
            name: node_name(provider, &self.id),
            ssh: self.ssh.clone(),
            tome: self.tome.clone(),
            projects: self.projects.clone(),
        }
    }
}

/// `<provider>-<id>`, lowercased, so it is a valid node name.
pub fn node_name(provider: &str, id: &str) -> String {
    let id: String = id
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("{provider}-{id}")
}

/// Why `release` is called, which the plugin may use to decide what to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    /// The run ended (succeeded, failed or was cancelled).
    Finished,
    /// `provide` failed or timed out; the machine may be half made.
    ProvideFailed,
    /// The machine failed `node check`.
    CheckFailed,
    /// Ctrl-C while waiting for the machine.
    Interrupted,
}

impl Cause {
    pub fn as_str(self) -> &'static str {
        match self {
            Cause::Finished => "finished",
            Cause::ProvideFailed => "provide_failed",
            Cause::CheckFailed => "check_failed",
            Cause::Interrupted => "interrupted",
        }
    }
}

/// The record of a run on a machine that has been released.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stub {
    pub provider: String,
    pub id: String,
    /// The node name the machine had while it lived.
    pub node: String,
    pub run: Option<i64>,
    pub workflow: Option<String>,
    /// The run's final status, or `unknown` if the node couldn't be asked.
    pub status: String,
    pub reason: Option<String>,
    pub summary: Option<String>,
    pub started: Option<String>,
    pub finished: Option<String>,
    pub released_at: String,
    pub log_tail: Vec<LogTail>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogTail {
    pub step: String,
    pub content: String,
}

impl Stub {
    /// `cloud-vm:i-0abc (released)`
    pub fn label(&self) -> String {
        format!("{}:{} (released)", self.provider, self.id)
    }

    /// From the node's `runs.show` and `runs.logs` answers.
    pub fn from_node(
        provider: &str,
        provided: &Provided,
        show: &Value,
        logs: Option<&Value>,
        released_at: &str,
    ) -> Stub {
        let run = &show["run"];
        let text = |key: &str| run[key].as_str().map(str::to_string);
        let log_tail = logs
            .and_then(|l| l["logs"].as_array())
            .into_iter()
            .flatten()
            .filter_map(|l| {
                Some(LogTail {
                    step: l["step"].as_str()?.to_string(),
                    content: last_lines(l["content"].as_str()?, TAIL_LINES),
                })
            })
            .collect();
        Stub {
            provider: provider.to_string(),
            id: provided.id.clone(),
            node: node_name(provider, &provided.id),
            run: run["id"].as_i64(),
            workflow: text("workflow_name"),
            status: text("status").unwrap_or_else(|| "unknown".into()),
            reason: text("reason"),
            summary: text("summary"),
            started: text("created_at"),
            finished: text("finished_at"),
            released_at: released_at.to_string(),
            log_tail,
        }
    }

    /// When the node couldn't be asked: the record says so.
    pub fn unavailable(provider: &str, provided: &Provided, why: &str, released_at: &str) -> Stub {
        Stub {
            provider: provider.to_string(),
            id: provided.id.clone(),
            node: node_name(provider, &provided.id),
            run: None,
            workflow: None,
            status: "unknown".into(),
            reason: Some(format!(
                "the run's outcome couldn't be read before release: {why}"
            )),
            summary: None,
            started: None,
            finished: None,
            released_at: released_at.to_string(),
            log_tail: Vec::new(),
        }
    }

    /// The row `tome runs ls` shows.
    pub fn row(&self) -> Vec<String> {
        let or_dash = |v: &Option<String>| v.clone().unwrap_or_else(|| "-".into());
        vec![
            self.label(),
            or_dash(&self.workflow),
            self.status.clone(),
            or_dash(&self.started),
            or_dash(&self.finished),
            or_dash(&self.reason),
        ]
    }

    /// What the plugin is told about the run, in the `release` request.
    pub fn summary_json(&self) -> Value {
        json!({
            "run": self.run,
            "status": self.status,
            "summary": self.summary,
            "reason": self.reason,
        })
    }
}

/// The last `n` lines of `text`.
pub fn last_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// The stubs that pass `status` and `workflow`, newest release first.
pub fn filter<'a>(
    stubs: &'a [Stub],
    status: Option<&str>,
    workflow: Option<&str>,
    limit: usize,
) -> Vec<&'a Stub> {
    let mut out: Vec<&Stub> = stubs
        .iter()
        .filter(|s| status.is_none_or(|st| s.status == st))
        .filter(|s| workflow.is_none_or(|w| s.workflow.as_deref() == Some(w)))
        .collect();
    out.sort_by(|a, b| b.released_at.cmp(&a.released_at));
    out.truncate(limit);
    out
}

// --- files (the edge) ---------------------------------------------------

/// `~/.tome/released/`: one stub per run of a released machine.
pub fn stubs_dir() -> PathBuf {
    paths::tome_home().join("released")
}

/// `~/.tome/ephemeral/`: machines a provider made that are still around,
/// as nodes.
pub fn ephemeral_dir() -> PathBuf {
    paths::tome_home().join("ephemeral")
}

pub fn save_stub(stub: &Stub) -> std::io::Result<PathBuf> {
    let dir = stubs_dir();
    std::fs::create_dir_all(&dir)?;
    let run = stub.run.map_or_else(|| "x".to_string(), |r| r.to_string());
    let path = dir.join(format!(
        "{}-{}-{}.json",
        node_name(&stub.provider, &stub.id),
        run,
        stub.released_at.replace([':', '-'], "")
    ));
    let text = serde_json::to_string_pretty(stub).map_err(std::io::Error::other)?;
    std::fs::write(&path, text)?;
    Ok(path)
}

/// Every stub, skipping files that don't parse.
pub fn load_stubs() -> Vec<Stub> {
    let Ok(read) = std::fs::read_dir(stubs_dir()) else {
        return Vec::new();
    };
    read.filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|t| serde_json::from_str(&t).ok())
        .collect()
}

/// Make `node` reachable by name while its machine lives.
pub fn register(node: &Node) -> std::io::Result<()> {
    let dir = ephemeral_dir();
    std::fs::create_dir_all(&dir)?;
    let text = serde_json::to_string_pretty(node).map_err(std::io::Error::other)?;
    std::fs::write(dir.join(format!("{}.json", node.name)), text)
}

pub fn unregister(name: &str) -> bool {
    std::fs::remove_file(ephemeral_dir().join(format!("{name}.json"))).is_ok()
}

/// The registered ephemeral nodes, by name.
pub fn ephemeral_nodes() -> BTreeMap<String, Node> {
    let Ok(read) = std::fs::read_dir(ephemeral_dir()) else {
        return BTreeMap::new();
    };
    read.filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|t| serde_json::from_str::<Value>(&t).ok())
        .filter_map(|v| {
            let name = v["name"].as_str()?.to_string();
            node::check_name(&name).ok()?;
            let projects = v["projects"]
                .as_object()
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                        .collect()
                })
                .unwrap_or_default();
            Some((
                name.clone(),
                Node {
                    name,
                    ssh: v["ssh"].as_str()?.to_string(),
                    tome: v["tome"].as_str().map(str::to_string),
                    projects,
                },
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn options_are_key_value_and_later_ones_win() {
        let opts = parse_opts(&args(&[
            "size=large",
            "region=eu",
            "size=small",
            "note=a=b",
        ]))
        .unwrap();
        assert_eq!(opts["size"], "small");
        assert_eq!(opts["note"], "a=b", "only the first `=` splits");
        assert!(parse_opts(&args(&["size"])).is_err());
        assert!(parse_opts(&args(&["=x"])).is_err());
        assert!(parse_opts(&args(&["a b=1"])).is_err());
    }

    #[test]
    fn defaults_come_from_yaml_and_the_command_line_wins() {
        let yaml: Yaml = serde_yaml::from_str("size: large\ndisk: 40\nspot: true").unwrap();
        let defaults = opts_from_yaml(&yaml).unwrap();
        assert_eq!(defaults["disk"], "40");
        assert_eq!(defaults["spot"], "true");
        let cli = parse_opts(&args(&["size=small"])).unwrap();
        let merged = merge_opts(&[defaults, cli]);
        assert_eq!(merged["size"], "small");
        assert_eq!(merged["disk"], "40");
        let bad: Yaml = serde_yaml::from_str("size: [a]").unwrap();
        assert!(opts_from_yaml(&bad).is_err());
        assert!(opts_from_yaml(&Yaml::String("x".into())).is_err());
    }

    #[test]
    fn a_provide_reply_must_say_where_the_machine_is() {
        let ok = Provided::from_reply(&json!({
            "id": "i-0Abc", "ssh": "ubuntu@10.0.0.7", "tome": "/opt/tome",
            "projects": {"app": "~/app"}
        }))
        .unwrap();
        assert_eq!(ok.tome.as_deref(), Some("/opt/tome"));
        assert_eq!(ok.projects["app"], "~/app");
        let node = ok.node("cloud-vm");
        assert_eq!(node.name, "cloud-vm-i-0abc");
        assert!(node::check_name(&node.name).is_ok());
        assert_eq!(node.ssh, "ubuntu@10.0.0.7");

        for bad in [
            json!({"ssh": "h"}),
            json!({"id": "a"}),
            json!({"id": "a b", "ssh": "h"}),
            json!({"id": "a", "ssh": 3}),
            json!({"id": "a", "ssh": "h", "projects": "x"}),
            json!(null),
        ] {
            assert!(Provided::from_reply(&bad).is_err(), "{bad}");
        }
    }

    fn provided() -> Provided {
        Provided::from_reply(&json!({"id": "i-1", "ssh": "h"})).unwrap()
    }

    #[test]
    fn a_stub_keeps_the_outcome_and_the_log_tail() {
        let long: String = (1..=100).map(|n| format!("line {n}\n")).collect();
        let show = json!({"run": {
            "id": 7, "workflow_name": "build", "status": "failed", "reason": "step failed",
            "summary": "tests broke", "created_at": "2026-01-01T10:00:00Z",
            "finished_at": "2026-01-01T10:05:00Z"
        }});
        let logs = json!({"logs": [{"step": "test", "content": long}]});
        let stub = Stub::from_node(
            "cloud-vm",
            &provided(),
            &show,
            Some(&logs),
            "2026-01-01T10:06:00Z",
        );
        assert_eq!(stub.label(), "cloud-vm:i-1 (released)");
        assert_eq!(stub.run, Some(7));
        assert_eq!(stub.status, "failed");
        assert_eq!(stub.log_tail[0].content.lines().count(), TAIL_LINES);
        assert!(stub.log_tail[0].content.ends_with("line 100"));
        assert_eq!(stub.row()[2], "failed");
        let back: Stub = serde_json::from_str(&serde_json::to_string(&stub).unwrap()).unwrap();
        assert_eq!(back, stub);
    }

    #[test]
    fn an_unreadable_outcome_is_recorded_as_unknown() {
        let stub = Stub::unavailable(
            "cloud-vm",
            &provided(),
            "ssh timed out",
            "2026-01-01T10:06:00Z",
        );
        assert_eq!(stub.status, "unknown");
        assert!(stub.reason.unwrap().contains("ssh timed out"));
    }

    #[test]
    fn stubs_filter_and_sort_newest_first() {
        let mk = |id: &str, status: &str, wf: &str, at: &str| Stub {
            provider: "p".into(),
            id: id.into(),
            node: format!("p-{id}"),
            run: Some(1),
            workflow: Some(wf.into()),
            status: status.into(),
            reason: None,
            summary: None,
            started: None,
            finished: None,
            released_at: at.into(),
            log_tail: vec![],
        };
        let stubs = vec![
            mk("a", "failed", "x", "2026-01-01T00:00:00Z"),
            mk("b", "succeeded", "x", "2026-01-02T00:00:00Z"),
            mk("c", "failed", "y", "2026-01-03T00:00:00Z"),
        ];
        let ids = |v: Vec<&Stub>| v.iter().map(|s| s.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(filter(&stubs, None, None, 10)), ["c", "b", "a"]);
        assert_eq!(ids(filter(&stubs, Some("failed"), None, 10)), ["c", "a"]);
        assert_eq!(ids(filter(&stubs, None, Some("x"), 10)), ["b", "a"]);
        assert_eq!(ids(filter(&stubs, None, None, 1)), ["c"]);
    }

    #[test]
    fn tails_keep_the_last_lines() {
        assert_eq!(last_lines("a\nb\nc", 2), "b\nc");
        assert_eq!(last_lines("a", 5), "a");
        assert_eq!(last_lines("", 5), "");
    }
}
