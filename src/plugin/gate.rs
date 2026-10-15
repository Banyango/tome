//! The reply to a blocking `run.before_start` hook, and what it may change.
//!
//! A plugin answers with `{"action": "allow"}`, `{"action": "reject",
//! "reason": "..."}` or `{"action": "modify", "params": {...}, "env": {...}}`.
//! Each plugin sees the run as the plugins before it left it. What changed is
//! kept as [`Change`]s, which are stored on the run and shown by
//! `tome runs show`. Env values aren't recorded, only their names.

use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Allow,
    Reject(String),
    Modify {
        params: Map<String, Value>,
        env: BTreeMap<String, String>,
    },
}

/// Read a hook's reply. An error means the plugin broke the protocol, which
/// the caller treats like any other hook failure.
pub fn parse_reply(reply: &Value) -> Result<Verdict, String> {
    let action = reply
        .get("action")
        .and_then(Value::as_str)
        .ok_or("the reply has no `action` (use allow, reject or modify)")?;
    match action {
        "allow" => Ok(Verdict::Allow),
        "reject" => {
            let reason = reply
                .get("reason")
                .and_then(Value::as_str)
                .filter(|r| !r.trim().is_empty())
                .unwrap_or("no reason given");
            Ok(Verdict::Reject(reason.to_string()))
        }
        "modify" => {
            let params = match reply.get("params") {
                None | Some(Value::Null) => Map::new(),
                Some(Value::Object(m)) => m.clone(),
                Some(_) => return Err("`params` must be an object".into()),
            };
            let mut env = BTreeMap::new();
            match reply.get("env") {
                None | Some(Value::Null) => {}
                Some(Value::Object(m)) => {
                    for (key, value) in m {
                        let value = value
                            .as_str()
                            .ok_or_else(|| format!("env `{key}` must be a string"))?;
                        check_env_name(key)?;
                        env.insert(key.clone(), value.to_string());
                    }
                }
                Some(_) => return Err("`env` must be an object".into()),
            }
            Ok(Verdict::Modify { params, env })
        }
        other => Err(format!(
            "unknown action `{other}` (use allow, reject or modify)"
        )),
    }
}

/// Env a hook adds to the run's sessions. `TOME_*` is tome's own.
fn check_env_name(name: &str) -> Result<(), String> {
    let mut chars = name.chars();
    let first_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    if !first_ok || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(format!("`{name}` is not a valid environment variable name"));
    }
    if name.starts_with("TOME_") {
        return Err(format!("`{name}` is reserved: `TOME_*` belongs to tome"));
    }
    Ok(())
}

/// What one plugin changed.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub plugin: String,
    /// Param name to its value before and after (`None` before: not set).
    pub params: BTreeMap<String, (Option<Value>, Value)>,
    /// Names of env variables set or replaced.
    pub env: Vec<String>,
}

impl Change {
    pub fn to_json(&self) -> Value {
        let params: Map<String, Value> = self
            .params
            .iter()
            .map(|(k, (from, to))| (k.clone(), json!({ "from": from, "to": to })))
            .collect();
        json!({ "plugin": self.plugin, "params": params, "env": self.env })
    }
}

/// The run as the hooks see and change it.
#[derive(Debug, Clone, Default)]
pub struct Gate {
    pub params: Map<String, Value>,
    pub env: BTreeMap<String, String>,
    pub changes: Vec<Change>,
}

impl Gate {
    pub fn new(params: Map<String, Value>) -> Gate {
        Gate {
            params,
            ..Gate::default()
        }
    }

    /// Take a plugin's modification. Values equal to what is already there
    /// aren't changes.
    pub fn apply(
        &mut self,
        plugin: &str,
        params: Map<String, Value>,
        env: BTreeMap<String, String>,
    ) {
        let mut change = Change {
            plugin: plugin.to_string(),
            params: BTreeMap::new(),
            env: Vec::new(),
        };
        for (key, to) in params {
            let from = self.params.get(&key).cloned();
            if from.as_ref() != Some(&to) {
                self.params.insert(key.clone(), to.clone());
                change.params.insert(key, (from, to));
            }
        }
        for (key, value) in env {
            if self.env.get(&key) != Some(&value) {
                self.env.insert(key.clone(), value);
                change.env.push(key);
            }
        }
        if !change.params.is_empty() || !change.env.is_empty() {
            self.changes.push(change);
        }
    }

