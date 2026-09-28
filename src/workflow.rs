//! Workflow files: markdown with yaml frontmatter and a natural-English body.
//!
//! ```markdown
//! ---
//! name: review-loop
//! params:
//!   base: {default: main}
//! ---
//! ## Implement
//! Create a worktree off {{params.base}} ...
//! ```
//!
//! Validation covers the frontmatter (yaml syntax, known keys, value shapes)
//! and `{{placeholders}}` in the body; the English itself is never parsed.
//! Every problem is reported with a 1-based line number in the file.

use crate::duration;
use crate::output::{CliError, CliResult, ErrorKind};
use serde::Serialize;
use serde_json::{json, Map, Value};
use serde_yaml::Value as Yaml;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const TOP_LEVEL_KEYS: &[&str] =
    &["name", "description", "triggers", "params", "defaults", "concurrency", "on_conflict", "orchestrator"];
pub const DEFAULTS_KEYS: &[&str] = &["backend", "layout", "harness", "orchestrator_harness", "timeout", "on_failure", "start_timeout"];
pub const PARAM_KEYS: &[&str] = &["type", "default", "description"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub line: usize,
    pub message: String,
}

impl Diagnostic {
    fn new(line: usize, message: impl Into<String>) -> Self {
        Diagnostic { line, message: message.into() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ParamType {
    String,
    Int,
    Float,
    Bool,
}

impl ParamType {
    fn parse(s: &str) -> Option<ParamType> {
        match s {
            "string" | "str" => Some(ParamType::String),
            "int" | "integer" => Some(ParamType::Int),
            "float" | "number" => Some(ParamType::Float),
            "bool" | "boolean" => Some(ParamType::Bool),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            ParamType::String => "string",
            ParamType::Int => "int",
            ParamType::Float => "float",
            ParamType::Bool => "bool",
        }
    }

    /// Coerce a `--param k=v` string into this type.
    fn coerce_str(self, raw: &str) -> Option<Value> {
        match self {
            ParamType::String => Some(json!(raw)),
            ParamType::Int => raw.trim().parse::<i64>().ok().map(|v| json!(v)),
            ParamType::Float => raw.trim().parse::<f64>().ok().map(|v| json!(v)),
            ParamType::Bool => match raw.trim().to_ascii_lowercase().as_str() {
                "true" | "yes" | "1" | "on" => Some(json!(true)),
                "false" | "no" | "0" | "off" => Some(json!(false)),
                _ => None,
            },
        }
    }

    /// Check (and normalise) a trigger's param value against this type.
    fn coerce_json(self, value: &Value) -> Option<Value> {
        self.coerce_yaml(&serde_yaml::to_value(value).ok()?)
    }

    /// Check (and normalise) a yaml default against this type.
    fn coerce_yaml(self, value: &Yaml) -> Option<Value> {
        match (self, value) {
            (ParamType::String, Yaml::String(s)) => Some(json!(s)),
            // A bare `default: 42` for a string param is fine; keep its text.
            (ParamType::String, Yaml::Number(n)) => Some(json!(n.to_string())),
            (ParamType::String, Yaml::Bool(b)) => Some(json!(b.to_string())),
            (ParamType::Int, Yaml::Number(n)) => n.as_i64().map(|v| json!(v)),
            (ParamType::Float, Yaml::Number(n)) => n.as_f64().map(|v| json!(v)),
            (ParamType::Bool, Yaml::Bool(b)) => Some(json!(b)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ParamSpec {
    #[serde(rename = "type")]
    pub ty: ParamType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Defaults {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    /// `tab`, `split` or `workspace`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orchestrator_harness: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_failure: Option<String>,
    /// How long the orchestrator and agent workers have to make their
    /// first tome call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_timeout: Option<StartTimeout>,
}

/// `defaults.start_timeout`: a duration, or `off` for no start check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartTimeout {
    Off,
    After(Duration),
}

impl StartTimeout {
    /// The timeout, or `None` when the check is off.
    pub fn duration(self) -> Option<Duration> {
        match self {
            StartTimeout::Off => None,
            StartTimeout::After(d) => Some(d),
        }
    }
}

/// `"off"`, or the number of seconds.
impl Serialize for StartTimeout {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            StartTimeout::Off => s.serialize_str("off"),
            StartTimeout::After(d) => s.serialize_u64(d.as_secs()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OnConflict {
    Queue,
    Reject,
}

#[derive(Debug, Clone, Serialize)]
pub struct Frontmatter {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub triggers: Vec<Trigger>,
    pub params: BTreeMap<String, ParamSpec>,
    pub defaults: Defaults,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_conflict: Option<OnConflict>,
    /// Extra instructions for the orchestrator, added to its bootstrap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orchestrator: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Workflow {
    pub path: PathBuf,
    pub source: String,
    pub frontmatter: Frontmatter,
    /// Raw frontmatter text (between the `---` fences).
    pub frontmatter_text: String,
    pub body: String,
    /// 1-based file line of the body's first line.
    pub body_line: usize,
}

/// A workflow file that failed validation.
#[derive(Debug, Clone)]
pub struct Invalid {
    pub path: PathBuf,
    /// The `name` from the frontmatter, if it could be read.
    pub name: Option<String>,
    pub errors: Vec<Diagnostic>,
}

impl Invalid {
    pub fn errors_json(&self) -> Value {
        Value::Array(
            self.errors
                .iter()
                .map(|d| {
                    json!({
                        "line": d.line,
                        "message": d.message,
                        "display": format!("{}:{}: {}", self.path.display(), d.line, d.message),
                    })
                })
                .collect(),
        )
    }

    pub fn into_cli_error(self) -> CliError {
        let label = self.name.clone().unwrap_or_else(|| self.path.display().to_string());
        let n = self.errors.len();
        CliError::new(
            ErrorKind::InvalidWorkflow,
            format!("workflow `{label}` is invalid ({n} error{})", if n == 1 { "" } else { "s" }),
        )
        .with_details(json!({ "path": self.path, "errors": self.errors_json() }))
    }
}

impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for d in &self.errors {
            writeln!(f, "{}:{}: {}", self.path.display(), d.line, d.message)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Parsing & validation
// ---------------------------------------------------------------------------

pub fn load(path: &Path) -> Result<Workflow, Invalid> {
    match std::fs::read_to_string(path) {
        Ok(source) => parse(path, &source),
        Err(e) => Err(Invalid {
            path: path.to_path_buf(),
            name: None,
            errors: vec![Diagnostic::new(1, format!("cannot read file: {e}"))],
        }),
    }
}

pub fn parse(path: &Path, source: &str) -> Result<Workflow, Invalid> {
    parse_source(path, source, true)
}

/// Parse a run's workflow snapshot. Its placeholders are already
/// substituted, so a param value that looks like one isn't an error.
pub fn parse_snapshot(path: &Path, source: &str) -> Result<Workflow, Invalid> {
    parse_source(path, source, false)
}

fn parse_source(path: &Path, source: &str, placeholders: bool) -> Result<Workflow, Invalid> {
    let invalid = |name: Option<String>, errors: Vec<Diagnostic>| Invalid { path: path.to_path_buf(), name, errors };

    let lines: Vec<&str> = source.lines().collect();
    if lines.first().map(|l| l.trim_end()) != Some("---") {
        return Err(invalid(None, vec![Diagnostic::new(1, "workflow must start with a `---` yaml frontmatter fence")]));
    }
    let Some(close) = lines.iter().skip(1).position(|l| matches!(l.trim_end(), "---" | "...")).map(|i| i + 1) else {
        return Err(invalid(None, vec![Diagnostic::new(1, "frontmatter is not closed with a `---` line")]));
    };
    let fm_lines = &lines[1..close];
    let frontmatter_text = fm_lines.join("\n");
    let body: String = lines[close + 1..].join("\n");
    let body_line = close + 2;
    let locator = Locator { lines: fm_lines };

    let yaml: Yaml = if frontmatter_text.trim().is_empty() {
        Yaml::Mapping(Default::default())
    } else {
        match serde_yaml::from_str(&frontmatter_text) {
            Ok(v) => v,
            Err(e) => {
                // Frontmatter starts on file line 2.
                let line = e.location().map(|l| l.line() + 1).unwrap_or(2);
                return Err(invalid(None, vec![Diagnostic::new(line, format!("invalid yaml: {}", strip_location(&e)))]));
            }
        }
    };
    let Yaml::Mapping(map) = yaml else {
        return Err(invalid(None, vec![Diagnostic::new(2, "frontmatter must be a yaml mapping of `key: value` pairs")]));
    };

    let mut errors = Vec::new();
    let mut fm = Frontmatter {
        name: String::new(),
        description: None,
        triggers: Vec::new(),
        params: BTreeMap::new(),
        defaults: Defaults::default(),
        concurrency: None,
        on_conflict: None,
        orchestrator: None,
    };

    for (key, value) in &map {
        let Some(key) = key.as_str() else {
            errors.push(Diagnostic::new(2, "frontmatter keys must be strings"));
            continue;
        };
        let line = locator.line(&[key]);
        match key {
            "name" => match value.as_str().map(str::trim) {
                Some(s) if !s.is_empty() => fm.name = s.to_string(),
                _ => errors.push(Diagnostic::new(line, "`name` must be a non-empty string")),
            },
            "description" => match value {
                Yaml::String(s) => fm.description = Some(s.clone()),
                Yaml::Null => {}
                _ => errors.push(Diagnostic::new(line, "`description` must be a string")),
            },
            "triggers" => fm.triggers = parse_triggers(value, &locator, &mut errors),
            "params" => fm.params = parse_params(value, &locator, &mut errors),
            "defaults" => fm.defaults = parse_defaults(value, &locator, &mut errors),
            "concurrency" => match value.as_u64() {
                Some(n) if n >= 1 && n <= u32::MAX as u64 => fm.concurrency = Some(n as u32),
                _ => errors.push(Diagnostic::new(line, "`concurrency` must be a positive integer")),
            },
            "on_conflict" => match value.as_str() {
                Some("queue") => fm.on_conflict = Some(OnConflict::Queue),
                Some("reject") => fm.on_conflict = Some(OnConflict::Reject),
                _ => errors.push(Diagnostic::new(line, "`on_conflict` must be `queue` or `reject`")),
            },
            "orchestrator" => match value {
                Yaml::String(s) => fm.orchestrator = Some(s.clone()),
                Yaml::Null => {}
                _ => errors.push(Diagnostic::new(line, "`orchestrator` must be a string of extra instructions")),
            },
            other => errors.push(Diagnostic::new(
                line,
                format!("unknown frontmatter key `{other}` (expected one of: {})", TOP_LEVEL_KEYS.join(", ")),
            )),
        }
    }
    check_trigger_params(&mut fm, &mut errors);
    if !map.contains_key("name") {
        errors.push(Diagnostic::new(1, "missing required frontmatter key `name`"));
    }

    if placeholders {
        errors.extend(check_placeholders(&body, body_line, &fm.params));
    }

    let name = (!fm.name.is_empty()).then(|| fm.name.clone());
    if !errors.is_empty() {
        errors.sort_by_key(|d| d.line);
        return Err(invalid(name, errors));
    }
    Ok(Workflow { path: path.to_path_buf(), source: source.to_string(), frontmatter: fm, frontmatter_text, body, body_line })
}

fn strip_location(e: &serde_yaml::Error) -> String {
    // serde_yaml appends " at line X column Y"; we report the line ourselves.
    let msg = e.to_string();
    match msg.find(" at line ") {
        Some(idx) => msg[..idx].to_string(),
        None => msg,
    }
}

/// A trigger rule from the frontmatter.
#[derive(Debug, Clone, Serialize)]
pub struct Trigger {
    #[serde(flatten)]
    pub kind: TriggerKind,
    pub to: Target,
    pub params: Map<String, Value>,
    /// File line of the trigger's list item.
    #[serde(skip)]
    pub line: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum TriggerKind {
    Manual,
    File(FileTrigger),
    Cron {
        cron: String,
        #[serde(skip)]
        schedule: crate::cron::Cron,
    },
    /// `on: <pattern>`: events published to a matching topic.
    Topic {
        on: crate::topic::Pattern,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct FileTrigger {
    pub file: String,
    pub on: Vec<FileEvent>,
    #[serde(serialize_with = "ser_secs")]
    pub debounce: Duration,
    pub ignore: Vec<String>,
    pub while_running: WhileRunning,
    #[serde(skip)]
    pub glob: crate::glob::Glob,
    #[serde(skip)]
    pub ignore_globs: Vec<crate::glob::Glob>,
}

fn ser_secs<S: serde::Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_f64(d.as_secs_f64())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FileEvent {
    Created,
    Modified,
}

impl FileEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            FileEvent::Created => "created",
            FileEvent::Modified => "modified",
        }
    }
}

/// What a file trigger does with changes made while a run of its workflow
/// is active (`while_running:`). Only applies to `to: new`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WhileRunning {
    /// Queue one run that starts once the workflow is idle; later batches
    /// merge into it.
    #[default]
    Queue,
    /// Start a run for each batch, through `concurrency` / `on_conflict`.
    Parallel,
    /// Drop the changes.
    Mute,
}

impl WhileRunning {
    pub fn as_str(self) -> &'static str {
        match self {
            WhileRunning::Queue => "queue",
            WhileRunning::Parallel => "parallel",
            WhileRunning::Mute => "mute",
        }
    }
}

/// Where a fired trigger goes (`to:`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Target {
    New,
    Running,
    RunningOrNew,
}

impl Target {
    pub fn as_str(self) -> &'static str {
        match self {
            Target::New => "new",
            Target::Running => "running",
            Target::RunningOrNew => "running-or-new",
        }
    }
}

impl Trigger {
    pub fn kind_name(&self) -> &'static str {
        match self.kind {
            TriggerKind::Manual => "manual",
            TriggerKind::File(_) => "file",
            TriggerKind::Cron { .. } => "cron",
            TriggerKind::Topic { .. } => "topic",
        }
    }

    /// `file specs/**/*.md`, `cron 0 9 * * 1-5`, `manual`.
    pub fn describe(&self) -> String {
        match &self.kind {
            TriggerKind::Manual => "manual".into(),
            TriggerKind::File(f) => format!("file {}", f.file),
            TriggerKind::Cron { cron, .. } => format!("cron {cron}"),
            TriggerKind::Topic { on } => format!("on {on}"),
        }
    }

    /// Whether this trigger drops events while a run of its workflow is
    /// active (a `to: new` file trigger with `while_running: mute`).
    pub fn mutes_while_running(&self) -> bool {
        self.to == Target::New && matches!(&self.kind, TriggerKind::File(f) if f.while_running == WhileRunning::Mute)
    }
}

pub const TRIGGER_KEYS: &[&str] = &["file", "cron", "on", "debounce", "ignore", "while_running", "to", "params"];
pub const TRIGGER_FIELDS: &[&str] =
    &["kind", "paths", "event", "time", "scheduled", "topic", "payload", "event_id", "sender"];
const DEFAULT_DEBOUNCE: Duration = Duration::from_secs(2);

fn parse_triggers(value: &Yaml, loc: &Locator, errors: &mut Vec<Diagnostic>) -> Vec<Trigger> {
    let line = loc.line(&["triggers"]);
    let items: Vec<&Yaml> = match value {
        Yaml::Null => return Vec::new(),
        Yaml::String(_) | Yaml::Mapping(_) => vec![value],
        Yaml::Sequence(seq) => seq.iter().collect(),
        _ => {
            errors.push(Diagnostic::new(line, "`triggers` must be a list"));
            return Vec::new();
        }
    };
    let item_lines = loc.list_items(&["triggers"]);
    let mut out = Vec::new();
    for (i, item) in items.into_iter().enumerate() {
        let line = item_lines.get(i).copied().unwrap_or(line);
        let before = errors.len();
        let trigger = parse_trigger(item, line, errors);
        if errors.len() == before {
            out.extend(trigger);
        }
    }
    out
}

fn parse_trigger(item: &Yaml, line: usize, errors: &mut Vec<Diagnostic>) -> Option<Trigger> {
    let n = |errors: &mut Vec<Diagnostic>, msg: String| errors.push(Diagnostic::new(line, msg));
    let map = match item {
        Yaml::String(s) if s == "manual" => {
            return Some(Trigger { kind: TriggerKind::Manual, to: Target::New, params: Map::new(), line })
        }
        Yaml::String(s) => {
            n(errors, format!("unknown trigger `{s}` (expected `manual`, or a mapping with `file:`, `cron:` or `on:`)"));
            return None;
        }
        Yaml::Mapping(m) => m,
        _ => {
            n(errors, "each trigger must be `manual` or a mapping with `file:`, `cron:` or `on:`".into());
            return None;
        }
    };
    let get = |k: &str| map.get(k);
    for key in map.keys() {
        match key.as_str() {
            Some(k) if TRIGGER_KEYS.contains(&k) => {}
            Some(k) => n(errors, format!("unknown trigger key `{k}` (expected one of: {})", TRIGGER_KEYS.join(", "))),
            None => n(errors, "trigger keys must be strings".into()),
        }
    }

    let to = match get("to").map(|v| v.as_str()) {
        None => Target::New,
        Some(Some("new")) => Target::New,
        Some(Some("running")) => Target::Running,
        Some(Some("running-or-new")) => Target::RunningOrNew,
        Some(_) => {
            n(errors, "trigger `to` must be `new`, `running` or `running-or-new`".into());
            Target::New
        }
    };
    let params = match get("params") {
        None | Some(Yaml::Null) => Map::new(),
        Some(Yaml::Mapping(m)) => {
            let mut out = Map::new();
            for (k, v) in m {
                match k.as_str() {
                    Some(k) => {
                        out.insert(k.to_string(), yaml_to_json(v));
                    }
                    None => n(errors, "trigger `params` keys must be strings".into()),
                }
            }
            out
        }
        Some(_) => {
            n(errors, "trigger `params` must be a mapping of param names to values".into());
            Map::new()
        }
    };

    let kind = match (get("file"), get("cron")) {
        (Some(_), Some(_)) => {
            n(errors, "a trigger has either `file:` or `cron:`, not both".into());
            return None;
        }
        (None, None) => {
            let Some(on) = get("on") else {
                n(errors, "a trigger mapping needs `file:`, `cron:` or `on:`".into());
                return None;
            };
            for k in ["debounce", "ignore", "while_running"] {
                if get(k).is_some() {
                    n(errors, format!("`{k}` only applies to file triggers"));
                }
            }
            let Some(pattern) = on.as_str() else {
                n(errors, "`on` must be a topic pattern like `review.requested`".into());
                return None;
            };
            match crate::topic::Pattern::parse(pattern) {
                Ok(on) => TriggerKind::Topic { on },
                Err(e) => {
                    n(errors, e);
                    return None;
                }
            }
        }
        (None, Some(cron)) => {
            for k in ["on", "debounce", "ignore", "while_running"] {
                if get(k).is_some() {
                    n(errors, format!("`{k}` only applies to file triggers"));
                }
            }
            let Some(expr) = cron.as_str() else {
                n(errors, "`cron` must be a quoted 5-field cron expression".into());
                return None;
            };
            match crate::cron::Cron::parse(expr) {
                Ok(schedule) => TriggerKind::Cron { cron: expr.to_string(), schedule },
                Err(e) => {
                    n(errors, e);
                    return None;
                }
            }
        }
        (Some(file), None) => {
            let Some(pattern) = file.as_str() else {
                n(errors, "`file` must be a glob string".into());
                return None;
            };
            let glob = match crate::glob::Glob::new(pattern) {
                Ok(g) => Some(g),
                Err(e) => {
                    n(errors, e);
                    None
                }
            };
            let on = match get("on") {
                None => vec![FileEvent::Created, FileEvent::Modified],
                Some(v) => {
                    let items: Vec<&Yaml> = match v {
                        Yaml::Sequence(s) => s.iter().collect(),
                        other => vec![other],
                    };
                    let mut on = Vec::new();
                    for item in items {
                        match item.as_str() {
                            Some("created") => on.push(FileEvent::Created),
                            Some("modified") => on.push(FileEvent::Modified),
                            _ => n(errors, "`on` must list `created` and/or `modified`".into()),
                        }
                    }
                    if on.is_empty() {
                        n(errors, "`on` must list `created` and/or `modified`".into());
                    }
                    on.dedup();
                    on
                }
            };
            let debounce = match get("debounce") {
                None => DEFAULT_DEBOUNCE,
                Some(v) => {
                    let raw = match v {
                        Yaml::String(s) => s.clone(),
                        Yaml::Number(num) => num.to_string(),
                        _ => String::new(),
                    };
                    match crate::duration::parse(&raw) {
                        Ok(d) => d,
                        Err(e) => {
                            n(errors, format!("`debounce`: {e}"));
                            DEFAULT_DEBOUNCE
                        }
                    }
                }
            };
            let ignore: Vec<String> = match get("ignore") {
                None | Some(Yaml::Null) => Vec::new(),
                Some(Yaml::String(s)) => vec![s.clone()],
                Some(Yaml::Sequence(seq)) if seq.iter().all(|v| v.as_str().is_some()) => {
                    seq.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()
                }
                Some(_) => {
                    n(errors, "`ignore` must be a list of globs".into());
                    Vec::new()
                }
            };
            let mut ignore_globs = Vec::new();
            for g in &ignore {
                match crate::glob::Glob::new(g) {
                    Ok(g) => ignore_globs.push(g),
                    Err(e) => n(errors, format!("`ignore`: {e}")),
                }
            }
            let while_running = match get("while_running") {
                None => WhileRunning::default(),
                Some(v) => {
                    if to != Target::New {
                        n(errors, format!("`while_running` only applies to `to: new`, not `to: {}`", to.as_str()));
                    }
                    match v.as_str() {
                        Some("queue") => WhileRunning::Queue,
                        Some("parallel") => WhileRunning::Parallel,
                        Some("mute") => WhileRunning::Mute,
                        _ => {
                            n(errors, "`while_running` must be `queue`, `parallel` or `mute`".into());
                            WhileRunning::default()
                        }
                    }
                }
            };
            TriggerKind::File(FileTrigger { file: pattern.to_string(), on, debounce, ignore, while_running, glob: glob?, ignore_globs })
        }
    };
    Some(Trigger { kind, to, params, line })
}

/// Trigger checks that need the whole frontmatter: each trigger's `params`
/// must name real params, fit their types, and together with the defaults
/// fill every required one.
fn check_trigger_params(fm: &mut Frontmatter, errors: &mut Vec<Diagnostic>) {
    for t in &mut fm.triggers {
        if matches!(t.kind, TriggerKind::Manual) {
            continue;
        }
        let mut coerced = Map::new();
        for (k, v) in &t.params {
            match fm.params.get(k) {
                None => errors.push(Diagnostic::new(t.line, format!("trigger `{}`: unknown param `{k}`", t.describe()))),
                Some(spec) => match spec.ty.coerce_json(v) {
                    Some(v) => {
                        coerced.insert(k.clone(), v);
                    }
                    None => errors.push(Diagnostic::new(
                        t.line,
                        format!("trigger `{}`: param `{k}`: `{}` is not a valid {}", t.describe(), value_text(v), spec.ty.name()),
                    )),
                },
            }
        }
        for (name, spec) in &fm.params {
            if spec.default.is_none() && !t.params.contains_key(name) {
                errors.push(Diagnostic::new(
                    t.line,
                    format!("trigger `{}` leaves required param `{name}` unfilled; set it in the trigger's `params:`", t.describe()),
                ));
            }
        }
        t.params = coerced;
    }
}

/// Checks that depend on where the workflow lives: a global workflow has no
/// project root, so its file triggers need absolute or `~/` globs, and no
/// project bus, so it can't have topic triggers.
pub fn check_scope(wf: Workflow, scope: Scope) -> Result<Workflow, Invalid> {
    if scope == Scope::Project {
        return Ok(wf);
    }
    let errors: Vec<Diagnostic> = wf
        .frontmatter
        .triggers
        .iter()
        .filter_map(|t| match &t.kind {
            TriggerKind::File(f) if !crate::glob::Glob::is_absolute(&f.file) => Some(Diagnostic::new(
                t.line,
                format!("file trigger `{}` in a global workflow must use an absolute or `~/` path", f.file),
            )),
            TriggerKind::Topic { on } => Some(Diagnostic::new(
                t.line,
                format!("topic trigger `on: {on}` needs a project; a global workflow has no project bus"),
            )),
            _ => None,
        })
        .collect();
    if errors.is_empty() {
        Ok(wf)
    } else {
        Err(Invalid { path: wf.path.clone(), name: Some(wf.name().to_string()), errors })
    }
}

fn parse_params(value: &Yaml, loc: &Locator, errors: &mut Vec<Diagnostic>) -> BTreeMap<String, ParamSpec> {
    let mut out = BTreeMap::new();
    let map = match value {
        Yaml::Null => return out,
        Yaml::Mapping(m) => m,
        _ => {
            errors.push(Diagnostic::new(loc.line(&["params"]), "`params` must be a mapping of name to spec"));
            return out;
        }
    };
    for (key, spec) in map {
        let Some(name) = key.as_str() else {
            errors.push(Diagnostic::new(loc.line(&["params"]), "param names must be strings"));
            continue;
        };
        let line = loc.line(&["params", name]);
        if !is_identifier(name) {
            errors.push(Diagnostic::new(line, format!("invalid param name `{name}` (use letters, digits, `_` or `-`)")));
            continue;
        }
        let parsed = match spec {
            // Shorthand: `base: main` is a string param defaulting to "main".
            Yaml::String(_) | Yaml::Number(_) | Yaml::Bool(_) => {
                let ty = match spec {
                    Yaml::Number(n) if n.is_i64() || n.is_u64() => ParamType::Int,
                    Yaml::Number(_) => ParamType::Float,
                    Yaml::Bool(_) => ParamType::Bool,
                    _ => ParamType::String,
                };
                Some(ParamSpec { ty, default: ty.coerce_yaml(spec), description: None })
            }
            Yaml::Null => Some(ParamSpec { ty: ParamType::String, default: None, description: None }),
            Yaml::Mapping(m) => parse_param_spec(name, m, loc, errors),
            _ => {
                errors.push(Diagnostic::new(line, format!("param `{name}` must be a default value or a mapping")));
                None
            }
        };
        if let Some(p) = parsed {
            out.insert(name.to_string(), p);
        }
    }
    out
}

fn parse_param_spec(
    name: &str,
    m: &serde_yaml::Mapping,
    loc: &Locator,
    errors: &mut Vec<Diagnostic>,
) -> Option<ParamSpec> {
    let before = errors.len();
    let mut ty = ParamType::String;
    let mut explicit_type = false;
    let mut description = None;
    for (k, v) in m {
        let key = k.as_str().unwrap_or("");
        let line = loc.line(&["params", name, key]);
        match key {
            "type" => match v.as_str().and_then(ParamType::parse) {
                Some(t) => {
                    ty = t;
                    explicit_type = true;
                }
                None => errors.push(Diagnostic::new(line, format!("param `{name}`: `type` must be one of string, int, float, bool"))),
            },
            "description" => match v.as_str() {
                Some(s) => description = Some(s.to_string()),
                None => errors.push(Diagnostic::new(line, format!("param `{name}`: `description` must be a string"))),
            },
            "default" => {}
            other => errors.push(Diagnostic::new(
                line,
                format!("param `{name}`: unknown key `{other}` (expected one of: {})", PARAM_KEYS.join(", ")),
            )),
        }
    }
    let default = match m.get("default") {
        None | Some(Yaml::Null) => None,
        Some(v) => {
            // Without an explicit type, infer it from the default.
            if !explicit_type {
                ty = match v {
                    Yaml::Number(n) if n.is_i64() || n.is_u64() => ParamType::Int,
                    Yaml::Number(_) => ParamType::Float,
                    Yaml::Bool(_) => ParamType::Bool,
                    _ => ParamType::String,
                };
            }
            match ty.coerce_yaml(v) {
                Some(d) => Some(d),
                None => {
                    errors.push(Diagnostic::new(
                        loc.line(&["params", name, "default"]),
                        format!("param `{name}`: default does not match type `{}`", ty.name()),
                    ));
                    None
                }
            }
        }
    };
    (errors.len() == before).then_some(ParamSpec { ty, default, description })
}

fn parse_defaults(value: &Yaml, loc: &Locator, errors: &mut Vec<Diagnostic>) -> Defaults {
    let mut d = Defaults::default();
    let map = match value {
        Yaml::Null => return d,
        Yaml::Mapping(m) => m,
        _ => {
            errors.push(Diagnostic::new(loc.line(&["defaults"]), "`defaults` must be a mapping"));
            return d;
        }
    };
    for (k, v) in map {
        let key = k.as_str().unwrap_or("");
        let line = loc.line(&["defaults", key]);
        let as_string = |errors: &mut Vec<Diagnostic>| match v.as_str() {
            Some(s) => Some(s.to_string()),
            None => {
                errors.push(Diagnostic::new(line, format!("`defaults.{key}` must be a string")));
                None
            }
        };
        match key {
            "backend" => d.backend = as_string(errors),
            "layout" => match as_string(errors) {
                Some(l) if crate::session::Layout::parse(&l).is_none() => errors.push(Diagnostic::new(
                    line,
                    format!("unknown `defaults.layout` `{l}` (expected one of: {})", crate::session::Layout::NAMES.join(", ")),
                )),
                l => d.layout = l,
            },
            "harness" => d.harness = as_string(errors),
            "orchestrator_harness" => d.orchestrator_harness = as_string(errors),
            "on_failure" => d.on_failure = as_string(errors),
            "timeout" => {
                let parsed = match v {
                    Yaml::Number(n) => n.as_u64().ok_or_else(|| "must be a positive number of seconds".to_string()),
                    Yaml::String(s) => duration::parse(s).map(|d| d.as_secs()),
                    _ => Err("must be a duration like `30m` or a number of seconds".to_string()),
                };
                match parsed {
                    Ok(secs) => d.timeout_secs = Some(secs),
                    Err(e) => errors.push(Diagnostic::new(line, format!("`defaults.timeout` {e}"))),
                }
            }
            "start_timeout" => {
                let parsed = match v {
                    Yaml::String(s) if s.trim() == "off" => Ok(StartTimeout::Off),
                    Yaml::Bool(false) => Ok(StartTimeout::Off),
                    Yaml::Number(n) => n.as_u64().ok_or_else(|| "must be a positive number of seconds".to_string()).map(Duration::from_secs).map(StartTimeout::After),
                    Yaml::String(s) => duration::parse(s).map(StartTimeout::After),
                    _ => Err("must be a duration like `2m`, or `off`".to_string()),
                };
                match parsed {
                    Ok(StartTimeout::After(d)) if d.is_zero() => {
                        errors.push(Diagnostic::new(line, "`defaults.start_timeout` must be longer than 0s (use `off` to turn the check off)"))
                    }
                    Ok(t) => d.start_timeout = Some(t),
                    Err(e) => errors.push(Diagnostic::new(line, format!("`defaults.start_timeout` {e}"))),
                }
            }
            other => errors.push(Diagnostic::new(
                line,
                format!("unknown key `defaults.{other}` (expected one of: {})", DEFAULTS_KEYS.join(", ")),
            )),
        }
    }
    d
}

pub fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Finds the file line of a (possibly nested) block-style yaml key. Falls
/// back to the closest ancestor found, or the first frontmatter line.
struct Locator<'a> {
    lines: &'a [&'a str],
}

impl Locator<'_> {
    fn line(&self, path: &[&str]) -> usize {
        let mut start = 0;
        let mut end = self.lines.len();
        let mut parent_indent: Option<usize> = None;
        let mut found = None;
        for key in path {
            let hit = (start..end).find(|&i| {
                let line = self.lines[i];
                let indent = line.len() - line.trim_start().len();
                let ok_indent = match parent_indent {
                    None => indent == 0,
                    Some(p) => indent > p,
                };
                ok_indent && key_of(line.trim_start()) == Some(key)
            });
            let Some(i) = hit else { break };
            found = Some(i);
            let indent = self.lines[i].len() - self.lines[i].trim_start().len();
            // The key's block runs until the next line at the same or lower indent.
            let block_end = (i + 1..end)
                .find(|&j| {
                    let l = self.lines[j];
                    !l.trim().is_empty() && !l.trim_start().starts_with('#') && l.len() - l.trim_start().len() <= indent
                })
                .unwrap_or(end);
            start = i + 1;
            end = block_end;
            parent_indent = Some(indent);
        }
        // +2: frontmatter line 0 is file line 2.
        found.map(|i| i + 2).unwrap_or(2)
    }
}

impl Locator<'_> {
    /// File lines of the `- ` items of a block-style list under `path`.
    fn list_items(&self, path: &[&str]) -> Vec<usize> {
        let key_line = self.line(path);
        let Some(key_idx) = key_line.checked_sub(2).filter(|&i| i < self.lines.len()) else { return Vec::new() };
        let indent_of = |l: &str| l.len() - l.trim_start().len();
        let key_indent = indent_of(self.lines[key_idx]);
        let meaningful = |l: &str| !l.trim().is_empty() && !l.trim_start().starts_with('#');
        let Some(first) = self.lines[key_idx + 1..].iter().find(|l| meaningful(l)) else { return Vec::new() };
        let item_indent = indent_of(first);
        if !first.trim_start().starts_with('-') || item_indent < key_indent {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (i, l) in self.lines.iter().enumerate().skip(key_idx + 1) {
            if !meaningful(l) {
                continue;
            }
            let indent = indent_of(l);
            if indent < item_indent || (indent == item_indent && !l.trim_start().starts_with('-')) {
                break;
            }
            if indent == item_indent {
                out.push(i + 2);
            }
        }
        out
    }
}

fn key_of(line: &str) -> Option<&str> {
    let line = line.strip_prefix("- ").unwrap_or(line);
    let colon = line.find(':')?;
    let key = line[..colon].trim();
    Some(key.trim_matches(|c| c == '"' || c == '\''))
}

// ---------------------------------------------------------------------------
// Placeholders & parameter resolution
// ---------------------------------------------------------------------------

/// A `{{ expr }}` occurrence within one line.
struct Placeholder<'a> {
    start: usize,
    end: usize,
    expr: &'a str,
}

fn placeholders(line: &str) -> Vec<Placeholder<'_>> {
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(open) = line[pos..].find("{{").map(|i| i + pos) {
        let Some(close) = line[open + 2..].find("}}").map(|i| i + open + 2) else { break };
        out.push(Placeholder { start: open, end: close + 2, expr: line[open + 2..close].trim() });
        pos = close + 2;
    }
    out
}

