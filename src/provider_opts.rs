//! `--provider-opt key=value` and `defaults.provider_opts`: the options a node
//! provider is given. Pure parsing, shared by workflow frontmatter and the
//! provider commands.

use serde_yaml::Value as Yaml;
use std::collections::BTreeMap;

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
}
