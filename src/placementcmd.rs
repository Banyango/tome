//! The CLI side of placement: loads presets and the `layout` setting from
//! config, calls the pure rules in [`crate::placement`], and turns their
//! errors into [`CliError`]s (exit kind, hint) and reports.

use crate::config::{self, Config};
use crate::output::{table, CliError, CliResult, Report};
use crate::placement::{
    self, parse_env_layout, parse_settings, parse_value, FlagError, Inputs, Placement,
    PlacementError, Preset, Problem, Settings, Spec, DIRECTIONS, FROMS, LAYOUTS, RESERVED,
};
use serde_json::{json, Value};
use serde_yaml::Value as Yaml;
use std::collections::BTreeMap;
use std::path::Path;

impl From<FlagError> for CliError {
    fn from(e: FlagError) -> Self {
        CliError::invalid(e.to_string())
    }
}

impl From<PlacementError> for CliError {
    fn from(e: PlacementError) -> Self {
        let err = CliError::invalid(e.to_string());
        match e {
            PlacementError::UnknownPreset { known, .. } => err.with_hint(format!(
                "known presets: {}; define others under `layout_presets:` in {} or the project's .tome/config.yaml",
                known.join(", "),
                config::global_path().display()
            )),
            PlacementError::UnknownEnvLayout { .. } => {
                err.with_hint(format!("use {}", LAYOUTS.join(", ")))
            }
            PlacementError::CallerForWorker { .. } => err,
        }
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
    let mut out = placement::built_in_presets();
    for (name, (value, file)) in cfg.entries("layout_presets")? {
        let what = format!("preset `{name}`");
        let mut problems = Vec::new();
        let settings = match &value {
            Yaml::Mapping(m) => parse_settings(
                m,
                &["layout_presets", &name],
                &what,
                &[],
                false,
                &mut problems,
            ),
            _ => {
                return Err(file
                    .error(format!("{what} must be a mapping of placement settings"))
                    .with_hint(config_hint()))
            }
        };
        if let Some(p) = problems.first() {
            return Err(config_problem(file, p));
        }
        out.insert(
            name.clone(),
            Preset {
                name,
                scope: file.scope.as_str(),
                settings,
            },
        );
    }
    Ok(out)
}

/// Fail on a flag's preset that isn't defined, before anything is recorded.
pub fn check_flag_preset(flags: &Settings, source: &str, project: Option<&Path>) -> CliResult<()> {
    let Some(name) = &flags.preset else {
        return Ok(());
    };
    let known = presets(&Config::load(project)?)?;
    if known.contains_key(name) {
        Ok(())
    } else {
        Err(PlacementError::unknown_preset(name, &known, source).into())
    }
}

/// Every preset definition, built-in first, each marked with whether a
/// later one of the same name replaces it.
pub fn list(cfg: &Config) -> CliResult<Vec<(Preset, bool)>> {
    let effective = presets(cfg)?;
    let mut out = placement::built_in_list();
    for file in cfg.files() {
        let Some(Yaml::Mapping(map)) = file.get("layout_presets") else {
            continue;
        };
        for name in map.keys().filter_map(Yaml::as_str) {
            let mut problems = Vec::new();
            let settings = match map.get(name) {
                Some(Yaml::Mapping(m)) => parse_settings(m, &[], "", &[], false, &mut problems),
                _ => Settings::default(),
            };
            out.push(Preset {
                name: name.to_string(),
                scope: file.scope.as_str(),
                settings,
            });
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
            let scope = if *shadowed {
                format!("{} (overridden)", p.scope)
            } else {
                p.scope.to_string()
            };
            vec![p.name.clone(), scope, p.settings.describe()]
        })
        .collect();
    Ok(Report::new(
        json!({ "presets": data }),
        table(&["PRESET", "SCOPE", "SETTINGS"], rows),
    ))
}

/// Resolve a placement for `inputs` in the config that applies to
/// `project`. Unknown values in `TOME_LAYOUT` or the config, and presets
/// that aren't defined, are errors.
pub fn resolve(inputs: &Inputs, project: Option<&Path>) -> CliResult<Placement> {
    let env = std::env::var("TOME_LAYOUT").ok().filter(|s| !s.is_empty());
    resolve_in(inputs, &Config::load(project)?, env)
}

/// [`resolve`] with the config and `TOME_LAYOUT` given.
pub fn resolve_in(
    inputs: &Inputs,
    cfg: &Config,
    env_layout: Option<String>,
) -> CliResult<Placement> {
    let presets = presets(cfg)?;
    let env_layout = env_layout.as_deref().map(parse_env_layout).transpose()?;
    Ok(placement::resolve(
        inputs,
        &presets,
        env_layout,
        config_layout(cfg)?,
    )?)
}

/// The config's `layout` setting and where it came from.
fn config_layout(cfg: &Config) -> CliResult<Option<(Settings, String)>> {
    let Some((value, file)) = cfg.get("layout") else {
        return Ok(None);
    };
    let mut problems = Vec::new();
    let source = format!("{} config", file.scope.as_str());
    let parsed = parse_value(value, &["layout"], "`layout`", &[], &mut problems);
    if !problems.is_empty() {
        // Keep 008's message for an unknown layout name.
        if let Yaml::String(name) = value {
            return Err(CliError::invalid(format!(
                "unknown session layout `{name}` (from `layout` in {})",
                file.label()
            ))
            .with_hint(format!("use {}", LAYOUTS.join(", "))));
        }
        return Err(config_problem(file, &problems[0]));
    }
    Ok(parsed.map(|(s, _)| (s, source)))
}

/// Where `tome session move` puts a session placed at `current`: each
/// setting its `flags` (or their preset) give, the rest as they were. A
/// preset that isn't defined is an error.
pub fn moved(
    current: &Placement,
    flags: &Settings,
    project: Option<&Path>,
) -> CliResult<Placement> {
    let presets = presets(&Config::load(project)?)?;
    Ok(placement::moved(current, flags, &presets)?)
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
        Some(n) => {
            Err(
                PlacementError::unknown_preset(n, &known, "the workflow's `defaults.layout`")
                    .into(),
            )
        }
        None => Ok(()),
    }
}