fn check_placeholders(body: &str, body_line: usize, params: &BTreeMap<String, ParamSpec>) -> Vec<Diagnostic> {
    let mut errors = Vec::new();
    for (i, line) in body.lines().enumerate() {
        for ph in placeholders(line) {
            let ok = match ph.expr.split_once('.') {
                Some(("params", name)) => params.contains_key(name),
                Some(("run", "id")) => true,
                Some(("trigger", field)) => TRIGGER_FIELDS.contains(&field),
                _ => false,
            };
            if !ok {
                let msg = match ph.expr.split_once('.') {
                    Some(("params", name)) => format!("undefined placeholder `{{{{{}}}}}`: no param named `{name}`", ph.expr),
                    _ => format!(
                        "undefined placeholder `{{{{{}}}}}` (available: `params.<name>`, `run.id`, `trigger.{}`)",
                        ph.expr,
                        TRIGGER_FIELDS.join("|")
                    ),
                };
                errors.push(Diagnostic::new(body_line + i, msg));
            }
        }
    }
    errors
}

/// Parse `k=v` pairs from `--param` flags.
pub fn parse_param_args(args: &[String]) -> CliResult<Vec<(String, String)>> {
    args.iter()
        .map(|a| match a.split_once('=') {
            Some((k, v)) if !k.trim().is_empty() => Ok((k.trim().to_string(), v.to_string())),
            _ => Err(CliError::invalid(format!("invalid --param `{a}`: expected key=value"))),
        })
        .collect()
}

