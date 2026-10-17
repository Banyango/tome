//! The plugin manifest (`plugin.yaml`) and the rules around it: which hooks
//! exist, whether a plugin fits this tome, and which names it claims. All of
//! it is deterministic: no files, processes or config are read here, so the
//! rules can be tested without installing anything.
//!
//! ```yaml
//! name: cloudvm
//! version: 0.1.0
//! protocol: 1                 # the plugin-RPC version the plugin speaks
//! tome: ">=0.7"               # optional range of tome versions
//! subcommands:
//!   cloud: bin/cloud          # `tome cloud …` execs this, relative to the folder
//! hooks:
//!   provide: { timeout: 10m }
//!   release:
//!   run.before_start: { on_error: allow, timeout: 30s }
//! providers: [cloud-vm]       # names usable as `--on cloud-vm`
//! workflows: [provision]      # workflows/provision.md
//! harnesses:                  # same entries as `harnesses:` in config.yaml
//!   mine: ["mine-agent", "{{prompt}}"]
//! ```

use crate::harness::{self, Def};
use serde_yaml::Value as Yaml;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

/// The plugin-RPC version this tome speaks. It moves independently of tome's
/// own version and of the daemon protocol.
pub const PROTOCOL: u32 = 1;

/// Every hook a plugin can declare, in the order they're documented.
pub const HOOKS: &[&str] = &[
    "provide",
    "release",
    "run.before_start",
    "run.started",
    "run.finished",
    "daemon.started",
    "daemon.stopping",
];