/// The presets a workflow names that aren't defined, for `tome validate`
/// (a warning: presets depend on the environment).
pub fn undefined_presets(spec: Option<&Spec>, project: Option<&Path>) -> Vec<String> {
    let Some(spec) = spec else { return Vec::new() };
    let Ok(known) = Config::load(project).and_then(|c| presets(&c)) else {
        return Vec::new();
    };
    spec.presets()
        .into_iter()
        .filter(|n| !known.contains_key(*n))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::placement::{parse_spec, From, Role, Size};
    use crate::session::Layout;

    fn spec(yaml: &str) -> (Option<Spec>, Vec<Problem>) {
        let mut problems = Vec::new();
        let v: Yaml = serde_yaml::from_str(yaml).unwrap();
        (parse_spec(&v, &mut problems), problems)
    }

    #[test]
    fn config_presets_and_layout_are_levels() {
        let cfg = Config::parse(
            config::Scope::Project,
            "layout: { preset: wide, from: first }\nlayout_presets:\n  wide: { layout: split, split: { size: 70% } }\n",
        )
        .unwrap();
        let inputs = Inputs {
            role: Role::Orchestrator,
            flags: None,
            run_flags: None,
            spec: None,
            split_workers: true,
        };
        let p = resolve_in(&inputs, &cfg, Some("tab".into())).unwrap();
        // TOME_LAYOUT is above the config.
        assert_eq!(
            (p.layout, p.size, p.from),
            (Layout::Tab, Some(Size::Percent(70)), Some(From::First))
        );
        assert_eq!(p.sources["layout"], "TOME_LAYOUT");
        assert_eq!(
            p.sources["split.size"],
            "preset `wide` (from project config)"
        );
        let listed = presets(&cfg).unwrap();
        assert_eq!(listed["wide"].scope, "project");
        assert_eq!(
            listed["wide"].settings.describe(),
            "layout=split split.size=70%"
        );

        let nested = Config::parse(
            config::Scope::Global,
            "layout_presets:\n  a: { preset: b }\n",
        )
        .unwrap();
        assert!(presets(&nested)
            .unwrap_err()
            .message
            .contains("can't reference another preset"));
    }

    #[test]
    fn caller_is_for_the_orchestrator_only() {
        // A worker's own blocks can't give it.
        for yaml in [
            "workers: [{ match: \"*\", from: caller }]",
            "workers: { from: caller }",
        ] {
            let (_, problems) = spec(yaml);
            assert!(
                problems.iter().any(|p| p
                    .message
                    .contains("`from: caller` is for the run's agent or orchestrator only")),
                "{yaml}: {problems:?}"
            );
            assert_eq!(
                problems[0].path.last().map(String::as_str),
                Some("from"),
                "{problems:?}"
            );
        }
        let (_, problems) = spec("orchestrator: { from: caller }");
        assert!(problems.is_empty(), "{problems:?}");

        // At a shared level it's the orchestrator's; workers take `from`
        // from the next level down.
        let cfg = Config::parse(config::Scope::Project, "layout: { from: last }\n").unwrap();
        let (s, _) = spec("layout: split\nfrom: caller");
        let run = Settings {
            from: Some(From::Caller),
            ..Settings::default()
        };
        let inputs = Inputs {
            role: Role::Orchestrator,
            flags: None,
            run_flags: None,
            spec: s.as_ref(),
            split_workers: true,
        };
        let o = resolve_in(&inputs, &cfg, None).unwrap();
        assert_eq!(
            (o.from, o.sources["from"].as_str()),
            (Some(From::Caller), "`defaults.layout`")
        );
        let inputs = Inputs {
            role: Role::Worker("w"),
            flags: None,
            run_flags: Some(&run),
            spec: s.as_ref(),
            split_workers: true,
        };
        let w = resolve_in(&inputs, &cfg, None).unwrap();
        assert_eq!(
            (w.from, w.sources["from"].as_str()),
            (Some(From::Last), "project config")
        );

        // A worker's own preset giving it is an error.
        let cfg = Config::parse(
            config::Scope::Project,
            "layout_presets:\n  chat: { from: caller }\n",
        )
        .unwrap();
        let flags = Settings {
            preset: Some("chat".into()),
            ..Settings::default()
        };
        let inputs = Inputs {
            role: Role::Worker("w"),
            flags: Some(&flags),
            run_flags: None,
            spec: None,
            split_workers: true,
        };
        let err = resolve_in(&inputs, &cfg, None).unwrap_err();
        assert!(
            err.message
                .contains("(from preset `chat` (from `tome worker spawn` flags))"),
            "{}",
            err.message
        );
    }

    #[test]
    fn an_undefined_preset_is_an_error_with_the_known_ones() {
        let (s, _) = spec("preset: nope");
        let inputs = Inputs {
            role: Role::Orchestrator,
            flags: None,
            run_flags: None,
            spec: s.as_ref(),
            split_workers: true,
        };
        let err = resolve_in(&inputs, &Config::default(), None).unwrap_err();
        assert!(
            err.message
                .contains("unknown layout preset `nope` (from `defaults.layout`)"),
            "{}",
            err.message
        );
        assert!(err
            .hint
            .unwrap()
            .contains("known presets: split, tab, workspace"));
    }
}