impl Workflow {
    pub fn name(&self) -> &str {
        &self.frontmatter.name
    }

    /// Things `tome validate` warns about without failing: topic triggers
    /// that can match the same topic (an event is delivered once, for the
    /// first), and patterns that match the workflow's own lifecycle events.
    pub fn warnings(&self) -> Vec<Diagnostic> {
        let topics: Vec<(&Trigger, &crate::topic::Pattern)> = self
            .frontmatter
            .triggers
            .iter()
            .filter_map(|t| match &t.kind {
                TriggerKind::Topic { on } => Some((t, on)),
                _ => None,
            })
            .collect();
        let mut out = Vec::new();
        for (i, (t, on)) in topics.iter().enumerate() {
            if let Some((_, first)) = topics[..i].iter().find(|(_, earlier)| earlier.overlaps(on)) {
                out.push(Diagnostic::new(
                    t.line,
                    format!("`on: {on}` and `on: {first}` can match the same topic; such an event is delivered once, for `on: {first}`"),
                ));
            }
            if crate::topic::lifecycle_topics(self.name()).iter().any(|topic| on.matches(topic)) {
                out.push(Diagnostic::new(
                    t.line,
                    format!("`on: {on}` matches this workflow's own `tome.run.{}.*` events, so its runs can start more of its runs", self.name()),
                ));
            }
        }
        out
    }

