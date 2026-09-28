//! Where a session goes: its placement settings, the presets they can be
//! built on, and how each setting resolves from the levels that can set it.
//!
//! The settings are `layout` (`tab`, `split`; `workspace` is kept as an
//! alias for `workspace: own`), `workspace` (`project`, `focused`, a name,
//! `own`), `split.direction`, `split.size`, `from` and `preset`.
//!
//! Each setting resolves on its own, from the highest level down:
//!
//! | for workers                  | for the orchestrator          |
//! |------------------------------|-------------------------------|
//! | `tome worker spawn` flags    | `tome run` flags              |
//! | the first matching rule      | the `orchestrator` role block |
//! | the `workers` role block     |                               |
//! | `tome run` flags             |                               |
//!
//! then, for both: `defaults.layout`, its preset, `TOME_LAYOUT`, the
//! project's `.tome/config.yaml`, the global `~/.tome/config.yaml` and
//! the built-in default (`tab` in the project workspace). A level's own
//! settings come before the preset it's built on.

use crate::config::{self, Config};
use crate::glob::Glob;
use crate::output::{table, CliError, CliResult, Report};
use crate::session::Layout;
use serde::{Serialize, Serializer};
use serde_json::{json, Value};
use serde_yaml::Value as Yaml;
use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

/// The words `workspace` reads as keywords rather than names.
pub const RESERVED: &[&str] = &["project", "focused", "own"];
pub const LAYOUTS: &[&str] = &["tab", "split", "workspace"];
pub const DIRECTIONS: &[&str] = &["right", "down", "left", "up"];
pub const FROMS: &[&str] = &["orchestrator", "last", "first"];
/// The keys a placement block takes (a workflow's also takes role blocks).
pub const KEYS: &[&str] = &["preset", "layout", "workspace", "split", "from"];
pub const SPLIT_KEYS: &[&str] = &["direction", "size"];

