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

pub const TOP_LEVEL_KEYS: &[&str] =
    &["name", "description", "triggers", "params", "defaults", "concurrency", "on_conflict"];
pub const DEFAULTS_KEYS: &[&str] = &["backend", "harness", "orchestrator_harness", "timeout", "on_failure"];
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orchestrator_harness: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_failure: Option<String>,
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
    pub triggers: Vec<Value>,
    pub params: BTreeMap<String, ParamSpec>,
    pub defaults: Defaults,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_conflict: Option<OnConflict>,
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
            "triggers" => fm.triggers = parse_triggers(value, line, &mut errors),
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
            other => errors.push(Diagnostic::new(
                line,
                format!("unknown frontmatter key `{other}` (expected one of: {})", TOP_LEVEL_KEYS.join(", ")),
            )),
        }
    }
    if !map.contains_key("name") {
        errors.push(Diagnostic::new(1, "missing required frontmatter key `name`"));
    }

    errors.extend(check_placeholders(&body, body_line, &fm.params));

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

fn parse_triggers(value: &Yaml, line: usize, errors: &mut Vec<Diagnostic>) -> Vec<Value> {
    let items: Vec<&Yaml> = match value {
        Yaml::Null => return Vec::new(),
        Yaml::String(_) | Yaml::Mapping(_) => vec![value],
        Yaml::Sequence(seq) => seq.iter().collect(),
        _ => {
            errors.push(Diagnostic::new(line, "`triggers` must be a list"));
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for item in items {
        match item {
            Yaml::String(_) | Yaml::Mapping(_) => out.push(yaml_to_json(item)),
            _ => errors.push(Diagnostic::new(line, "each trigger must be a name or a mapping")),
        }
    }
    out
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
            other => errors.push(Diagnostic::new(
                line,
                format!("unknown key `defaults.{other}` (expected one of: {})", DEFAULTS_KEYS.join(", ")),
            )),
        }
    }
    d
}

fn is_identifier(s: &str) -> bool {
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
                _ => false,
            };
            if !ok {
                let msg = match ph.expr.split_once('.') {
                    Some(("params", name)) => format!("undefined placeholder `{{{{{}}}}}`: no param named `{name}`", ph.expr),
                    _ => format!(
                        "undefined placeholder `{{{{{}}}}}` (available: `params.<name>`, `run.id`)",
                        ph.expr
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

    /// Substitute `{{params.x}}` and `{{run.id}}` into the body. Plain text
    /// substitution only.
    pub fn render_body(&self, params: &Map<String, Value>, run_id: &str) -> String {
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

    /// The resolved workflow as saved with a run: the original frontmatter
    /// followed by the substituted body.
    pub fn render_snapshot(&self, params: &Map<String, Value>, run_id: &str) -> String {
        format!("---\n{}\n---\n{}", self.frontmatter_text, self.render_body(params, run_id))
    }
}

fn value_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
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
                let result = load(&path);
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
                return Ok(load(as_path));
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

fn same_path(a: &Path, b: &Path) -> bool {
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
    fn resolves_and_renders() {
        let wf = p(GOOD).unwrap();
        let params = wf
            .resolve_params(&[("base".into(), "dev".into()), ("ticket".into(), "T-1".into())], true)
            .unwrap();
        assert_eq!(params["retries"], json!(3));
        let body = wf.render_body(&params, "42");
        assert!(body.contains("off dev for run 42."));
        assert!(body.contains("up to 3 times on T-1."));
        let snap = wf.render_snapshot(&params, "42");
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

    #[test]
    fn locator_handles_nested_keys() {
        let lines = ["name: x", "params:", "  base:", "    default: main", "defaults:", "  harness: c"];
        let loc = Locator { lines: &lines };
        assert_eq!(loc.line(&["params", "base", "default"]), 5);
        assert_eq!(loc.line(&["defaults", "harness"]), 7);
        assert_eq!(loc.line(&["defaults", "missing"]), 6);
    }
}