    /// Combine declared defaults with `--param` overrides. Unknown params,
    /// values of the wrong type and required params without a value are
    /// errors.
    pub fn resolve_params(&self, overrides: &[(String, String)], require_all: bool) -> Result<Map<String, Value>, Invalid> {
        let mut errors = Vec::new();
        let mut values = Map::new();
        for (key, raw) in overrides {
            match self.frontmatter.params.get(key) {
                None => errors.push(Diagnostic::new(1, format!("unknown param `{key}` passed with --param"))),
                Some(spec) => match spec.ty.coerce_str(raw) {
                    Some(v) => {
                        values.insert(key.clone(), v);
                    }
                    None => errors.push(Diagnostic::new(
                        1,
                        format!("param `{key}`: `{raw}` is not a valid {}", spec.ty.name()),
                    )),
                },
            }
        }
        for (name, spec) in &self.frontmatter.params {
            if values.contains_key(name) {
                continue;
            }
            match &spec.default {
                Some(d) => {
                    values.insert(name.clone(), d.clone());
                }
                None if require_all => errors.push(Diagnostic::new(
                    1,
                    format!("param `{name}` has no default; pass it with --param {name}=<value>"),
                )),
                None => {}
            }
        }
        if errors.is_empty() {
            Ok(values)
        } else {
            Err(Invalid { path: self.path.clone(), name: Some(self.name().to_string()), errors })
        }
    }