/// Named in the design but not available yet; a manifest declaring it is
/// refused with a pointer to the reason.
const RESERVED_HOOKS: &[&str] = &["worker.place"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnError {
    /// A failing or timed-out before-hook rejects the run.
    Reject,
    Allow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hook {
    pub name: String,
    pub on_error: OnError,
    pub timeout: Option<Duration>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub name: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub protocol: u32,
    pub tome: Option<VersionReq>,
    /// The persistent process the daemon runs for this plugin's hooks, as a
    /// path inside the plugin folder.
    pub command: Option<PathBuf>,
    /// Subcommand name to the command's path inside the plugin folder.
    pub subcommands: BTreeMap<String, PathBuf>,
    pub hooks: Vec<Hook>,
    pub providers: Vec<String>,
    pub workflows: Vec<String>,
    pub harnesses: BTreeMap<String, Def>,
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Manifest, String> {
        let doc: Yaml = serde_yaml::from_str(text).map_err(|e| format!("invalid YAML: {e}"))?;
        let map = doc
            .as_mapping()
            .ok_or("the manifest must be a mapping with at least `name` and `protocol`")?;
        for key in map.keys() {
            let key = key.as_str().unwrap_or("?");
            if !KEYS.contains(&key) {
                return Err(format!("unknown key `{key}` (known: {})", KEYS.join(", ")));
            }
        }
        let get = |key: &str| map.get(key).filter(|v| !v.is_null());

        let name = text_field(get("name"), "name")?.ok_or("`name` is required")?;
        check_ident(&name, "plugin name")?;
        let protocol = match get("protocol") {
            Some(Yaml::Number(n)) => n
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or("`protocol` must be a whole number")?,
            Some(_) => return Err("`protocol` must be a whole number".into()),
            None => return Err("`protocol` is required: the plugin-RPC version it speaks".into()),
        };
        let version = match get("version") {
            Some(Yaml::Number(n)) => Some(n.to_string()),
            other => text_field(other, "version")?,
        };
        let tome = match get("tome") {
            None => None,
            Some(Yaml::String(s)) => {
                Some(VersionReq::parse(s).map_err(|e| format!("`tome`: {e}"))?)
            }
            Some(_) => return Err("`tome` must be a version range string like \">=0.7\"".into()),
        };

        let command = match get("command") {
            None => None,
            Some(Yaml::String(s)) if !s.trim().is_empty() => Some(check_relative(s, "command")?),
            Some(_) => return Err("`command` is the path of the plugin's hook process".into()),
        };

        let mut subcommands = BTreeMap::new();
        match get("subcommands") {
            None => {}
            Some(Yaml::Mapping(m)) => {
                for (k, v) in m {
                    let cmd = k.as_str().ok_or("`subcommands` keys must be names")?;
                    check_ident(cmd, "subcommand name")?;
                    let path = v.as_str().filter(|s| !s.trim().is_empty()).ok_or_else(|| {
                        format!("subcommand `{cmd}`: give the path of its executable")
                    })?;
                    subcommands.insert(cmd.to_string(), check_relative(path, cmd)?);
                }
            }
            Some(_) => return Err("`subcommands` maps a name to an executable path".into()),
        }

        let hooks = parse_hooks(get("hooks"))?;
        let providers = name_list(get("providers"), "providers", |n| {
            crate::ids::check_node_name(n)
        })?;
        let workflows = name_list(get("workflows"), "workflows", |n| {
            if crate::text::is_identifier(n) {
                Ok(())
            } else {
                Err(format!(
                    "invalid workflow name `{n}`: use letters, digits, `_` and `-`"
                ))
            }
        })?;

        let mut harnesses = BTreeMap::new();
        match get("harnesses") {
            None => {}
            Some(Yaml::Mapping(m)) => {
                for (k, v) in m {
                    let h = k.as_str().ok_or("`harnesses` keys must be names")?;
                    harnesses.insert(h.to_string(), harness::parse_harness(h, v)?);
                }
            }
            Some(_) => return Err("`harnesses` maps a name to its definition".into()),
        }

        let has = |hook: &str| hooks.iter().any(|h| h.name == hook);
        if !providers.is_empty() && !(has("provide") && has("release")) {
            return Err("`providers` needs both the `provide` and `release` hooks".into());
        }
        if providers.is_empty() && (has("provide") || has("release")) {
            return Err("the `provide` and `release` hooks need a name under `providers`".into());
        }

        Ok(Manifest {
            name,
            version,
            description: text_field(get("description"), "description")?,
            protocol,
            tome,
            command,
            subcommands,
            hooks,
            providers,
            workflows,
            harnesses,
        })
    }

    pub fn hook_names(&self) -> Vec<&str> {
        self.hooks.iter().map(|h| h.name.as_str()).collect()
    }

    /// The names this plugin takes, for [`conflicts`]. `source` labels them
    /// in messages, e.g. ``plugin `cloudvm` ``.
    pub fn claims(&self, source: &str) -> Vec<Claim> {
        let claim = |space, name: &String| Claim {
            space,
            name: name.clone(),
            source: source.to_string(),
        };
        let mut out = vec![claim(Space::Plugin, &self.name)];
        out.extend(self.subcommands.keys().map(|n| claim(Space::Command, n)));
        out.extend(self.providers.iter().map(|n| claim(Space::Provider, n)));
        out.extend(self.workflows.iter().map(|n| claim(Space::Workflow, n)));
        out.extend(self.harnesses.keys().map(|n| claim(Space::Harness, n)));
        out
    }
}

const KEYS: &[&str] = &[
    "name",
    "version",
    "description",
    "protocol",
    "tome",
    "command",
    "subcommands",
    "hooks",
    "providers",
    "workflows",
    "harnesses",
];

fn text_field(value: Option<&Yaml>, key: &str) -> Result<Option<String>, String> {
    match value {
        None => Ok(None),
        Some(Yaml::String(s)) if !s.trim().is_empty() => Ok(Some(s.trim().to_string())),
        Some(_) => Err(format!("`{key}` must be a non-empty string")),
    }
}

/// Plugin, subcommand and provider names: `[a-z0-9_-]`, starting with a
/// letter or digit so they can't read as flags.
pub fn check_ident(name: &str, what: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && !name.starts_with(['-', '_'])
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    if ok {
        Ok(())
    } else {
        Err(format!(
            "invalid {what} `{name}`: use lowercase letters, digits, `_` and `-`, starting with a letter or digit"
        ))
    }
}

/// A path inside the plugin folder: relative, no `..`.
fn check_relative(path: &str, what: &str) -> Result<PathBuf, String> {
    let p = Path::new(path.trim());
    let escapes = p.is_absolute()
        || p.components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir));
    if escapes {
        return Err(format!(
            "`{what}`: `{path}` must be a path inside the plugin folder (relative, no `..`)"
        ));
    }
    Ok(p.to_path_buf())
}