/// Presets every install has; they match 008's layouts.
pub const BUILT_IN: &[&str] = &["tab", "split", "workspace"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Workspace {
    /// The project's `<project>-orchestrator` workspace.
    Project,
    /// Whatever was focused when the run started.
    Focused,
    /// `<project>-<name>`.
    Named(String),
    /// One per session.
    Own,
}

impl Workspace {
    /// A `workspace:` value: a reserved word is the keyword, anything else
    /// a name.
    pub fn parse(s: &str) -> Result<Workspace, String> {
        match s {
            "project" => Ok(Workspace::Project),
            "focused" => Ok(Workspace::Focused),
            "own" => Ok(Workspace::Own),
            name => Workspace::named(name),
        }
    }

    /// `{ name: <name> }`: always a name, so the reserved words are refused.
    pub fn named(name: &str) -> Result<Workspace, String> {
        if RESERVED.contains(&name) {
            return Err(format!("`{name}` is reserved and can't be a workspace name (use `workspace: {name}` for the keyword)"));
        }
        if !crate::workflow::is_identifier(name) {
            return Err(format!(
                "unknown workspace `{name}` (expected one of: {}, or a name of letters, digits, `-` and `_`)",
                RESERVED.join(", ")
            ));
        }
        Ok(Workspace::Named(name.to_string()))
    }
}

impl fmt::Display for Workspace {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Workspace::Project => f.write_str("project"),
            Workspace::Focused => f.write_str("focused"),
            Workspace::Named(n) => f.write_str(n),
            Workspace::Own => f.write_str("own"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Right,
    Down,
    Left,
    Up,
}

impl Direction {
    pub fn parse(s: &str) -> Option<Direction> {
        match s {
            "right" => Some(Direction::Right),
            "down" => Some(Direction::Down),
            "left" => Some(Direction::Left),
            "up" => Some(Direction::Up),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Direction::Right => "right",
            Direction::Down => "down",
            Direction::Left => "left",
            Direction::Up => "up",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    Percent(u32),
    Cells(u32),
}

impl Size {
    /// `30%` or `80` (cells).
    pub fn parse(s: &str) -> Result<Size, String> {
        let bad = || format!("invalid `split.size` `{s}` (expected a percent like `30%` or a number of cells like `80`)");
        let s = s.trim();
        let (digits, percent) = match s.strip_suffix('%') {
            Some(d) => (d.trim(), true),
            None => (s, false),
        };
        let n: u32 = digits.parse().map_err(|_| bad())?;
        if n == 0 {
            return Err(bad());
        }
        Ok(if percent { Size::Percent(n) } else { Size::Cells(n) })
    }
}

impl fmt::Display for Size {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Size::Percent(n) => write!(f, "{n}%"),
            Size::Cells(n) => write!(f, "{n}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum From {
    Orchestrator,
    Last,
    First,
}

impl From {
    pub fn parse(s: &str) -> Option<From> {
        match s {
            "orchestrator" => Some(From::Orchestrator),
            "last" => Some(From::Last),
            "first" => Some(From::First),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            From::Orchestrator => "orchestrator",
            From::Last => "last",
            From::First => "first",
        }
    }
}

macro_rules! serialize_as_string {
    ($($t:ty),*) => {$(
        impl Serialize for $t {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(&self.to_string())
            }
        }
    )*};
}
impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
impl fmt::Display for From {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
serialize_as_string!(Workspace, Direction, Size, From);

/// One level's placement settings; unset ones are left to the levels below.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Settings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// `tab`, `split`, or `workspace` (the alias for `workspace: own`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout: Option<Layout>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<Workspace>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direction: Option<Direction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<Size>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<From>,
}

impl Serialize for Layout {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl Settings {
    pub fn is_empty(&self) -> bool {
        *self == Settings::default()
    }

    /// Just a layout, as the old single `layout` string gave.
    pub fn layout(layout: Layout) -> Settings {
        Settings { layout: Some(layout), ..Settings::default() }
    }

    /// The settings as `key=value` pairs, for listings.
    pub fn describe(&self) -> String {
        let mut out = Vec::new();
        if let Some(p) = &self.preset {
            out.push(format!("preset={p}"));
        }
        if let Some(l) = self.layout {
            out.push(format!("layout={}", l.as_str()));
        }
        if let Some(w) = &self.workspace {
            out.push(format!("workspace={w}"));
        }
        if let Some(d) = self.direction {
            out.push(format!("split.direction={d}"));
        }
        if let Some(s) = self.size {
            out.push(format!("split.size={s}"));
        }
        if let Some(f) = self.from {
            out.push(format!("from={f}"));
        }
        out.join(" ")
    }
}

pub fn parse_layout(s: &str) -> Result<Layout, String> {
    Layout::parse(s).ok_or_else(|| format!("unknown `layout` `{s}` (expected one of: {})", LAYOUTS.join(", ")))
}

pub fn parse_direction(s: &str) -> Result<Direction, String> {
    Direction::parse(s)
        .ok_or_else(|| format!("unknown `split.direction` `{s}` (expected one of: {})", DIRECTIONS.join(", ")))
}

pub fn parse_from(s: &str) -> Result<From, String> {
    From::parse(s).ok_or_else(|| format!("unknown `from` `{s}` (expected one of: {})", FROMS.join(", ")))
}

/// A YAML scalar as a string (`size: 80` is a number).
fn scalar(v: &Yaml) -> Option<String> {
    match v {
        Yaml::String(s) => Some(s.clone()),
        Yaml::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// An error found while reading a placement block, with the key path it's
/// at (for line numbers).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub path: Vec<String>,
    pub message: String,
}

impl Problem {
    fn new(path: &[&str], message: impl Into<String>) -> Problem {
        Problem { path: path.iter().map(|s| s.to_string()).collect(), message: message.into() }
    }
}

/// Read one placement block's settings from `map`, skipping the keys in
/// `other` (a workflow's role blocks). `at` is the block's key path, for
/// problems; `what` names it in messages. With `allow_preset` false a
/// `preset:` is refused (presets can't nest).
pub fn parse_settings(
    map: &serde_yaml::Mapping,
    at: &[&str],
    what: &str,
    other: &[&str],
    allow_preset: bool,
    problems: &mut Vec<Problem>,
) -> Settings {
    let mut s = Settings::default();
    for (k, v) in map {
        let key = k.as_str().unwrap_or("");
        let mut path = at.to_vec();
        path.push(key);
        let mut problem = |m: String| problems.push(Problem::new(&path, m));
        match key {
            "preset" if !allow_preset => problem(format!("{what}: a preset can't reference another preset")),
            "preset" => match v.as_str().filter(|p| !p.trim().is_empty()) {
                Some(p) => s.preset = Some(p.to_string()),
                None => problem(format!("{what}: `preset` must be a preset name")),
            },
            "layout" => match v.as_str().map(parse_layout) {
                Some(Ok(l)) => s.layout = Some(l),
                Some(Err(e)) => problem(format!("{what}: {e}")),
                None => problem(format!("{what}: `layout` must be a string")),
            },
            "workspace" => {
                let parsed = match v {
                    Yaml::String(w) => Workspace::parse(w),
                    Yaml::Mapping(m) => match (m.len(), m.get("name").and_then(Yaml::as_str)) {
                        (1, Some(name)) => Workspace::named(name),
                        _ => Err("`workspace` must be a keyword, a name, or `{ name: <name> }`".to_string()),
                    },
                    _ => Err("`workspace` must be a keyword, a name, or `{ name: <name> }`".to_string()),
                };
                match parsed {
                    Ok(w) => s.workspace = Some(w),
                    Err(e) => problem(format!("{what}: {e}")),
                }
            }
            "split" => {
                let Yaml::Mapping(split) = v else {
                    problem(format!("{what}: `split` must be a mapping with `direction` and/or `size`"));
                    continue;
                };
                for (sk, sv) in split {
                    let skey = sk.as_str().unwrap_or("");
                    let mut spath = path.clone();
                    spath.push(skey);
                    let mut problem = |m: String| problems.push(Problem::new(&spath, m));
                    match skey {
                        "direction" => match sv.as_str().map(parse_direction) {
                            Some(Ok(d)) => s.direction = Some(d),
                            Some(Err(e)) => problem(format!("{what}: {e}")),
                            None => problem(format!("{what}: `split.direction` must be a string")),
                        },
                        "size" => match scalar(sv).map(|x| Size::parse(&x)) {
                            Some(Ok(size)) => s.size = Some(size),
                            Some(Err(e)) => problem(format!("{what}: {e}")),
                            None => problem(format!("{what}: `split.size` must be a percent or a number of cells")),
                        },
                        other => problem(format!(
                            "{what}: unknown key `split.{other}` (expected one of: {})",
                            SPLIT_KEYS.join(", ")
                        )),
                    }
                }
            }
            "from" => match v.as_str().map(parse_from) {
                Some(Ok(f)) => s.from = Some(f),
                Some(Err(e)) => problem(format!("{what}: {e}")),
                None => problem(format!("{what}: `from` must be a string")),
            },
            k if other.contains(&k) => {}
            other_key => {
                let mut known: Vec<&str> = KEYS.iter().filter(|k| allow_preset || **k != "preset").copied().collect();
                known.extend_from_slice(other);
                problem(format!("{what}: unknown key `{other_key}` (expected one of: {})", known.join(", ")))
            }
        }
    }
    s
}

/// A `layout:` value that's either the old single layout name or a block of
/// settings.
pub fn parse_value<'a>(
    v: &'a Yaml,
    at: &[&str],
    what: &str,
    other: &[&str],
    problems: &mut Vec<Problem>,
) -> Option<(Settings, Option<&'a serde_yaml::Mapping>)> {
    match v {
        Yaml::Null => None,
        Yaml::String(s) => match Layout::parse(s) {
            Some(l) => Some((Settings::layout(l), None)),
            None => {
                problems.push(Problem::new(at, format!("unknown {what} `{s}` (expected one of: {})", LAYOUTS.join(", "))));
                None
            }
        },
        Yaml::Mapping(m) => Some((parse_settings(m, at, what, other, true, problems), Some(m))),
        _ => {
            problems.push(Problem::new(at, format!("{what} must be a layout name or a mapping of placement settings")));
            None
        }
    }
}

/// A `workers` rule: settings for the workers whose spawn name matches.
#[derive(Debug, Clone, Serialize)]
pub struct Rule {
    #[serde(rename = "match")]
    pub pattern: String,
    #[serde(skip)]
    pub glob: Option<Glob>,
    #[serde(flatten)]
    pub settings: Settings,
}

impl Rule {
    pub fn matches(&self, name: &str) -> bool {
        self.glob.as_ref().is_some_and(|g| g.matches(name))
    }
}

/// A workflow's `defaults.layout`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Spec {
    /// The settings written directly under it (with its `preset`).
    #[serde(flatten)]
    pub base: Settings,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orchestrator: Option<Settings>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workers: Option<Settings>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<Rule>,
}

impl Spec {
    /// Every preset it names.
    pub fn presets(&self) -> Vec<&str> {
        let mut out: Vec<&str> = [Some(&self.base), self.orchestrator.as_ref(), self.workers.as_ref()]
            .into_iter()
            .flatten()
            .chain(self.rules.iter().map(|r| &r.settings))
            .filter_map(|s| s.preset.as_deref())
            .collect();
        out.dedup();
        out
    }
}

/// Parse a workflow's `defaults.layout`: a layout name, or a block with
/// settings, an `orchestrator` role block and `workers` (a list of rules, or
/// a role block with the rules under `rules`).
pub fn parse_spec(v: &Yaml, problems: &mut Vec<Problem>) -> Option<Spec> {
    const AT: &[&str] = &["defaults", "layout"];
    let (base, map) = parse_value(v, AT, "`defaults.layout`", &["orchestrator", "workers"], problems)?;
    let mut spec = Spec { base, ..Spec::default() };
    let Some(map) = map else { return Some(spec) };
    if let Some(o) = map.get("orchestrator") {
        let at = ["defaults", "layout", "orchestrator"];
        match o {
            Yaml::Mapping(m) => spec.orchestrator = Some(parse_settings(m, &at, "`defaults.layout.orchestrator`", &[], true, problems)),
            Yaml::Null => {}
            _ => problems.push(Problem::new(&at, "`defaults.layout.orchestrator` must be a mapping of placement settings")),
        }
    }
    if let Some(w) = map.get("workers") {
        let at = ["defaults", "layout", "workers"];
        let rules: Option<&Vec<Yaml>> = match w {
            Yaml::Sequence(rules) => Some(rules),
            Yaml::Mapping(m) => {
                spec.workers = Some(parse_settings(m, &at, "`defaults.layout.workers`", &["rules"], true, problems));
                match m.get("rules") {
                    None | Some(Yaml::Null) => None,
                    Some(Yaml::Sequence(rules)) => Some(rules),
                    Some(_) => {
                        problems.push(Problem::new(&["defaults", "layout", "workers", "rules"], "`defaults.layout.workers.rules` must be a list of rules"));
                        None
                    }
                }
            }
            Yaml::Null => None,
            _ => {
                problems.push(Problem::new(&at, "`defaults.layout.workers` must be a list of rules or a mapping of placement settings"));
                None
            }
        };
        for (i, rule) in rules.into_iter().flatten().enumerate() {
            let what = format!("`defaults.layout.workers` rule {}", i + 1);
            let Yaml::Mapping(m) = rule else {
                problems.push(Problem::new(&at, format!("{what} must be a mapping with `match` and placement settings")));
                continue;
            };
            let pattern = match m.get("match").and_then(Yaml::as_str) {
                Some(p) => p.to_string(),
                None => {
                    problems.push(Problem::new(&at, format!("{what} needs a `match` glob for the worker's name")));
                    continue;
                }
            };
            let glob = match Glob::new(&pattern) {
                Ok(g) => Some(g),
                Err(e) => {
                    problems.push(Problem::new(&at, format!("{what}: {e}")));
                    None
                }
            };
            let settings = parse_settings(m, &at, &what, &["match"], true, problems);
            spec.rules.push(Rule { pattern, glob, settings });
        }
    }
    Some(spec)
}

// --- presets -------------------------------------------------------------

/// A named preset and where it's defined.
#[derive(Debug, Clone, Serialize)]
pub struct Preset {
    pub name: String,
    /// `built-in`, `global` or `project`.
    pub scope: &'static str,
    pub settings: Settings,
}

pub fn built_in(name: &str) -> Option<Settings> {
    match name {
        "tab" => Some(Settings::layout(Layout::Tab)),
        "split" => Some(Settings::layout(Layout::Split)),
        "workspace" => Some(Settings { workspace: Some(Workspace::Own), ..Settings::default() }),
        _ => None,
    }
}

fn config_hint() -> String {
    format!("see `layout:` and `layout_presets:` in the tome docs; placement values: layout {}, workspace {} or a name, split.direction {}, from {}",
        LAYOUTS.join("|"), RESERVED.join("|"), DIRECTIONS.join("|"), FROMS.join("|"))
}

fn config_problem(file: &config::File, p: &Problem) -> CliError {
    file.error(&p.message).with_hint(config_hint())
}

/// Every preset: the built-in ones, then the config's (the project's
/// replacing the global ones, and either replacing a built-in one).
pub fn presets(cfg: &Config) -> CliResult<BTreeMap<String, Preset>> {
    let mut out: BTreeMap<String, Preset> = BUILT_IN
        .iter()
        .map(|n| (n.to_string(), Preset { name: n.to_string(), scope: "built-in", settings: built_in(n).unwrap() }))
        .collect();
    for (name, (value, file)) in cfg.entries("layout_presets")? {
        let what = format!("preset `{name}`");
        let mut problems = Vec::new();
        let settings = match &value {
            Yaml::Mapping(m) => parse_settings(m, &["layout_presets", &name], &what, &[], false, &mut problems),
            _ => return Err(file.error(format!("{what} must be a mapping of placement settings")).with_hint(config_hint())),
        };
        if let Some(p) = problems.first() {
            return Err(config_problem(file, p));
        }
        out.insert(name.clone(), Preset { name, scope: file.scope.as_str(), settings });
    }
    Ok(out)
}

/// Every preset definition, built-in first, each marked with whether a
/// later one of the same name replaces it.
pub fn list(cfg: &Config) -> CliResult<Vec<(Preset, bool)>> {
    let effective = presets(cfg)?;
    let mut out: Vec<Preset> = BUILT_IN
        .iter()
        .map(|n| Preset { name: n.to_string(), scope: "built-in", settings: built_in(n).unwrap() })
        .collect();
    for file in cfg.files() {
        let Some(Yaml::Mapping(map)) = file.get("layout_presets") else { continue };
        for name in map.keys().filter_map(Yaml::as_str) {
            let mut problems = Vec::new();
            let settings = match map.get(name) {
                Some(Yaml::Mapping(m)) => parse_settings(m, &[], "", &[], false, &mut problems),
                _ => Settings::default(),
            };
            out.push(Preset { name: name.to_string(), scope: file.scope.as_str(), settings });
        }
    }
    Ok(out
        .into_iter()
        .map(|p| {
            let shadowed = effective.get(&p.name).is_some_and(|e| e.scope != p.scope);
            (p, shadowed)
        })
        .collect())
}

/// `tome layout presets`
pub fn presets_report(cwd: &Path) -> CliResult<Report> {
    let listed = list(&Config::load(Some(cwd))?)?;
    let data: Vec<Value> = listed
        .iter()
        .map(|(p, shadowed)| {
            let mut v = json!({ "name": p.name, "scope": p.scope, "settings": p.settings });
            if *shadowed {
                v["overridden"] = json!(true);
            }
            v
        })
        .collect();
    let rows = listed
        .iter()
        .map(|(p, shadowed)| {
            let scope = if *shadowed { format!("{} (overridden)", p.scope) } else { p.scope.to_string() };
            vec![p.name.clone(), scope, p.settings.describe()]
        })
        .collect();
    Ok(Report::new(json!({ "presets": data }), table(&["PRESET", "SCOPE", "SETTINGS"], rows)))
}

fn unknown_preset(name: &str, known: &BTreeMap<String, Preset>, source: &str) -> CliError {
    CliError::invalid(format!("unknown layout preset `{name}` (from {source})")).with_hint(format!(
        "known presets: {}; define others under `layout_presets:` in {} or the project's .tome/config.yaml",
        known.keys().cloned().collect::<Vec<_>>().join(", "),
        config::global_path().display()
    ))
}

// --- resolution ----------------------------------------------------------

/// Whose placement is being resolved.
#[derive(Debug, Clone, Copy)]
pub enum Role<'a> {
    Orchestrator,
    Worker(&'a str),
}

/// What a placement is resolved from, apart from the environment and config.
pub struct Inputs<'a> {
    pub role: Role<'a>,
    /// This command's flags: `tome worker spawn`'s for a worker, `tome
    /// run`'s for the orchestrator.
    pub flags: Option<&'a Settings>,
    /// `tome run`'s flags, when resolving a worker.
    pub run_flags: Option<&'a Settings>,
    /// The workflow's `defaults.layout`.
    pub spec: Option<&'a Spec>,
}

/// A resolved placement.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Placement {
    /// `tab` or `split`; `workspace` when the session gets its own.
    pub layout: Layout,
    pub workspace: Workspace,
    pub direction: Direction,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<Size>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<From>,
    /// Where each setting came from.
    pub sources: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl Placement {
    pub fn to_json(&self) -> Value {
        json!(self)
    }
}

/// One level, before its preset is expanded.
struct Level {
    source: String,
    settings: Settings,
}

/// Resolve a placement for `inputs` in the config that applies to
/// `project`. Unknown values in `TOME_LAYOUT` or the config, and presets
/// that aren't defined, are errors.
pub fn resolve(inputs: &Inputs, project: Option<&Path>) -> CliResult<Placement> {
    let env = std::env::var("TOME_LAYOUT").ok().filter(|s| !s.is_empty());
    resolve_in(inputs, &Config::load(project)?, env)
}

/// [`resolve`] with the config and `TOME_LAYOUT` given.
pub fn resolve_in(inputs: &Inputs, cfg: &Config, env_layout: Option<String>) -> CliResult<Placement> {
    let presets = presets(cfg)?;
    let mut levels = Vec::new();
    let push = |levels: &mut Vec<Level>, source: String, s: Option<&Settings>| {
        if let Some(s) = s {
            levels.push(Level { source, settings: s.clone() });
        }
    };
    let spec = inputs.spec;
    match inputs.role {
        Role::Orchestrator => {
            push(&mut levels, "`tome run` flags".into(), inputs.flags);
            push(&mut levels, "the `orchestrator` block".into(), spec.and_then(|s| s.orchestrator.as_ref()));
        }
        Role::Worker(name) => {
            push(&mut levels, "`tome worker spawn` flags".into(), inputs.flags);
            if let Some(rule) = spec.and_then(|s| s.rules.iter().find(|r| r.matches(name))) {
                push(&mut levels, format!("the `workers` rule `{}`", rule.pattern), Some(&rule.settings));
            }
            push(&mut levels, "the `workers` block".into(), spec.and_then(|s| s.workers.as_ref()));
            push(&mut levels, "`tome run` flags".into(), inputs.run_flags);
        }
    }
    push(&mut levels, "`defaults.layout`".into(), spec.map(|s| &s.base));
    if let Some(env) = env_layout {
        let layout = Layout::parse(&env).ok_or_else(|| {
            CliError::invalid(format!("unknown session layout `{env}` (from TOME_LAYOUT)")).with_hint(format!("use {}", LAYOUTS.join(", ")))
        })?;
        push(&mut levels, "TOME_LAYOUT".into(), Some(&Settings::layout(layout)));
    }
    if let Some((value, file)) = cfg.get("layout") {
        let mut problems = Vec::new();
        let source = format!("{} config", file.scope.as_str());
        match parse_value(value, &["layout"], "`layout`", &[], &mut problems) {
            _ if !problems.is_empty() => {
                // Keep 008's message for an unknown layout name.
                if let Yaml::String(name) = value {
                    return Err(CliError::invalid(format!("unknown session layout `{name}` (from `layout` in {})", file.label()))
                        .with_hint(format!("use {}", LAYOUTS.join(", "))));
                }
                return Err(config_problem(file, &problems[0]));
            }
            Some((s, _)) => push(&mut levels, source, Some(&s)),
            None => {}
        }
    }

    // Expand presets: a level's own settings, then its preset's.
    let mut expanded: Vec<Level> = Vec::new();
    for level in levels {
        let preset = level.settings.preset.clone();
        let source = level.source.clone();
        expanded.push(level);
        if let Some(name) = preset {
            let p = presets.get(&name).ok_or_else(|| unknown_preset(&name, &presets, &source))?;
            expanded.push(Level { source: format!("preset `{name}` (from {source})"), settings: p.settings.clone() });
        }
    }

    let mut sources = BTreeMap::new();
    macro_rules! pick {
        ($field:ident, $key:expr) => {{
            let found = expanded.iter().enumerate().find_map(|(i, l)| l.settings.$field.clone().map(|v| (i, v)));
            if let Some((i, _)) = &found {
                sources.insert($key.to_string(), expanded[*i].source.clone());
            }
            found
        }};
    }
    let layout = pick!(layout, "layout");
    let workspace = pick!(workspace, "workspace");
    let direction = pick!(direction, "split.direction");
    let size = pick!(size, "split.size");
    let from = pick!(from, "from");

    // `layout: workspace` is `workspace: own`, unless a higher level chose
    // the workspace itself.
    let (layout, workspace) = match (layout, workspace) {
        (Some((li, Layout::Workspace)), Some((wi, w))) if wi < li => {
            sources.insert("layout".into(), "the default".into());
            (Layout::Tab, w)
        }
        (Some((li, Layout::Workspace)), _) => {
            let source = expanded[li].source.clone();
            sources.insert("workspace".into(), source);
            (Layout::Workspace, Workspace::Own)
        }
        (layout, workspace) => {
            let layout = layout.map(|(_, l)| l).unwrap_or(Layout::Tab);
            let workspace = workspace.map(|(_, w)| w).unwrap_or(Workspace::Project);
            (if workspace == Workspace::Own { Layout::Workspace } else { layout }, workspace)
        }
    };
    for key in ["layout", "workspace", "split.direction"] {
        sources.entry(key.to_string()).or_insert_with(|| "the default".into());
    }
    Ok(Placement {
        layout,
        workspace,
        direction: direction.map(|(_, d)| d).unwrap_or(Direction::Right),
        size: size.map(|(_, s)| s),
        from: from.map(|(_, f)| f),
        sources,
        warnings: Vec::new(),
    })
}

/// Check every preset a workflow names against `presets`, for commands that
/// fail on an undefined one.
pub fn check_presets(spec: Option<&Spec>, project: Option<&Path>) -> CliResult<()> {
    let Some(spec) = spec else { return Ok(()) };
    let names = spec.presets();
    if names.is_empty() {
        return Ok(());
    }
    let known = presets(&Config::load(project)?)?;
    match names.into_iter().find(|n| !known.contains_key(*n)) {
        Some(n) => Err(unknown_preset(n, &known, "the workflow's `defaults.layout`")),
        None => Ok(()),
    }
}

/// The presets a workflow names that aren't defined, for `tome validate`
/// (a warning: presets depend on the environment).
pub fn undefined_presets(spec: Option<&Spec>, project: Option<&Path>) -> Vec<String> {
    let Some(spec) = spec else { return Vec::new() };
    let Ok(known) = Config::load(project).and_then(|c| presets(&c)) else { return Vec::new() };
    spec.presets().into_iter().filter(|n| !known.contains_key(*n)).map(str::to_string).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(yaml: &str) -> (Option<Spec>, Vec<Problem>) {
        let mut problems = Vec::new();
        let v: Yaml = serde_yaml::from_str(yaml).unwrap();
        (parse_spec(&v, &mut problems), problems)
    }

    #[test]
    fn parses_the_block_form() {
        let (s, problems) = spec(
            "preset: wide\nsplit: { size: 30% }\norchestrator: { layout: split, split: { direction: right, size: 40% } }\nworkers:\n  - match: \"review-*\"\n    preset: quiet\n    from: orchestrator\n  - match: \"*\"\n    layout: split\n    split: { direction: down }\n",
        );
        assert!(problems.is_empty(), "{problems:?}");
        let s = s.unwrap();
        assert_eq!(s.base.preset.as_deref(), Some("wide"));
        assert_eq!(s.base.size, Some(Size::Percent(30)));
        let o = s.orchestrator.as_ref().unwrap();
        assert_eq!((o.layout, o.direction, o.size), (Some(Layout::Split), Some(Direction::Right), Some(Size::Percent(40))));
        assert_eq!(s.rules.len(), 2);
        assert!(s.rules[0].matches("review-1") && !s.rules[0].matches("build"));
        assert_eq!(s.rules[1].settings.direction, Some(Direction::Down));
        assert_eq!(s.presets(), ["wide", "quiet"]);
    }

    #[test]
    fn workers_can_be_a_role_block_with_rules() {
        let (s, problems) = spec("workers:\n  layout: split\n  rules:\n    - match: a*\n      from: first\n");
        assert!(problems.is_empty(), "{problems:?}");
        let s = s.unwrap();
        assert_eq!(s.workers.unwrap().layout, Some(Layout::Split));
        assert_eq!(s.rules[0].settings.from, Some(From::First));
    }

    #[test]
    fn bad_values_are_explained() {
        for (yaml, want) in [
            ("tabs", "unknown `defaults.layout` `tabs`"),
            ("layout: sideways", "unknown `layout` `sideways`"),
            ("split: { direction: diagonal }", "unknown `split.direction` `diagonal`"),
            ("split: { size: 0 }", "invalid `split.size` `0`"),
            ("split: { size: lots }", "invalid `split.size`"),
            ("split: { dir: up }", "unknown key `split.dir`"),
            ("from: middle", "unknown `from` `middle`"),
            ("workspace: { name: own }", "`own` is reserved"),
            ("workspace: \"a b\"", "unknown workspace `a b`"),
            ("orchestrator: { preset: 3 }", "`preset` must be a preset name"),
            ("workers: [{ layout: tab }]", "needs a `match` glob"),
            ("colour: red", "unknown key `colour`"),
        ] {
            let (_, problems) = spec(yaml);
            assert!(problems.iter().any(|p| p.message.contains(want)), "{yaml}: {problems:?}");
        }
    }

    #[test]
    fn workspace_words_are_keywords_and_other_strings_names() {
        assert_eq!(Workspace::parse("own"), Ok(Workspace::Own));
        assert_eq!(Workspace::parse("focused"), Ok(Workspace::Focused));
        assert_eq!(Workspace::parse("reviews"), Ok(Workspace::Named("reviews".into())));
        assert_eq!(Size::parse("80"), Ok(Size::Cells(80)));
        assert_eq!(Size::parse("30%"), Ok(Size::Percent(30)));
    }

    fn resolve_with(spec_yaml: &str, role: Role, flags: Option<&Settings>, run_flags: Option<&Settings>) -> Placement {
        let (s, problems) = spec(spec_yaml);
        assert!(problems.is_empty(), "{problems:?}");
        resolve_in(&Inputs { role, flags, run_flags, spec: s.as_ref() }, &Config::default(), None).unwrap()
    }

    #[test]
    fn each_setting_resolves_on_its_own() {
        let yaml = "preset: split\nsplit: { size: 30% }\norchestrator: { split: { direction: down } }\nworkers:\n  layout: tab\n  rules:\n    - match: \"review-*\"\n      from: orchestrator\n";
        let o = resolve_with(yaml, Role::Orchestrator, None, None);
        assert_eq!((o.layout, o.direction, o.size, o.from), (Layout::Split, Direction::Down, Some(Size::Percent(30)), None));
        assert_eq!(o.sources["layout"], "preset `split` (from `defaults.layout`)");
        assert_eq!(o.sources["split.direction"], "the `orchestrator` block");
        assert_eq!(o.sources["split.size"], "`defaults.layout`");
        assert_eq!(o.sources["workspace"], "the default");

        let flags = Settings { size: Some(Size::Cells(80)), ..Settings::default() };
        let w = resolve_with(yaml, Role::Worker("review-1"), Some(&flags), None);
        assert_eq!((w.layout, w.size, w.from), (Layout::Tab, Some(Size::Cells(80)), Some(From::Orchestrator)));
        assert_eq!(w.sources["from"], "the `workers` rule `review-*`");
        assert_eq!(w.sources["layout"], "the `workers` block");
        assert_eq!(w.sources["split.size"], "`tome worker spawn` flags");

        // Run flags sit below the worker blocks, above `defaults.layout`.
        let run = Settings { layout: Some(Layout::Split), direction: Some(Direction::Up), ..Settings::default() };
        let w = resolve_with(yaml, Role::Worker("build"), None, Some(&run));
        assert_eq!((w.layout, w.direction), (Layout::Tab, Direction::Up));
    }

    #[test]
    fn layout_workspace_is_an_own_workspace() {
        let o = resolve_with("workspace", Role::Orchestrator, None, None);
        assert_eq!((o.layout, &o.workspace), (Layout::Workspace, &Workspace::Own));
        // A higher level's own layout wins over it.
        let flags = Settings::layout(Layout::Tab);
        let o = resolve_with("workspace", Role::Orchestrator, Some(&flags), None);
        assert_eq!((o.layout, &o.workspace), (Layout::Tab, &Workspace::Project));
        // A higher level's workspace too.
        let flags = Settings { workspace: Some(Workspace::Named("x".into())), ..Settings::default() };
        let o = resolve_with("workspace", Role::Orchestrator, Some(&flags), None);
        assert_eq!((o.layout, &o.workspace), (Layout::Tab, &Workspace::Named("x".into())));
    }

    #[test]
    fn config_presets_and_layout_are_levels() {
        let cfg = Config::parse(
            config::Scope::Project,
            "layout: { preset: wide, from: first }\nlayout_presets:\n  wide: { layout: split, split: { size: 70% } }\n",
        )
        .unwrap();
        let inputs = Inputs { role: Role::Orchestrator, flags: None, run_flags: None, spec: None };
        let p = resolve_in(&inputs, &cfg, Some("tab".into())).unwrap();
        // TOME_LAYOUT is above the config.
        assert_eq!((p.layout, p.size, p.from), (Layout::Tab, Some(Size::Percent(70)), Some(From::First)));
        assert_eq!(p.sources["layout"], "TOME_LAYOUT");
        assert_eq!(p.sources["split.size"], "preset `wide` (from project config)");
        let listed = presets(&cfg).unwrap();
        assert_eq!(listed["wide"].scope, "project");
        assert_eq!(listed["wide"].settings.describe(), "layout=split split.size=70%");

        let nested = Config::parse(config::Scope::Global, "layout_presets:\n  a: { preset: b }\n").unwrap();
        assert!(presets(&nested).unwrap_err().message.contains("can't reference another preset"));
    }

    #[test]
    fn an_undefined_preset_is_an_error_with_the_known_ones() {
        let (s, _) = spec("preset: nope");
        let inputs = Inputs { role: Role::Orchestrator, flags: None, run_flags: None, spec: s.as_ref() };
        let err = resolve_in(&inputs, &Config::default(), None).unwrap_err();
        assert!(err.message.contains("unknown layout preset `nope` (from `defaults.layout`)"), "{}", err.message);
        assert!(err.hint.unwrap().contains("known presets: split, tab, workspace"));
    }
}