    /// Substitute `{{params.x}}`, `{{run.id}}` and `{{trigger.x}}` into the
    /// body. Plain text substitution only. `trigger` holds the event that
    /// started the run; its fields are empty for a manual run.
    pub fn render_body(&self, params: &Map<String, Value>, run_id: &str, trigger: &Map<String, Value>) -> String {
        let mut out = String::with_capacity(self.body.len());
        for (i, line) in self.body.split('\n').enumerate() {
            if i > 0 {
                out.push('\n');
            }
            let mut last = 0;
            for ph in placeholders(line) {
                let replacement = match ph.expr.split_once('.') {
                    Some(("params", name)) => params.get(name).map(value_text),
                    Some(("run", "id")) => Some(run_id.to_string()),
                    Some(("trigger", field)) if TRIGGER_FIELDS.contains(&field) => {
                        Some(trigger.get(field).map(trigger_text).unwrap_or_default())
                    }
                    _ => None,
                };
                if let Some(text) = replacement {
                    out.push_str(&line[last..ph.start]);
                    out.push_str(&text);
                    last = ph.end;
                }
            }
            out.push_str(&line[last..]);
        }
        out
    }

    /// The workflow as saved with a run whose placeholders are filled in
    /// when it starts: [`render_snapshot`](Self::render_snapshot) without the
    /// substitution. [`parse`] reads it back.
    pub fn template_snapshot(&self) -> String {
        format!("---\n{}\n---\n{}", self.frontmatter_text, self.body)
    }