fn name_list(
    value: Option<&Yaml>,
    key: &str,
    check: impl Fn(&str) -> Result<(), String>,
) -> Result<Vec<String>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let items = value
        .as_sequence()
        .ok_or_else(|| format!("`{key}` must be a list of names"))?;
    let mut out: Vec<String> = Vec::new();
    for item in items {
        let name = item
            .as_str()
            .ok_or_else(|| format!("`{key}` must be a list of names"))?;
        check(name)?;
        if out.iter().any(|n| n == name) {
            return Err(format!("`{key}` lists `{name}` twice"));
        }
        out.push(name.to_string());
    }
    Ok(out)
}

/// `hooks:` is a list of names, or a mapping from name to options (or
/// nothing).
fn parse_hooks(value: Option<&Yaml>) -> Result<Vec<Hook>, String> {
    let entries: Vec<(String, Option<&Yaml>)> = match value {
        None => return Ok(Vec::new()),
        Some(Yaml::Sequence(items)) => items
            .iter()
            .map(|i| {
                i.as_str()
                    .map(|n| (n.to_string(), None))
                    .ok_or("`hooks` must be a list of hook names, or a mapping of name to options")
            })
            .collect::<Result<_, _>>()?,
        Some(Yaml::Mapping(m)) => m
            .iter()
            .map(|(k, v)| {
                k.as_str()
                    .map(|n| (n.to_string(), Some(v).filter(|v| !v.is_null())))
                    .ok_or("`hooks` keys must be hook names")
            })
            .collect::<Result<_, _>>()?,
        Some(_) => {
            return Err(
                "`hooks` must be a list of hook names, or a mapping of name to options".into(),
            )
        }
    };

    let mut hooks: Vec<Hook> = Vec::new();
    for (name, options) in entries {
        if RESERVED_HOOKS.contains(&name.as_str()) {
            return Err(format!("hook `{name}` is not yet supported"));
        }
        if !HOOKS.contains(&name.as_str()) {
            return Err(format!(
                "unknown hook `{name}` (known: {})",
                HOOKS.join(", ")
            ));
        }
        if hooks.iter().any(|h| h.name == name) {
            return Err(format!("hook `{name}` is declared twice"));
        }
        let mut hook = Hook {
            name: name.clone(),
            on_error: OnError::Reject,
            timeout: None,
        };
        if let Some(options) = options {
            let opts = options
                .as_mapping()
                .ok_or_else(|| format!("hook `{name}`: options must be a mapping"))?;
            for (k, v) in opts {
                match k.as_str() {
                    Some("on_error") => {
                        if name != "run.before_start" {
                            return Err(format!(
                                "hook `{name}`: `on_error` only applies to `run.before_start`"
                            ));
                        }
                        hook.on_error = match v.as_str() {
                            Some("allow") => OnError::Allow,
                            Some("reject") => OnError::Reject,
                            _ => {
                                return Err(format!(
                                    "hook `{name}`: `on_error` must be `allow` or `reject`"
                                ))
                            }
                        };
                    }
                    Some("timeout") => {
                        let text = match v {
                            Yaml::String(s) => s.clone(),
                            Yaml::Number(n) => n.to_string(),
                            _ => {
                                return Err(format!(
                                    "hook `{name}`: `timeout` must be a duration like 30s"
                                ))
                            }
                        };
                        let d = crate::duration::parse(&text)
                            .map_err(|e| format!("hook `{name}`: {e}"))?;
                        if d.is_zero() {
                            return Err(format!("hook `{name}`: `timeout` must be more than zero"));
                        }
                        hook.timeout = Some(d);
                    }
                    other => {
                        return Err(format!(
                            "hook `{name}`: unknown option `{}` (known: on_error, timeout)",
                            other.unwrap_or("?")
                        ))
                    }
                }
            }
        }
        hooks.push(hook);
    }
    Ok(hooks)
}

// ---------------------------------------------------------------------------
// Versions

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u64, pub u64, pub u64);

impl Version {
    /// `1`, `1.2` or `1.2.3`, with any `-pre` or `+build` suffix ignored.
    pub fn parse(s: &str) -> Result<Version, String> {
        let core = s.trim().split(['-', '+']).next().unwrap_or("");
        let mut parts = [0u64; 3];
        let mut seen = 0;
        for piece in core.split('.') {
            if seen == 3 {
                return Err(format!("invalid version `{s}`"));
            }
            parts[seen] = piece
                .parse()
                .map_err(|_| format!("invalid version `{s}`"))?;
            seen += 1;
        }
        Ok(Version(parts[0], parts[1], parts[2]))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Ge,
    Gt,
    Le,
    Lt,
    Eq,
}

/// A range of versions: comparators joined by commas or spaces, all of which
/// must hold (`>=0.7, <1`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionReq {
    text: String,
    comparators: Vec<(Op, Version)>,
}

