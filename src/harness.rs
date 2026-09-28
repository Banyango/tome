//! The harness adapter: how to launch an agent CLI, as a command template.
//!
//! Harnesses are named in workflows (`defaults.harness`) and defined in
//! `~/.tome/config.yaml` or the project's `.tome/config.yaml` (see
//! [`crate::config`]); `claude` (Claude Code) is built in:
//!
//! ```yaml
//! harnesses:
//!   claude:
//!     command: ["claude", "--model", "opus", "{{prompt}}"]   # argv: one element per argument
//!   aider:
//!     command: aider --message-file {{prompt_file}}   # string: run with `sh -c`
//! ```
//!
//! Template variables: `{{prompt}}` (the agent's instructions),
//! `{{prompt_file}}` (a file holding them), `{{run_id}}`, `{{session}}` and
//! `{{cwd}}`. In the argv form each element is substituted as is; in the
//! string form values are shell-quoted first.

use crate::config::{self, Config};
use crate::output::{CliError, CliResult};
use serde_yaml::Value as Yaml;
use std::collections::BTreeMap;
use std::path::Path;

/// Used when a workflow names no harness.
pub const DEFAULT: &str = "claude";

const VARS: &[&str] = &["prompt", "prompt_file", "run_id", "session", "cwd"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Template {
    Argv(Vec<String>),
    Shell(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Harness {
    pub name: String,
    pub template: Template,
}

fn presets() -> BTreeMap<String, Template> {
    // Claude Code, interactive, with the prompt as its first message. It may
    // run `tome` without asking, so it can report progress on its own.
    // `--allowedTools` takes a list, so `--` stops it swallowing the prompt.
    let claude = ["claude", "--allowedTools", "Bash(tome:*)", "--", "{{prompt}}"];
    BTreeMap::from([("claude".to_string(), Template::Argv(claude.iter().map(|s| s.to_string()).collect()))])
}

/// Every known harness for a project: the presets, overridden or extended
/// by the global config and then the project's.
pub fn all(project: Option<&Path>) -> CliResult<BTreeMap<String, Template>> {
    let cfg = Config::load(project)?;
    let mut out = presets();
    for (name, (spec, file)) in cfg.entries("harnesses").map_err(hinted)? {
        out.insert(name.clone(), parse_harness(&name, &spec).map_err(|e| hinted(file.error(e)))?);
    }
    Ok(out)
}

fn hinted(e: CliError) -> CliError {
    if e.hint.is_some() {
        return e;
    }
    e.with_hint("see `harnesses:` in the tome docs")
}

/// Look a harness up by name, in the config that applies to `project`.
pub fn resolve(name: &str, project: Option<&Path>) -> CliResult<Harness> {
    let mut all = all(project)?;
    match all.remove(name) {
        Some(template) => Ok(Harness { name: name.to_string(), template }),
        None => Err(CliError::invalid(format!("unknown harness `{name}`")).with_hint(format!(
            "known harnesses: {}; define others under `harnesses:` in {} or the project's .tome/config.yaml",
            all.keys().cloned().collect::<Vec<_>>().join(", "),
            config::global_path().display()
        ))),
    }
}

/// One `harnesses:` entry: `name: {command: ...}` or the shorthand
/// `name: <command>`.
fn parse_harness(name: &str, spec: &Yaml) -> Result<Template, String> {
    let command = match spec {
        Yaml::Mapping(m) => {
            if let Some(other) = m.keys().filter_map(Yaml::as_str).find(|k| *k != "command") {
                return Err(format!("harness `{name}`: unknown key `{other}` (expected `command`)"));
            }
            m.get("command").ok_or(format!("harness `{name}`: missing `command`"))?
        }
        other => other,
    };
    let template = match command {
        Yaml::String(s) if !s.trim().is_empty() => Template::Shell(s.clone()),
        Yaml::Sequence(items) if !items.is_empty() => Template::Argv(
            items
                .iter()
                .map(|i| match i {
                    Yaml::String(s) => Ok(s.clone()),
                    Yaml::Number(n) => Ok(n.to_string()),
                    _ => Err(format!("harness `{name}`: command arguments must be strings")),
                })
                .collect::<Result<_, _>>()?,
        ),
        _ => return Err(format!("harness `{name}`: `command` must be a non-empty list or string")),
    };
    check_vars(name, &template)?;
    Ok(template)
}

fn check_vars(name: &str, template: &Template) -> Result<(), String> {
    let parts: Vec<&str> = match template {
        Template::Argv(v) => v.iter().map(String::as_str).collect(),
        Template::Shell(s) => vec![s],
    };
    for part in parts {
        let mut rest = part;
        while let Some(open) = rest.find("{{") {
            let Some(close) = rest[open..].find("}}") else { break };
            let var = rest[open + 2..open + close].trim();
            if !VARS.contains(&var) {
                return Err(format!("harness `{name}`: unknown variable `{{{{{var}}}}}` (available: {})", VARS.join(", ")));
            }
            rest = &rest[open + close + 2..];
        }
    }
    Ok(())
}

/// Values for the template variables.
pub struct Vars<'a> {
    pub prompt: &'a str,
    pub prompt_file: &'a str,
    pub run_id: i64,
    pub session: &'a str,
    pub cwd: &'a str,
}

impl Harness {
    /// The argv to execute.
    pub fn command(&self, vars: &Vars) -> Vec<String> {
        match &self.template {
            Template::Argv(args) => args.iter().map(|a| substitute(a, vars, false)).collect(),
            Template::Shell(s) => vec!["sh".into(), "-c".into(), substitute(s, vars, true)],
        }
    }
}

fn substitute(s: &str, vars: &Vars, quote: bool) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(open) = rest.find("{{") {
        let Some(close) = rest[open..].find("}}") else { break };
        out.push_str(&rest[..open]);
        let value = match rest[open + 2..open + close].trim() {
            "prompt" => vars.prompt.to_string(),
            "prompt_file" => vars.prompt_file.to_string(),
            "run_id" => vars.run_id.to_string(),
            "session" => vars.session.to_string(),
            "cwd" => vars.cwd.to_string(),
            // Rejected when the config was read.
            _ => String::new(),
        };
        out.push_str(&if quote { shell_quote(&value) } else { value });
        rest = &rest[open + close + 2..];
    }
    out.push_str(rest);
    out
}

/// `it's` → `'it'\''s'`
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars() -> Vars<'static> {
        Vars { prompt: "do it's thing", prompt_file: "/r/1/prompt.md", run_id: 7, session: "tome-7-x", cwd: "/p" }
    }

    #[test]
    fn claude_preset_passes_the_prompt_as_one_argument() {
        let h = Harness { name: "claude".into(), template: presets().remove("claude").unwrap() };
        assert_eq!(h.command(&vars()), ["claude", "--allowedTools", "Bash(tome:*)", "--", "do it's thing"]);
    }

    fn parse_config(text: &str) -> Result<BTreeMap<String, Template>, String> {
        let cfg = Config::parse(config::Scope::Global, text).map_err(|e| e.message)?;
        let mut out = BTreeMap::new();
        for (name, (spec, _)) in cfg.entries("harnesses").map_err(|e| e.message)? {
            out.insert(name.clone(), parse_harness(&name, &spec)?);
        }
        Ok(out)
    }

    #[test]
    fn config_defines_and_overrides_harnesses() {
        let cfg = parse_config(
            "harnesses:\n  claude: [claude, --model, opus, \"{{prompt}}\"]\n  aider:\n    command: aider --message-file {{prompt_file}} --tag run-{{ run_id }}\n",
        )
        .unwrap();
        let claude = Harness { name: "claude".into(), template: cfg["claude"].clone() };
        assert_eq!(claude.command(&vars()), ["claude", "--model", "opus", "do it's thing"]);
        let aider = Harness { name: "aider".into(), template: cfg["aider"].clone() };
        assert_eq!(aider.command(&vars()), ["sh", "-c", "aider --message-file '/r/1/prompt.md' --tag run-'7'"]);
    }

    #[test]
    fn shell_form_quotes_values() {
        let h = Harness { name: "x".into(), template: Template::Shell("echo {{prompt}}".into()) };
        assert_eq!(h.command(&vars())[2], r"echo 'do it'\''s thing'");
    }

    #[test]
    fn bad_config_is_explained() {
        for (src, want) in [
            ("harnesses: [a]", "must be a mapping"),
            ("harnesses:\n  x: {cmd: y}", "unknown key `cmd`"),
            ("harnesses:\n  x: []", "non-empty"),
            ("harnesses:\n  x: run {{task}}", "unknown variable `{{task}}`"),
        ] {
            let err = parse_config(src).unwrap_err();
            assert!(err.contains(want), "{src}: {err}");
        }
        assert!(parse_config("").unwrap().is_empty());
    }
}