    /// The resolved workflow as saved with a run: the original frontmatter
    /// followed by the substituted body.
    pub fn render_snapshot(&self, params: &Map<String, Value>, run_id: &str, trigger: &Map<String, Value>) -> String {
        format!("---\n{}\n---\n{}", self.frontmatter_text, self.render_body(params, run_id, trigger))
    }
}

fn value_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// A trigger field as text: `paths` (a list of `{path, event}`) reads
/// `specs/a.md (created), specs/b.md (modified)`.
pub fn trigger_text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Array(items) => items
            .iter()
            .map(|i| match (i["path"].as_str(), i["event"].as_str()) {
                (Some(p), Some(e)) => format!("{p} ({e})"),
                _ => value_text(i),
            })
            .collect::<Vec<_>>()
            .join(", "),
        other => value_text(other),
    }
}

fn yaml_to_json(v: &Yaml) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

// ---------------------------------------------------------------------------
// Lookup
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Global,
    Project,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub scope: Scope,
    pub path: PathBuf,
    pub result: Result<Workflow, Invalid>,
}

impl Entry {
    pub fn name(&self) -> Option<&str> {
        match &self.result {
            Ok(wf) => Some(wf.name()),
            Err(inv) => inv.name.as_deref(),
        }
    }
}

/// The workflow directories visible from a working directory:
/// `~/.tome/workflows` (global) and the nearest `.tome/workflows` found by
/// walking up from the working directory (project).
pub struct Library {
    pub global_dir: PathBuf,
    pub project_dir: Option<PathBuf>,
}

impl Library {
    pub fn discover(cwd: &Path) -> Library {
        let home = crate::paths::tome_home();
        let global_dir = home.join("workflows");
        let project_dir = cwd.ancestors().find_map(|dir| {
            let tome = dir.join(".tome");
            let candidate = tome.join("workflows");
            (candidate.is_dir() && !same_path(&tome, &home)).then_some(candidate)
        });
        Library { global_dir, project_dir }
    }

    /// A workflow file outside the global directory is treated as a project
    /// one.
    pub fn scope_of(&self, path: &Path) -> Scope {
        match path.parent() {
            Some(dir) if same_path(dir, &self.global_dir) => Scope::Global,
            _ => Scope::Project,
        }
    }

    /// The project root (directory containing `.tome`), if any.
    pub fn project_root(&self) -> Option<PathBuf> {
        self.project_dir.as_ref().and_then(|d| d.parent()?.parent().map(Path::to_path_buf))
    }

    /// Every workflow file in both directories, global first.
    pub fn entries(&self) -> Vec<Entry> {
        let mut out = Vec::new();
        for (scope, dir) in [(Scope::Global, Some(&self.global_dir)), (Scope::Project, self.project_dir.as_ref())] {
            let Some(dir) = dir else { continue };
            let Ok(read) = std::fs::read_dir(dir) else { continue };
            let mut paths: Vec<PathBuf> = read
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "md"))
                .collect();
            paths.sort();
            for path in paths {
                let result = load(&path).and_then(|wf| check_scope(wf, scope));
                out.push(Entry { scope, path, result });
            }
        }
        out
    }

    /// Find a workflow by `name` (project wins over global) or by file path.
    pub fn find(&self, target: &str) -> CliResult<Workflow> {
        self.locate(target)?.map_err(Invalid::into_cli_error)
    }

    /// Like [`find`](Self::find), but an invalid workflow is returned as
    /// `Ok(Err(..))` so callers can report its diagnostics. Only "not found"
    /// and an ambiguous name are errors.
    pub fn locate(&self, target: &str) -> CliResult<Result<Workflow, Invalid>> {
        let as_path = Path::new(target);
        if target.contains('/') || target.ends_with(".md") {
            if as_path.is_file() {
                return Ok(load(as_path).and_then(|wf| check_scope(wf, self.scope_of(as_path))));
            }
            if target.contains('/') {
                return Err(CliError::not_found(format!("no workflow file at `{target}`")));
            }
        }

        let entries = self.entries();
        for scope in [Scope::Project, Scope::Global] {
            let matches: Vec<&Entry> = entries.iter().filter(|e| e.scope == scope && e.name() == Some(target)).collect();
            match matches.as_slice() {
                [] => {}
                [only] => return Ok(only.result.clone()),
                many => {
                    let paths: Vec<String> = many.iter().map(|e| e.path.display().to_string()).collect();
                    return Err(CliError::new(
                        ErrorKind::InvalidWorkflow,
                        format!("workflow name `{target}` is defined more than once: {}", paths.join(", ")),
                    ));
                }
            }
        }
        // A file named after the target whose frontmatter couldn't be read is
        // most likely what was meant; surface its errors.
        if let Some(e) = entries.iter().rev().find(|e| e.path.file_stem().is_some_and(|s| s == target)) {
            if e.result.is_err() {
                return Ok(e.result.clone());
            }
        }
        Err(CliError::not_found(format!("no workflow named `{target}`")).with_hint(format!(
            "workflows are loaded from {}{}",
            self.global_dir.display(),
            self.project_dir.as_ref().map(|d| format!(" and {}", d.display())).unwrap_or_default()
        )))
    }
}

pub(crate) fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(src: &str) -> Result<Workflow, Invalid> {
        parse(Path::new("wf.md"), src)
    }

    fn errs(src: &str) -> Vec<Diagnostic> {
        p(src).expect_err("expected invalid").errors
    }

    const GOOD: &str = "---
name: review-loop
description: Implement then review
triggers: [manual]
params:
  base: {default: main}
  retries:
    type: int
    default: 3
  ticket:
    description: which ticket
defaults:
  harness: claude
  timeout: 30m