impl VersionReq {
    pub fn parse(s: &str) -> Result<VersionReq, String> {
        let mut comparators = Vec::new();
        let spaced = s.replace(',', " ");
        let mut words = spaced.split_whitespace().peekable();
        while let Some(word) = words.next() {
            let (op, rest) = [
                (">=", Op::Ge),
                ("<=", Op::Le),
                (">", Op::Gt),
                ("<", Op::Lt),
                ("=", Op::Eq),
            ]
            .iter()
            .find_map(|(sym, op)| word.strip_prefix(sym).map(|r| (*op, r)))
            .unwrap_or((Op::Eq, word));
            // Allow `>= 0.7` with a space after the operator.
            let version = if rest.is_empty() {
                words
                    .next()
                    .ok_or_else(|| format!("version range `{s}` ends after an operator"))?
            } else {
                rest
            };
            comparators.push((op, Version::parse(version)?));
        }
        if comparators.is_empty() {
            return Err("empty version range".into());
        }
        Ok(VersionReq {
            text: s.trim().to_string(),
            comparators,
        })
    }

    pub fn matches(&self, v: &Version) -> bool {
        self.comparators.iter().all(|(op, bound)| match op {
            Op::Ge => v >= bound,
            Op::Gt => v > bound,
            Op::Le => v <= bound,
            Op::Lt => v < bound,
            Op::Eq => v == bound,
        })
    }

    /// Whether `v` is below the range's lower bounds, so the way out is a
    /// newer tome rather than a different plugin.
    fn too_low(&self, v: &Version) -> bool {
        self.comparators.iter().any(|(op, bound)| match op {
            Op::Ge => v < bound,
            Op::Gt => v <= bound,
            Op::Eq => v < bound,
            Op::Le | Op::Lt => false,
        })
    }
}

impl fmt::Display for VersionReq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

// ---------------------------------------------------------------------------
// Compatibility

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Incompatible {
    /// The plugin speaks a different plugin-RPC version.
    Protocol { plugin: u32, tome: u32 },
    /// The plugin wants a different tome.
    Tome {
        required: String,
        running: Version,
        upgrade_tome: bool,
    },
}

impl Incompatible {
    pub fn message(&self) -> String {
        match self {
            Incompatible::Protocol { plugin, tome } => {
                format!("speaks plugin protocol {plugin}; this tome speaks {tome}")
            }
            Incompatible::Tome {
                required, running, ..
            } => format!("needs tome {required}; this is tome {running}"),
        }
    }

    /// Which side to upgrade.
    pub fn hint(&self, plugin: &str) -> String {
        let upgrade_tome = match self {
            Incompatible::Protocol { plugin, tome } => plugin > tome,
            Incompatible::Tome { upgrade_tome, .. } => *upgrade_tome,
        };
        if upgrade_tome {
            "upgrade tome".to_string()
        } else {
            format!("upgrade the plugin: `tome plugin update {plugin}`")
        }
    }
}

