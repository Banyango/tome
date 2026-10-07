//! Diagnostics for configured agent harnesses.

use crate::harness::{self, Harness, Template, Vars};
use crate::ids::RunId;
use crate::output::{exit, CliError, CliResult, Report};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// `tome harness validate [name]`: check configuration and whether the
/// configured program can be found, without launching an interactive agent.
pub fn validate(project: &Path, name: Option<&str>, model: Option<&str>) -> CliResult<Report> {
    let known = harness::all(Some(project))?;
    let names: Vec<String> = match name {
        Some(name) if known.contains_key(name) => vec![name.to_string()],
        Some(name) => {
            return Err(
                CliError::invalid(format!("unknown harness `{name}`")).with_hint(format!(
                    "known harnesses: {}",
                    known.keys().cloned().collect::<Vec<_>>().join(", ")
                )),
            );
        }
        None => known.keys().cloned().collect(),
    };

    let mut human = String::new();
    let mut results = Vec::new();
    let mut ok = true;
    for name in names {
        let h = harness::resolve(&name, Some(project))?;
        let found = match &h.template {
            Template::Argv(args) => args.first().and_then(|program| find_program(program)),
            Template::Shell(_) => find_program("sh"),
        };
        let executable_ok = found.is_some();
        let model_error = model.and_then(|m| h.check_model(Some(m)).err());
        let shell_error = match &h.template {
            Template::Shell(command) => shell_syntax_error(command),
            Template::Argv(_) => None,
        };
        let config_ok = model_error.is_none() && shell_error.is_none();
        let harness_ok = executable_ok && config_ok;
        ok &= harness_ok;

        let preview = preview(&h, model);
        let program = program_name(&h);
        results.push(json!({
            "name": name,
            "ok": harness_ok,
            "program": program,
            "executable": found.as_ref().map(|p| p.display().to_string()),
            "model": model,
            "model_error": model_error.as_ref().map(|e| e.message.clone()),
            "shell_error": shell_error,
            "preview": preview,
        }));
        human.push_str(&format!(
            "{} {}\n",
            if harness_ok { "✓" } else { "✗" },
            h.name
        ));
        match found {
            Some(path) => human.push_str(&format!("  executable: {}\n", path.display())),
            None => human.push_str(&format!(
                "  executable: not found ({})\n  fix: install it or add its directory to PATH\n",
                program_name(&h)
            )),
        }
        if let Some(error) = model_error.as_ref() {
            human.push_str(&format!(
                "  model: {}\n  fix: {}\n",
                error.message,
                error
                    .hint
                    .as_deref()
                    .unwrap_or("configure model_flag or {{model}}")
            ));
        } else if let Some(model) = model {
            human.push_str(&format!("  model: {model} accepted\n"));
        }
        if let Some(error) = shell_error.as_ref() {
            human.push_str(&format!("  command: invalid shell syntax ({error})\n"));
        }
        human.push_str(&format!("  launch preview: {preview}\n"));
    }
    if !ok {
        human.push_str(
            "\nThe preview is not launched. tome starts harnesses inside its session backend.\n",
        );
    } else if results.is_empty() {
        human.push_str("No harnesses to validate.\n");
    }

    let data = json!({ "harnesses": results, "ok": ok });
    Ok(Report::new(data, human.trim_end()).with_exit(if ok { exit::OK } else { exit::INVALID }))
}

fn preview(h: &Harness, model: Option<&str>) -> String {
    let vars = Vars {
        prompt: "<Tome prompt>",
        prompt_file: "<prompt file>",
        run_id: RunId::new(1),
        session: "<session>",
        cwd: "<working directory>",
        model,
    };
    match h.command(&vars).as_slice() {
        [program, flag, command] if program == "sh" && flag == "-c" => command.clone(),
        argv => argv
            .iter()
            .map(|s| format!("{:?}", s))
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn program_name(h: &Harness) -> String {
    match &h.template {
        Template::Argv(args) => args
            .first()
            .cloned()
            .unwrap_or_else(|| "<empty command>".into()),
        Template::Shell(_) => "sh".into(),
    }
}

fn find_program(program: &str) -> Option<PathBuf> {
    let path = Path::new(program);
    if path.components().count() > 1 {
        return is_executable(path).then(|| path.to_path_buf());
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(program))
        .find(|p| is_executable(p))
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn shell_syntax_error(command: &str) -> Option<String> {
    let output = std::process::Command::new("sh")
        .args(["-n", "-c", command])
        .output()
        .ok()?;
    (!output.status.success()).then(|| String::from_utf8_lossy(&output.stderr).trim().to_string())
}