concurrency: 2
on_conflict: queue
---
## Implement
Create a worktree off {{params.base}} for run {{ run.id }}.
Retry up to {{params.retries}} times on {{params.ticket}}.
";

    #[test]
    fn parses_a_valid_workflow() {
        let wf = p(GOOD).unwrap();
        assert_eq!(wf.name(), "review-loop");
        assert_eq!(wf.frontmatter.params["retries"].ty, ParamType::Int);
        assert_eq!(wf.frontmatter.params["base"].default, Some(json!("main")));
        assert_eq!(wf.frontmatter.defaults.timeout_secs, Some(1800));
        assert_eq!(wf.frontmatter.concurrency, Some(2));
        assert_eq!(wf.frontmatter.on_conflict, Some(OnConflict::Queue));
        assert_eq!(wf.body_line, 18);
        assert!(wf.body.starts_with("## Implement"));
    }

    #[test]
    fn layout_is_tab_split_or_workspace() {
        let with = |v: &str| p(&format!("---\nname: x\ndefaults:\n  layout: {v}\n---\n## A\n"));
        for ok in ["tab", "split", "workspace"] {
            assert_eq!(with(ok).unwrap().frontmatter.defaults.layout.as_deref(), Some(ok));
        }
        for bad in ["tabs", "[tab]"] {
            let err = with(bad).unwrap_err();
            assert!(err.errors[0].message.contains("defaults.layout"), "{bad}: {:?}", err.errors);
        }
    }

    #[test]
    fn start_timeout_is_a_duration_or_off() {
        let with = |v: &str| p(&format!("---\nname: x\ndefaults:\n  start_timeout: {v}\n---\n## A\n"));
        let timeout = |v: &str| with(v).unwrap().frontmatter.defaults.start_timeout;
        assert_eq!(timeout("2m"), Some(StartTimeout::After(Duration::from_secs(120))));
        assert_eq!(timeout("90"), Some(StartTimeout::After(Duration::from_secs(90))));
        assert_eq!(timeout("off"), Some(StartTimeout::Off));
        assert_eq!(timeout("\"off\""), Some(StartTimeout::Off));
        for bad in ["soon", "0s", "[1]", "-5"] {
            let err = with(bad).unwrap_err();
            assert!(err.errors[0].message.contains("defaults.start_timeout"), "{bad}: {:?}", err.errors);
        }
    }

    #[test]
    fn resolves_and_renders() {
        let wf = p(GOOD).unwrap();
        let params = wf
            .resolve_params(&[("base".into(), "dev".into()), ("ticket".into(), "T-1".into())], true)
            .unwrap();
        assert_eq!(params["retries"], json!(3));
        let body = wf.render_body(&params, "42", &Map::new());
        assert!(body.contains("off dev for run 42."));
        assert!(body.contains("up to 3 times on T-1."));
        let snap = wf.render_snapshot(&params, "42", &Map::new());
        assert!(snap.starts_with("---\nname: review-loop"));
        assert!(snap.contains("off dev for run 42."));
    }

    #[test]
    fn param_resolution_errors() {
        let wf = p(GOOD).unwrap();
        let e = wf.resolve_params(&[], true).unwrap_err();
        assert!(e.errors[0].message.contains("`ticket` has no default"));
        assert!(wf.resolve_params(&[], false).is_ok());
        let e = wf.resolve_params(&[("nope".into(), "x".into()), ("ticket".into(), "t".into())], true).unwrap_err();
        assert!(e.errors[0].message.contains("unknown param `nope`"));
        let e = wf.resolve_params(&[("retries".into(), "many".into()), ("ticket".into(), "t".into())], true).unwrap_err();
        assert!(e.errors[0].message.contains("not a valid int"));
    }

    #[test]
    fn reports_yaml_syntax_errors_with_file_lines() {
        let e = errs("---\nname: x\nparams: [unclosed\n---\nbody\n");
        assert_eq!(e.len(), 1);
        assert!(e[0].message.starts_with("invalid yaml"), "{}", e[0].message);
        assert!(e[0].line >= 3, "line {}", e[0].line);
    }

    #[test]
    fn reports_unknown_keys_with_lines() {
        let e = errs("---\nname: x\nbogus: 1\ndefaults:\n  harness: claude\n  flavour: mint\n---\n");
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].line, 3);
        assert!(e[0].message.contains("unknown frontmatter key `bogus`"));
        assert_eq!(e[1].line, 6);
        assert!(e[1].message.contains("defaults.flavour"));
    }

    #[test]
    fn reports_undefined_placeholders_with_lines() {
        let e = errs("---\nname: x\nparams:\n  a: 1\n---\nline one {{params.a}}\n\nuses {{params.b}} and {{run.name}}\n");
        assert_eq!(e.len(), 2);
        assert!(e.iter().all(|d| d.line == 8));
        assert!(e[0].message.contains("no param named `b`"));
        assert!(e[1].message.contains("run.name"));
    }

    #[test]
    fn requires_name_and_fences() {
        assert!(errs("---\ndescription: d\n---\n")[0].message.contains("missing required frontmatter key `name`"));
        assert!(errs("no frontmatter")[0].message.contains("must start with"));
        assert!(errs("---\nname: x\n")[0].message.contains("not closed"));
    }

    #[test]
    fn validates_value_shapes() {
        let e = errs("---\nname: x\nconcurrency: 0\non_conflict: drop\nparams:\n  n:\n    type: int\n    default: abc\n    colour: red\n---\n");
        let lines: Vec<usize> = e.iter().map(|d| d.line).collect();
        assert_eq!(lines, vec![3, 4, 8, 9], "{e:?}");
    }

    const TRIGGERS: &str = "---
name: t
params:
  base: {default: main}
  spec: {}