pub fn compatibility(m: &Manifest, tome: &Version) -> Result<(), Incompatible> {
    if m.protocol != PROTOCOL {
        return Err(Incompatible::Protocol {
            plugin: m.protocol,
            tome: PROTOCOL,
        });
    }
    if let Some(req) = &m.tome {
        if !req.matches(tome) {
            return Err(Incompatible::Tome {
                required: req.to_string(),
                running: *tome,
                upgrade_tome: req.too_low(tome),
            });
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Names

/// A set of names that must each be unique.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Space {
    Plugin,
    /// Built-in commands and plugin subcommands.
    Command,
    /// Configured nodes and plugin providers (`--on <name>`).
    Provider,
    Workflow,
    Harness,
}

impl Space {
    /// The stable name used in JSON output.
    pub fn as_str(self) -> &'static str {
        match self {
            Space::Plugin => "plugin",
            Space::Command => "command",
            Space::Provider => "provider",
            Space::Workflow => "workflow",
            Space::Harness => "harness",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Space::Plugin => "plugin",
            Space::Command => "command",
            Space::Provider => "node or provider",
            Space::Workflow => "workflow",
            Space::Harness => "harness",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    pub space: Space,
    pub name: String,
    /// Who holds the name, in words: ``built-in command``, ``plugin `x` ``.
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub space: Space,
    pub name: String,
    pub ours: String,
    pub theirs: String,
}

impl Conflict {
    pub fn message(&self) -> String {
        format!(
            "{} `{}` is claimed by both {} and {}",
            self.space.label(),
            self.name,
            self.ours,
            self.theirs
        )
    }
}

/// Names in `mine` that `others` already holds.
pub fn conflicts(mine: &[Claim], others: &[Claim]) -> Vec<Conflict> {
    let taken: BTreeMap<(Space, &str), &str> = others
        .iter()
        .map(|c| ((c.space, c.name.as_str()), c.source.as_str()))
        .collect();
    let mut seen = BTreeSet::new();
    mine.iter()
        .filter_map(|c| {
            let theirs = taken.get(&(c.space, c.name.as_str()))?;
            seen.insert((c.space, c.name.clone())).then(|| Conflict {
                space: c.space,
                name: c.name.clone(),
                ours: c.source.clone(),
                theirs: theirs.to_string(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = r#"
name: cloudvm
version: 0.1.0
protocol: 1
tome: ">=0.6, <1"
subcommands:
  cloud: bin/cloud
hooks:
  provide: { timeout: 10m }
  release:
  run.before_start: { on_error: allow, timeout: 30s }
  run.finished:
providers: [cloud-vm]
workflows: [provision]
harnesses:
  mine: ["mine-agent", "{{prompt}}"]
"#;

    #[test]
    fn parses_a_full_manifest() {
        let m = Manifest::parse(FULL).unwrap();
        assert_eq!(m.name, "cloudvm");
        assert_eq!(m.version.as_deref(), Some("0.1.0"));
        assert_eq!(m.subcommands["cloud"], PathBuf::from("bin/cloud"));
        assert_eq!(
            m.hook_names(),
            ["provide", "release", "run.before_start", "run.finished"]
        );
        let before = &m.hooks[2];
        assert_eq!(before.on_error, OnError::Allow);
        assert_eq!(before.timeout, Some(Duration::from_secs(30)));
        assert_eq!(m.hooks[0].timeout, Some(Duration::from_secs(600)));
        assert_eq!(m.providers, ["cloud-vm"]);
        assert_eq!(m.workflows, ["provision"]);
        assert!(m.harnesses.contains_key("mine"));
    }

    #[test]
    fn hooks_can_be_a_plain_list() {
        let m = Manifest::parse("name: a\nprotocol: 1\nhooks: [run.started, daemon.started]\n")
            .unwrap();
        assert_eq!(m.hook_names(), ["run.started", "daemon.started"]);
        assert_eq!(m.hooks[0].on_error, OnError::Reject);
    }

    fn err(text: &str) -> String {
        Manifest::parse(text).unwrap_err()
    }

    #[test]
    fn rejects_unknown_and_reserved_hooks() {
        assert!(err("name: a\nprotocol: 1\nhooks: [run.exploded]\n").contains("unknown hook"));
        assert_eq!(
            err("name: a\nprotocol: 1\nhooks: [worker.place]\n"),
            "hook `worker.place` is not yet supported"
        );
    }

    #[test]
    fn requires_name_and_protocol() {
        assert!(err("protocol: 1\n").contains("`name` is required"));
        assert!(err("name: a\n").contains("`protocol` is required"));
        assert!(err("name: a\nprotocol: one\n").contains("whole number"));
    }

    #[test]
    fn rejects_unknown_keys_and_bad_names() {
        assert!(err("name: a\nprotocol: 1\nextra: 1\n").contains("unknown key `extra`"));
        assert!(err("name: Bad\nprotocol: 1\n").contains("invalid plugin name"));
        assert!(err("name: -x\nprotocol: 1\n").contains("invalid plugin name"));
    }

    #[test]
    fn subcommand_paths_stay_inside_the_folder() {
        assert!(
            err("name: a\nprotocol: 1\nsubcommands: {x: ../out}\n").contains("inside the plugin")
        );
        assert!(
            err("name: a\nprotocol: 1\nsubcommands: {x: /bin/sh}\n").contains("inside the plugin")
        );
    }

    #[test]
    fn providers_need_both_provider_hooks() {
        assert!(
            err("name: a\nprotocol: 1\nproviders: [p]\nhooks: [provide]\n")
                .contains("both the `provide` and `release`")
        );
        assert!(err("name: a\nprotocol: 1\nhooks: [provide, release]\n")
            .contains("need a name under `providers`"));
        assert!(
            err("name: a\nprotocol: 1\nproviders: [local]\nhooks: [provide, release]\n")
                .contains("reserved")
        );
    }

    #[test]
    fn on_error_is_for_before_start_only() {
        assert!(
            err("name: a\nprotocol: 1\nhooks:\n  run.started: {on_error: allow}\n")
                .contains("only applies to `run.before_start`")
        );
        assert!(
            err("name: a\nprotocol: 1\nhooks:\n  run.before_start: {on_error: maybe}\n")
                .contains("`allow` or `reject`")
        );
    }

    #[test]
    fn version_ranges() {
        let req = VersionReq::parse(">=0.6, <1").unwrap();
        assert!(req.matches(&Version(0, 6, 0)));
        assert!(req.matches(&Version(0, 9, 3)));
        assert!(!req.matches(&Version(0, 5, 9)));
        assert!(!req.matches(&Version(1, 0, 0)));
        assert!(VersionReq::parse(">= 0.6")
            .unwrap()
            .matches(&Version(0, 7, 0)));
        assert!(VersionReq::parse("0.6").unwrap().matches(&Version(0, 6, 0)));
        assert!(VersionReq::parse("").is_err());
        assert!(VersionReq::parse(">=x").is_err());
        assert_eq!(Version::parse("1.2.3-rc.1").unwrap(), Version(1, 2, 3));
        assert_eq!(Version::parse("2").unwrap(), Version(2, 0, 0));
    }

    #[test]
    fn compatibility_names_the_side_to_upgrade() {
        let tome = Version(0, 6, 0);
        let mut m = Manifest::parse("name: a\nprotocol: 1\ntome: \">=0.9\"\n").unwrap();
        let e = compatibility(&m, &tome).unwrap_err();
        assert_eq!(e.message(), "needs tome >=0.9; this is tome 0.6.0");
        assert_eq!(e.hint("a"), "upgrade tome");

        m.tome = Some(VersionReq::parse("<0.5").unwrap());
        let e = compatibility(&m, &tome).unwrap_err();
        assert_eq!(e.hint("a"), "upgrade the plugin: `tome plugin update a`");

        m.tome = None;
        m.protocol = 2;
        let e = compatibility(&m, &tome).unwrap_err();
        assert!(e.message().contains("protocol 2"));
        assert_eq!(e.hint("a"), "upgrade tome");
        m.protocol = 0;
        assert!(compatibility(&m, &tome)
            .unwrap_err()
            .hint("a")
            .contains("tome plugin update"));

        m.protocol = PROTOCOL;
        assert!(compatibility(&m, &tome).is_ok());
    }

    fn claim(space: Space, name: &str, source: &str) -> Claim {
        Claim {
            space,
            name: name.into(),
            source: source.into(),
        }
    }

    #[test]
    fn conflicts_match_within_a_space_only() {
        let m = Manifest::parse(FULL).unwrap();
        let mine = m.claims("plugin `cloudvm`");
        let others = [
            claim(Space::Command, "cloud", "built-in command"),
            claim(Space::Workflow, "cloud", "your workflow"), // other space: no clash
            claim(Space::Provider, "cloud-vm", "configured node `cloud-vm`"),
            claim(Space::Workflow, "provision", "plugin `other`"),
            claim(Space::Harness, "mine", "built-in harness"),
        ];
        let found = conflicts(&mine, &others);
        let names: Vec<_> = found.iter().map(|c| (c.space, c.name.as_str())).collect();
        assert_eq!(
            names,
            [
                (Space::Command, "cloud"),
                (Space::Provider, "cloud-vm"),
                (Space::Workflow, "provision"),
                (Space::Harness, "mine"),
            ]
        );
        assert_eq!(
            found[1].message(),
            "node or provider `cloud-vm` is claimed by both plugin `cloudvm` and configured node `cloud-vm`"
        );
        assert!(conflicts(&mine, &[]).is_empty());
    }
}
