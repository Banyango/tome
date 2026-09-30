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
//!     command: aider --message-file {{prompt_file}} --model {{model}}
//!   codex:
//!     command: ["codex", "{{prompt}}"]
//!     model_flag: --model   # put `--model <m>` after the program when a model is set
//! ```
//!
//! Template variables: `{{prompt}}` (the agent's instructions),
//! `{{prompt_file}}` (a file holding them), `{{run_id}}`, `{{session}}`,
//! `{{cwd}}` and `{{model}}` (the workflow's model, empty when none is set).
//! In the argv form each element is substituted as is; in the string form
//! values are shell-quoted first.
//!
//! A harness takes a model in one of two ways: `model_flag` (argv form only)
//! or a `{{model}}` in its command. One that does neither refuses a model.

use crate::config::{self, Config};
use crate::output::{CliError, CliResult};
use serde_yaml::Value as Yaml;
use std::collections::BTreeMap;
use std::path::Path;

/// Used when a workflow names no harness.
pub const DEFAULT: &str = "claude";

const VARS: &[&str] = &["prompt", "prompt_file", "run_id", "session", "cwd", "model"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Template {
    Argv(Vec<String>),
    Shell(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Def {
    pub template: Template,
    /// The flag that takes a model, such as `--model`.
    pub model_flag: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Harness {
    pub name: String,
    pub template: Template,
    pub model_flag: Option<String>,
}

fn presets() -> BTreeMap<String, Def> {
    // Claude Code, interactive, with the prompt as its first message. It may
    // run `tome` without asking, so it can report progress on its own.
    // `--allowedTools` takes a list, so `--` stops it swallowing the prompt.
    let claude = [
        "claude",
        "--allowedTools",
        "Bash(tome:*)",
        "--",
        "{{prompt}}",
    ];
    BTreeMap::from([(
        "claude".to_string(),
        Def {
            template: Template::Argv(claude.iter().map(|s| s.to_string()).collect()),
            model_flag: Some("--model".to_string()),
        },
    )])
}

/// Every known harness for a project: the presets, overridden or extended
/// by the global config and then the project's.
pub fn all(project: Option<&Path>) -> CliResult<BTreeMap<String, Def>> {
    let cfg = Config::load(project)?;
    let mut out = presets();
    for (name, (spec, file)) in cfg.entries("harnesses").map_err(hinted)? {
        out.insert(
            name.clone(),
            parse_harness(&name, &spec).map_err(|e| hinted(file.error(e)))?,
        );
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
        Some(def) => Ok(Harness {
            name: name.to_string(),
            template: def.template,
            model_flag: def.model_flag,
        }),
        None => Err(CliError::invalid(format!("unknown harness `{name}`")).with_hint(format!(
            "known harnesses: {}; define others under `harnesses:` in {} or the project's .tome/config.yaml",
            all.keys().cloned().collect::<Vec<_>>().join(", "),
            config::global_path().display()
        ))),
    }
}

/// One `harnesses:` entry: `name: {command: ..., model_flag: ...}` or the
/// shorthand `name: <command>`.
fn parse_harness(name: &str, spec: &Yaml) -> Result<Def, String> {
    let mut model_flag = None;
    let command = match spec {
        Yaml::Mapping(m) => {
            if let Some(other) = m
                .keys()
                .filter_map(Yaml::as_str)
                .find(|k| !matches!(*k, "command" | "model_flag"))
            {
                return Err(format!(
                    "harness `{name}`: unknown key `{other}` (expected `command` or `model_flag`)"
                ));
            }
            if let Some(flag) = m.get("model_flag") {
                match flag {
                    Yaml::String(s) if !s.trim().is_empty() => model_flag = Some(s.clone()),
                    _ => {
                        return Err(format!(
                            "harness `{name}`: `model_flag` must be a non-empty string"
                        ))
                    }
                }
            }
            m.get("command")
                .ok_or(format!("harness `{name}`: missing `command`"))?
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
                    _ => Err(format!(
                        "harness `{name}`: command arguments must be strings"
                    )),
                })
                .collect::<Result<_, _>>()?,
        ),
        _ => {
            return Err(format!(
                "harness `{name}`: `command` must be a non-empty list or string"
            ))
        }
    };
    check_vars(name, &template)?;
    if model_flag.is_some() && matches!(template, Template::Shell(_)) {
        return Err(format!(
            "harness `{name}`: `model_flag` needs a command list; put `{{{{model}}}}` in the command string instead"
        ));
    }
    Ok(Def {
        template,
        model_flag,
    })
}

fn check_vars(name: &str, template: &Template) -> Result<(), String> {
    let parts: Vec<&str> = match template {
        Template::Argv(v) => v.iter().map(String::as_str).collect(),
        Template::Shell(s) => vec![s],
    };
    for part in parts {
        let mut rest = part;
        while let Some(open) = rest.find("{{") {
            let Some(close) = rest[open..].find("}}") else {
                break;
            };
            let var = rest[open + 2..open + close].trim();
            if !VARS.contains(&var) {
                return Err(format!(
                    "harness `{name}`: unknown variable `{{{{{var}}}}}` (available: {})",
                    VARS.join(", ")
                ));
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
    pub model: Option<&'a str>,
}

impl Harness {
    /// Whether a model can reach the agent: through `model_flag` or a
    /// `{{model}}` in the command.
    fn takes_model(&self) -> bool {
        self.model_flag.is_some()
            || match &self.template {
                Template::Argv(args) => args.iter().any(|a| mentions_model(a)),
                Template::Shell(s) => mentions_model(s),
            }
    }

    /// Refuse a model this harness has no way to pass on.
    pub fn check_model(&self, model: Option<&str>) -> CliResult<()> {
        match model {
            Some(m) if !self.takes_model() => Err(CliError::invalid(format!(
                "harness `{}` can't take the model `{m}`",
                self.name
            ))
            .with_hint(
                "add `model_flag: --model` (or the flag it uses) to the harness, or put `{{model}}` in its command",
            )),
            _ => Ok(()),
        }
    }

    /// The argv to execute.
    pub fn command(&self, vars: &Vars) -> Vec<String> {
        match &self.template {
            Template::Argv(args) => {
                let mut argv: Vec<String> =
                    args.iter().map(|a| substitute(a, vars, false)).collect();
                if let (Some(flag), Some(model)) = (&self.model_flag, vars.model) {
                    argv.splice(1..1, [flag.clone(), model.to_string()]);
                }
                argv
            }
            Template::Shell(s) => vec!["sh".into(), "-c".into(), substitute(s, vars, true)],
        }
    }
}

fn mentions_model(s: &str) -> bool {
    let mut rest = s;
    while let Some(open) = rest.find("{{") {
        let Some(close) = rest[open..].find("}}") else {
            return false;
        };
        if rest[open + 2..open + close].trim() == "model" {
            return true;
        }
        rest = &rest[open + close + 2..];
    }
    false
}

fn substitute(s: &str, vars: &Vars, quote: bool) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(open) = rest.find("{{") {
        let Some(close) = rest[open..].find("}}") else {
            break;
        };
        out.push_str(&rest[..open]);
        let value = match rest[open + 2..open + close].trim() {
            "prompt" => vars.prompt.to_string(),
            "prompt_file" => vars.prompt_file.to_string(),
            "run_id" => vars.run_id.to_string(),
            "session" => vars.session.to_string(),
            "cwd" => vars.cwd.to_string(),
            "model" => vars.model.unwrap_or_default().to_string(),
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
        Vars {
            prompt: "do it's thing",
            prompt_file: "/r/1/prompt.md",
            run_id: 7,
            session: "tome-7-x",
            cwd: "/p",
            model: None,
        }
    }

    #[test]
    fn claude_preset_passes_the_prompt_as_one_argument() {
        let h = harness("claude", presets().remove("claude").unwrap());
        assert_eq!(
            h.command(&vars()),
            [
                "claude",
                "--allowedTools",
                "Bash(tome:*)",
                "--",
                "do it's thing"
            ]
        );
    }

    fn harness(name: &str, def: Def) -> Harness {
        Harness {
            name: name.into(),
            template: def.template,
            model_flag: def.model_flag,
        }
    }

    fn parse_config(text: &str) -> Result<BTreeMap<String, Def>, String> {
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
        let claude = harness("claude", cfg["claude"].clone());
        assert_eq!(
            claude.command(&vars()),
            ["claude", "--model", "opus", "do it's thing"]
        );
        let aider = harness("aider", cfg["aider"].clone());
        assert_eq!(
            aider.command(&vars()),
            [
                "sh",
                "-c",
                "aider --message-file '/r/1/prompt.md' --tag run-'7'"
            ]
        );
    }

    #[test]
    fn shell_form_quotes_values() {
        let h = harness(
            "x",
            Def {
                template: Template::Shell("echo {{prompt}}".into()),
                model_flag: None,
            },
        );
        assert_eq!(h.command(&vars())[2], r"echo 'do it'\''s thing'");
    }

    #[test]
    fn bad_config_is_explained() {
        for (src, want) in [
            ("harnesses: [a]", "must be a mapping"),
            ("harnesses:\n  x: {cmd: y}", "unknown key `cmd`"),
            ("harnesses:\n  x: []", "non-empty"),
            (
                "harnesses:\n  x: run {{task}}",
                "unknown variable `{{task}}`",
            ),
        ] {
            let err = parse_config(src).unwrap_err();
            assert!(err.contains(want), "{src}: {err}");
        }
        assert!(parse_config("").unwrap().is_empty());
    }

    #[test]
    fn a_model_goes_through_model_flag_or_the_model_variable() {
        let with = |m| Vars {
            model: Some(m),
            ..vars()
        };
        let claude = harness("claude", presets().remove("claude").unwrap());
        assert_eq!(
            claude.command(&with("opus")),
            [
                "claude",
                "--model",
                "opus",
                "--allowedTools",
                "Bash(tome:*)",
                "--",
                "do it's thing"
            ]
        );
        claude.check_model(Some("opus")).unwrap();

        let cfg = parse_config(
            "harnesses:\n  aider: aider --model {{model}}\n  plain: [tool, \"{{prompt}}\"]\n  flagged:\n    command: [tool]\n    model_flag: -m\n",
        )
        .unwrap();
        let aider = harness("aider", cfg["aider"].clone());
        aider.check_model(Some("gpt")).unwrap();
        assert_eq!(aider.command(&with("gpt"))[2], "aider --model 'gpt'");
        assert_eq!(aider.command(&vars())[2], "aider --model ''");

        let flagged = harness("flagged", cfg["flagged"].clone());
        assert_eq!(flagged.command(&with("m1")), ["tool", "-m", "m1"]);

        let plain = harness("plain", cfg["plain"].clone());
        plain.check_model(None).unwrap();
        let err = plain.check_model(Some("x")).unwrap_err();
        assert!(
            err.message.contains("can't take the model `x`"),
            "{}",
            err.message
        );
    }

    #[test]
    fn model_flag_needs_an_argv_command() {
        let err = parse_config("harnesses:\n  x:\n    command: run it\n    model_flag: -m\n")
            .unwrap_err();
        assert!(err.contains("model_flag"), "{err}");
    }
}