triggers:
  - manual
  - file: \"specs/**/*.md\"
    on: [created]
    debounce: 5s
    ignore: [\"specs/drafts/**\"]
    params: {spec: x}
  - cron: \"0 9 * * 1-5\"
    to: running-or-new
    params: {base: dev, spec: y}
---
{{trigger.kind}} {{trigger.paths}} {{trigger.event}} {{trigger.time}} {{trigger.scheduled}} {{params.spec}}
";

    #[test]
    fn parses_typed_triggers() {
        let wf = p(TRIGGERS).unwrap();
        let t = &wf.frontmatter.triggers;
        assert_eq!(t.len(), 3);
        assert!(matches!(t[0].kind, TriggerKind::Manual));
        let TriggerKind::File(f) = &t[1].kind else { panic!("{:?}", t[1]) };
        assert_eq!((f.file.as_str(), f.on.as_slice(), f.debounce), ("specs/**/*.md", &[FileEvent::Created][..], Duration::from_secs(5)));
        assert!(f.glob.matches("specs/a/b.md") && f.ignore_globs[0].matches("specs/drafts/x.md"));
        assert_eq!((t[1].to, t[1].line), (Target::New, 8));
        assert_eq!(f.while_running, WhileRunning::Queue, "the default");
        let parallel = p("---\nname: t\ntriggers:\n  - file: \"*.md\"\n    while_running: parallel\n---\n").unwrap();
        let TriggerKind::File(f) = &parallel.frontmatter.triggers[0].kind else { panic!() };
        assert_eq!(f.while_running, WhileRunning::Parallel);
        let running = p("---\nname: t\ntriggers:\n  - file: \"*.md\"\n    to: running\n---\n").unwrap();
        assert_eq!(running.frontmatter.triggers[0].to, Target::Running, "file triggers take any `to:`");
        assert!(matches!(t[2].kind, TriggerKind::Cron { .. }));
        assert_eq!((t[2].to, t[2].line, t[2].params["base"].clone()), (Target::RunningOrNew, 13, json!("dev")));
        let v = serde_json::to_value(&t[2]).unwrap();
        assert_eq!((v["kind"].as_str(), v["cron"].as_str(), v["to"].as_str()), (Some("cron"), Some("0 9 * * 1-5"), Some("running-or-new")));
    }

    #[test]
    fn template_snapshots_render_like_the_workflow() {
        let wf = p(TRIGGERS).unwrap();
        let params = wf.resolve_params(&[("spec".into(), "s".into())], true).unwrap();
        let ev = json!({"kind": "file", "paths": [{"path": "specs/a.md", "event": "created"}]});
        let ev = ev.as_object().unwrap();
        let template = wf.template_snapshot();
        assert!(template.contains("{{trigger.paths}}"));
        let back = parse(Path::new("t.md"), &template).unwrap();
        assert_eq!(back.render_snapshot(&params, "7", ev), wf.render_snapshot(&params, "7", ev));
    }

    #[test]
    fn trigger_placeholders_render_empty_for_manual_runs() {
        let wf = p(TRIGGERS).unwrap();
        let params = wf.resolve_params(&[("spec".into(), "s".into())], true).unwrap();
        assert_eq!(wf.render_body(&params, "1", &Map::new()), "     s");
        let ev = json!({"kind": "file", "event": "created", "time": "now",
            "paths": [{"path": "specs/a.md", "event": "created"}, {"path": "specs/b.md", "event": "modified"}]});
        let body = wf.render_body(&params, "1", ev.as_object().unwrap());
        assert_eq!(body, "file specs/a.md (created), specs/b.md (modified) created now  s");
    }

    #[test]
    fn trigger_errors() {
        let wf = |t: &str| format!("---\nname: x\nparams:\n  need: {{}}\n  n: {{type: int, default: 1}}\ntriggers:\n{t}---\n");
        let cases = [
            ("  - cron: \"61 * * * *\"\n    params: {need: a}\n", "minute `61` is out of range"),
            ("  - file: \"a[b\"\n    params: {need: a}\n", "unclosed `[`"),
            ("  - file: \"*.md\"\n    while_running: later\n    params: {need: a}\n", "`while_running` must be"),
            ("  - cron: \"* * * * *\"\n    while_running: mute\n    params: {need: a}\n", "`while_running` only applies to file triggers"),
            ("  - file: \"*.md\"\n    to: running-or-new\n    while_running: mute\n    params: {need: a}\n", "`while_running` only applies to `to: new`"),
            ("  - cron: \"* * * * *\"\n", "leaves required param `need` unfilled"),
            ("  - cron: \"* * * * *\"\n    params: {need: a, n: lots}\n", "`lots` is not a valid int"),
            ("  - cron: \"* * * * *\"\n    params: {need: a, other: 1}\n", "unknown param `other`"),
            ("  - webhook\n", "unknown trigger `webhook`"),
            ("  - cron: \"* * * * *\"\n    on: [created]\n    params: {need: a}\n", "only applies to file triggers"),
            ("  - file: \"*.md\"\n    on: [deleted]\n    params: {need: a}\n", "`on` must list"),
            ("  - file: \"*.md\"\n    debounce: soon\n    params: {need: a}\n", "`debounce`"),
            ("  - file: \"*.md\"\n    to: sideways\n    params: {need: a}\n", "trigger `to` must be"),
        ];
        for (triggers, expect) in cases {
            let e = errs(&wf(triggers));
            assert!(e.iter().any(|d| d.message.contains(expect) && d.line == 7), "{triggers}: {e:?}");
        }
        // `manual` needs nothing filled; a valid trigger with its params passes.
        assert!(p(&wf("  - manual\n  - cron: \"* * * * *\"\n    params: {need: a, n: 2}\n")).is_ok());
        assert!(errs("---\nname: x\n---\n{{trigger.who}}\n")[0].message.contains("trigger.kind|paths"));
    }

    #[test]
    fn global_workflows_need_absolute_file_globs() {
        let src = |g: &str| format!("---\nname: x\ntriggers:\n  - cron: \"0 * * * *\"\n  - file: \"{g}\"\n---\n");
        let e = check_scope(p(&src("specs/*.md")).unwrap(), Scope::Global).unwrap_err();
        assert_eq!(e.errors[0].line, 5);
        assert!(e.errors[0].message.contains("absolute or `~/`"));
        assert!(check_scope(p(&src("specs/*.md")).unwrap(), Scope::Project).is_ok());
        assert!(check_scope(p(&src("~/notes/*.md")).unwrap(), Scope::Global).is_ok());
        assert!(check_scope(p(&src("/srv/in/**")).unwrap(), Scope::Global).is_ok());
    }

    #[test]
    fn topic_triggers() {
        let wf = p("---\nname: review\nparams:\n  base: {}\ntriggers:\n  - on: review.requested\n    params: {base: main}\n  - on: \"feature.**\"\n    to: running-or-new\n    params: {base: dev}\n---\n{{trigger.topic}}|{{trigger.payload}}|{{trigger.event_id}}|{{trigger.sender}}\n").unwrap();
        let t = &wf.frontmatter.triggers;
        let TriggerKind::Topic { on } = &t[0].kind else { panic!("{:?}", t[0]) };
        assert!(on.matches("review.requested"));
        assert_eq!((t[0].to, t[0].describe(), t[0].kind_name()), (Target::New, "on review.requested".to_string(), "topic"));
        assert_eq!(t[1].to, Target::RunningOrNew);
        let v = serde_json::to_value(&t[1]).unwrap();
        assert_eq!((v["kind"].as_str(), v["on"].as_str()), (Some("topic"), Some("feature.**")));
        let params = wf.resolve_params(&[("base".into(), "x".into())], true).unwrap();
        assert_eq!(wf.render_body(&params, "1", &Map::new()), "|||", "empty on manual runs");
        let ev = json!({"topic": "review.requested", "payload": "p", "event_id": 4, "sender": "user"});
        assert_eq!(wf.render_body(&params, "1", ev.as_object().unwrap()), "review.requested|p|4|user");
        assert!(wf.warnings().is_empty());

        let e = |t: &str| errs(&format!("---\nname: x\ntriggers:\n{t}---\n"));
        for (t, expect) in [
            ("  - on: Review\n", "invalid topic pattern `Review`"),
            ("  - on: \"a.**.b\"\n", "`**` may only be the last segment"),
            ("  - on: [a, b]\n", "`on` must be a topic pattern"),
            ("  - on: a.b\n    debounce: 1s\n", "`debounce` only applies to file triggers"),
            ("  - on: a.b\n    to: later\n", "trigger `to` must be"),
            ("  - to: new\n", "needs `file:`, `cron:` or `on:`"),
        ] {
            assert!(e(t).iter().any(|d| d.message.contains(expect) && d.line == 4), "{t}: {:?}", e(t));
        }
        // A file trigger's `on:` is still its events.
        assert!(p("---\nname: x\ntriggers:\n  - file: \"*.md\"\n    on: created\n---\n").is_ok());

        let global = p("---\nname: x\ntriggers:\n  - on: a.b\n---\n").unwrap();
        let e = check_scope(global, Scope::Global).unwrap_err();
        assert!(e.errors[0].message.contains("global workflow has no project bus"), "{e:?}");
    }

    #[test]
    fn topic_trigger_warnings() {
        let wf = p("---\nname: impl\ntriggers:\n  - on: a.*\n  - on: a.b\n  - on: \"tome.run.*.succeeded\"\n  - on: tome.run.other.failed\n---\n").unwrap();
        let w = wf.warnings();
        assert_eq!(w.len(), 2, "{w:?}");
        assert_eq!(w[0].line, 5);
        assert!(w[0].message.contains("`on: a.b` and `on: a.*` can match the same topic"));
        assert_eq!(w[1].line, 6);
        assert!(w[1].message.contains("own `tome.run.impl.*` events"));
    }

    #[test]
    fn locator_handles_nested_keys() {
        let lines = ["name: x", "params:", "  base:", "    default: main", "defaults:", "  harness: c"];
        let loc = Locator { lines: &lines };
        assert_eq!(loc.line(&["params", "base", "default"]), 5);
        assert_eq!(loc.line(&["defaults", "harness"]), 7);
        assert_eq!(loc.line(&["defaults", "missing"]), 6);
    }
}