    /// What goes on the run's placement record: the changes, and the env its
    /// sessions get.
    pub fn record(&self) -> Map<String, Value> {
        let mut out = Map::new();
        if !self.changes.is_empty() {
            out.insert(
                "hooks".into(),
                Value::Array(self.changes.iter().map(Change::to_json).collect()),
            );
        }
        if !self.env.is_empty() {
            out.insert("env".into(), json!(self.env));
        }
        out
    }

    /// `--param` overrides for the params the hooks changed, as the run
    /// takes them.
    pub fn overrides(&self) -> Vec<(String, String)> {
        let mut keys: Vec<&String> = self.changes.iter().flat_map(|c| c.params.keys()).collect();
        keys.sort();
        keys.dedup();
        keys.into_iter()
            .map(|k| (k.clone(), as_arg(&self.params[k])))
            .collect()
    }
}

/// A param value in the form `--param key=value` takes.
fn as_arg(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_are_read_strictly() {
        assert_eq!(parse_reply(&json!({"action": "allow"})), Ok(Verdict::Allow));
        assert_eq!(
            parse_reply(&json!({"action": "reject", "reason": "over budget"})),
            Ok(Verdict::Reject("over budget".into()))
        );
        assert_eq!(
            parse_reply(&json!({"action": "reject"})),
            Ok(Verdict::Reject("no reason given".into()))
        );
        assert!(parse_reply(&json!({})).is_err());
        assert!(parse_reply(&json!(null)).is_err());
        assert!(parse_reply(&json!({"action": "maybe"})).is_err());
        assert!(parse_reply(&json!({"action": "modify", "params": 3})).is_err());
        assert!(parse_reply(&json!({"action": "modify", "env": {"A": 1}})).is_err());
    }

    #[test]
    fn env_names_are_checked() {
        for bad in ["TOME_RUN_ID", "1A", "A-B", "", "A B"] {
            let reply = json!({"action": "modify", "env": {bad: "x"}});
            assert!(parse_reply(&reply).is_err(), "{bad}");
        }
        let reply = json!({"action": "modify", "env": {"AWS_REGION": "eu"}});
        assert!(matches!(parse_reply(&reply), Ok(Verdict::Modify { .. })));
    }

    fn map(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn changes_record_what_differs_and_who() {
        let mut gate = Gate::new(map(json!({"size": "small", "tag": "x"})));
        gate.apply(
            "budget",
            map(json!({"size": "large", "tag": "x"})),
            BTreeMap::from([("REGION".to_string(), "eu".to_string())]),
        );
        gate.apply("noop", map(json!({"size": "large"})), BTreeMap::new());
        assert_eq!(gate.changes.len(), 1, "an unchanged value isn't a change");
        let change = &gate.changes[0];
        assert_eq!(change.plugin, "budget");
        assert_eq!(
            change.params["size"],
            (Some(json!("small")), json!("large"))
        );
        assert_eq!(change.env, vec!["REGION"]);
        assert_eq!(
            gate.overrides(),
            vec![("size".to_string(), "large".to_string())]
        );
        let record = gate.record();
        assert_eq!(record["env"], json!({"REGION": "eu"}));
        assert_eq!(record["hooks"][0]["params"]["size"]["to"], "large");
        assert!(
            !record["hooks"][0].to_string().contains("eu"),
            "values stay out"
        );
    }

    #[test]
    fn later_plugins_see_earlier_changes() {
        let mut gate = Gate::new(map(json!({"n": 1})));
        gate.apply("a", map(json!({"n": 2})), BTreeMap::new());
        gate.apply("b", map(json!({"n": 3})), BTreeMap::new());
        assert_eq!(gate.changes[1].params["n"], (Some(json!(2)), json!(3)));
        assert_eq!(gate.overrides(), vec![("n".to_string(), "3".to_string())]);
    }

    #[test]
    fn nothing_changed_records_nothing() {
        let gate = Gate::new(map(json!({"n": 1})));
        assert!(gate.record().is_empty());
        assert!(gate.overrides().is_empty());
    }
}
