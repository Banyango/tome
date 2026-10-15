//! `tome plugin install|update|rm|enable|disable|ls`: manage the plugins in
//! `~/.tome/plugins`.
//!
//! These only touch files on this machine (plugins are per machine, so they
//! refuse `--on`) and never run plugin code: an install validates the
//! manifest and copies or clones the folder, nothing more.

use crate::node;
use crate::output::{exit, table, CliError, CliResult, Report};
use crate::paths;
use crate::plugin::manifest::{self, Claim, Conflict, Incompatible, Manifest};
use crate::plugin::store::{self, InstallState, Installed};
use crate::workflow::{self, Scope};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

// ---------------------------------------------------------------------------
// Where a plugin comes from

#[derive(Debug, Clone, PartialEq, Eq)]
enum Source {
    Path(PathBuf),
    Git {
        url: String,
        reference: Option<String>,
    },
}

/// Transports a git install may use. `ext::` and friends run commands, so
/// they never get through.
const GIT_SCHEMES: &[&str] = &["https", "http", "ssh", "git", "file"];

impl Source {
    /// A git URL (`https://…`, `git@host:path`, or a `….git` that isn't a
    /// local path), optionally with `#ref`; anything else is a local folder.
    fn parse(arg: &str, exists: impl Fn(&Path) -> bool) -> Result<Source, String> {
        let arg = arg.trim();
        if arg.is_empty() {
            return Err("give a path or a git URL".into());
        }
        let (base, reference) = match arg.rsplit_once('#') {
            Some((base, r)) if !r.is_empty() && looks_like_git(base) => (base, Some(r)),
            _ => (arg, None),
        };
        let git = looks_like_git(base) || (base.ends_with(".git") && !exists(Path::new(arg)));
        if !git {
            return Ok(Source::Path(node::expand_home(arg)));
        }
        if base.starts_with('-') {
            return Err(format!("`{base}` isn't a git URL"));
        }
        if let Some((scheme, _)) = base.split_once("://") {
            if !GIT_SCHEMES.contains(&scheme) {
                return Err(format!(
                    "unsupported git URL scheme `{scheme}` (use {})",
                    GIT_SCHEMES.join(", ")
                ));
            }
        } else if base.contains("::") {
            return Err(format!("unsupported git URL `{base}`"));
        }
        if reference.is_some_and(|r| r.starts_with('-')) {
            return Err("the `#ref` can't start with `-`".into());
        }
        Ok(Source::Git {
            url: base.to_string(),
            reference: reference.map(str::to_string),
        })
    }
}

fn looks_like_git(s: &str) -> bool {
    if s.contains("://") || s.starts_with("git@") {
        return true;
    }
    // scp-style: user@host:path
    s.split_once(':')
        .is_some_and(|(head, _)| head.contains('@') && !head.contains('/'))
}

// ---------------------------------------------------------------------------
// Files and git (the edges)

/// A scratch folder inside `~/.tome/plugins` (so the final rename stays on
/// one filesystem), removed when dropped unless it was moved into place.
struct Staging(PathBuf);

impl Staging {
    fn new(plugins: &Path) -> CliResult<Staging> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        Ok(Staging(
            plugins.join(format!(".staging-{}-{nanos}", std::process::id())),
        ))
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn io_err(what: &str, path: &Path, e: std::io::Error) -> CliError {
    CliError::internal(format!("{what} {}: {e}", path.display()))
}

/// Copy a folder, keeping symlinks as symlinks and leaving `.git` behind.
fn copy_dir(from: &Path, to: &Path) -> CliResult<()> {
    fs::create_dir_all(to).map_err(|e| io_err("creating", to, e))?;
    for entry in fs::read_dir(from).map_err(|e| io_err("reading", from, e))? {
        let entry = entry.map_err(|e| io_err("reading", from, e))?;
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        let (src, dst) = (entry.path(), to.join(&name));
        let kind = entry.file_type().map_err(|e| io_err("reading", &src, e))?;
        if kind.is_symlink() {
            let target = fs::read_link(&src).map_err(|e| io_err("reading", &src, e))?;
            std::os::unix::fs::symlink(&target, &dst).map_err(|e| io_err("linking", &dst, e))?;
        } else if kind.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            fs::copy(&src, &dst).map_err(|e| io_err("copying", &src, e))?;
        }
    }
    Ok(())
}

/// Run `git`, returning its trimmed stdout, or its stderr on failure.
fn git(dir: Option<&Path>, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    if let Some(dir) = dir {
        cmd.arg("-C").arg(dir);
    }
    let out = cmd
        .args(args)
        .env("GIT_ALLOW_PROTOCOL", GIT_SCHEMES.join(":"))
        .output()
        .map_err(|e| format!("couldn't run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.trim();
        Err(if err.is_empty() {
            format!("git {} failed", args.first().unwrap_or(&""))
        } else {
            err.to_string()
        })
    }
}

// ---------------------------------------------------------------------------
// Validation

/// Everything about a plugin folder that can be checked without running it.
fn check_folder(dir: &Path) -> Result<Manifest, CliError> {
    let manifest = store::read_manifest(dir).map_err(|e| {
        CliError::invalid(format!("invalid plugin: {e}"))
            .with_hint("a plugin is a folder with a plugin.yaml manifest; see the plugins guide")
    })?;
    if let Err(incompatible) = manifest::compatibility(&manifest, &store::tome_version()) {
        return Err(incompatible_error(&manifest.name, &incompatible));
    }
    check_files(dir, &manifest).map_err(CliError::invalid)?;
    Ok(manifest)
}

fn incompatible_error(name: &str, incompatible: &Incompatible) -> CliError {
    CliError::invalid(format!(
        "plugin `{name}` is incompatible: {}",
        incompatible.message()
    ))
    .with_hint(incompatible.hint(name))
}

/// The files a manifest names exist, stay inside the folder, and are usable.
fn check_files(dir: &Path, m: &Manifest) -> Result<(), String> {
    let root = dir
        .canonicalize()
        .map_err(|e| format!("reading {}: {e}", dir.display()))?;
    let inside = |path: &Path, what: &str| -> Result<PathBuf, String> {
        let real = path
            .canonicalize()
            .map_err(|_| format!("{what}: {} doesn't exist", path.display()))?;
        if !real.starts_with(&root) {
            return Err(format!(
                "{what}: {} points outside the plugin folder",
                path.display()
            ));
        }
        if !real.is_file() {
            return Err(format!("{what}: {} isn't a file", path.display()));
        }
        Ok(real)
    };
    for (cmd, rel) in &m.subcommands {
        let what = format!("subcommand `{cmd}`");
        let real = inside(&dir.join(rel), &what)?;
        let mode = {
            use std::os::unix::fs::PermissionsExt;
            fs::metadata(&real)
                .map(|m| m.permissions().mode())
                .unwrap_or(0)
        };
        if mode & 0o111 == 0 {
            return Err(format!(
                "{what}: {} isn't executable (chmod +x it)",
                rel.display()
            ));
        }
    }
    for name in &m.workflows {
        let path = dir.join("workflows").join(format!("{name}.md"));
        let real = inside(&path, &format!("workflow `{name}`"))?;
        let wf = workflow::load(&real)
            .and_then(|wf| workflow::check_scope(wf, Scope::Global))
            .map_err(|inv| {
                format!(
                    "workflow `{name}` is invalid:\n{}",
                    inv.to_string().trim_end()
                )
            })?;
        if wf.name() != name {
            return Err(format!(
                "workflow `{name}`: workflows/{name}.md is named `{}` in its frontmatter",
                wf.name()
            ));
        }
    }
    Ok(())
}

fn conflict_error(subject: &str, conflicts: &[Conflict]) -> CliError {
    let lines: Vec<String> = conflicts.iter().map(Conflict::message).collect();
    CliError::invalid(format!("{subject}:\n  {}", lines.join("\n  ")))
        .with_hint("rename the clashing name in the plugin's plugin.yaml, or remove the other one")
        .with_details(
            json!({ "conflicts": conflicts.iter().map(conflict_json).collect::<Vec<_>>() }),
        )
}

fn conflict_json(c: &Conflict) -> Value {
    json!({ "kind": c.space.as_str(), "name": c.name, "ours": c.ours, "theirs": c.theirs })
}

// ---------------------------------------------------------------------------
// install

pub fn install(source: &str, builtin: &[String]) -> CliResult<Report> {
    let source_arg = source.trim().to_string();
    let source = Source::parse(&source_arg, Path::exists).map_err(CliError::invalid)?;
    let plugins = paths::plugins_dir();
    fs::create_dir_all(&plugins).map_err(|e| io_err("creating", &plugins, e))?;
    let staging = Staging::new(&plugins)?;

    let state = match &source {
        Source::Path(path) => {
            if !path.is_dir() {
                return Err(
                    CliError::invalid(format!("{} isn't a folder", path.display()))
                        .with_hint("pass a plugin folder, or a git URL (https://…, git@host:path)"),
                );
            }
            copy_dir(path, &staging.0)?;
            InstallState {
                source: source_arg,
                git: false,
                reference: None,
                enabled: true,
            }
        }
        Source::Git { url, reference } => {
            let target = staging.0.to_string_lossy().into_owned();
            git(None, &["clone", "--quiet", "--", url, &target])
                .map_err(|e| CliError::invalid(format!("cloning {url}: {e}")))?;
            if let Some(r) = reference {
                git(Some(&staging.0), &["checkout", "--quiet", r])
                    .map_err(|e| CliError::invalid(format!("checking out `{r}`: {e}")))?;
            }
            InstallState {
                source: source_arg,
                git: true,
                reference: reference.clone(),
                enabled: true,
            }
        }
    };

    let manifest = check_folder(&staging.0)?;
    let dest = plugins.join(&manifest.name);
    if dest.exists() {
        return Err(CliError::invalid(format!(
            "a plugin named `{}` is already installed",
            manifest.name
        ))
        .with_hint(format!(
            "update it with `tome plugin update {0}`, or remove it first with `tome plugin rm {0}`",
            manifest.name
        )));
    }
    let mine = manifest.claims(&format!("plugin `{}`", manifest.name));
    let others = store::claims_except(builtin, None)?;
    let clashes = store::conflicts(&mine, &others);
    if !clashes.is_empty() {
        return Err(conflict_error(
            &format!("can't install plugin `{}`", manifest.name),
            &clashes,
        ));
    }

    state.write(&staging.0)?;
    fs::rename(&staging.0, &dest).map_err(|e| io_err("installing into", &dest, e))?;

    let mut human = format!(
        "installed plugin `{}`{} from {}",
        manifest.name,
        manifest
            .version
            .as_ref()
            .map(|v| format!(" {v}"))
            .unwrap_or_default(),
        state.source
    );
    for (label, items) in provides(&manifest) {
        if !items.is_empty() {
            human.push_str(&format!("\n  {label}: {}", items.join(", ")));
        }
    }
    Ok(Report::new(
        json!({
            "name": manifest.name,
            "version": manifest.version,
            "dir": dest,
            "source": state.source,
        }),
        human,
    ))
}

/// What a plugin adds, as (label, names) rows.
fn provides(m: &Manifest) -> [(&'static str, Vec<String>); 5] {
    [
        ("subcommands", m.subcommands.keys().cloned().collect()),
        (
            "hooks",
            m.hook_names().into_iter().map(String::from).collect(),
        ),
        ("providers", m.providers.clone()),
        ("workflows", m.workflows.clone()),
        ("harnesses", m.harnesses.keys().cloned().collect()),
    ]
}

// ---------------------------------------------------------------------------
// lookup, rm, enable, disable

fn locate(name: &str) -> CliResult<Installed> {
    if manifest::check_ident(name, "plugin name").is_err()
        || !paths::plugins_dir().join(name).is_dir()
    {
        return Err(
            CliError::not_found(format!("no plugin `{name}` is installed"))
                .with_hint("`tome plugin ls` lists the installed plugins"),
        );
    }
    Ok(Installed::load(name))
}

pub fn rm(name: &str) -> CliResult<Report> {
    let plugin = locate(name)?;
    fs::remove_dir_all(&plugin.dir).map_err(|e| io_err("removing", &plugin.dir, e))?;
    Ok(Report::new(
        json!({ "name": name, "removed": true }),
        format!("removed plugin `{name}`"),
    ))
}

pub fn set_enabled(name: &str, enabled: bool) -> CliResult<Report> {
    let mut plugin = locate(name)?;
    if enabled {
        if let Err(e) = &plugin.manifest {
            return Err(
                CliError::invalid(format!("plugin `{name}` is invalid: {e}"))
                    .with_hint(format!("fix it, or `tome plugin update {name}`")),
            );
        }
    }
    let changed = plugin.state.enabled != enabled;
    plugin.state.enabled = enabled;
    plugin.state.write(&plugin.dir)?;
    let word = if enabled { "enabled" } else { "disabled" };
    Ok(Report::new(
        json!({ "name": name, "enabled": enabled, "changed": changed }),
        if changed {
            format!("{word} plugin `{name}`")
        } else {
            format!("plugin `{name}` is already {word}")
        },
    ))
}

// ---------------------------------------------------------------------------
// update

enum Updated {
    Skipped(&'static str),
    UpToDate,
    Updated { from: String, to: String },
}

/// Pull a git install, keeping the old checkout if the new one doesn't hold
/// up.
fn update_one(plugin: &Installed, builtin: &[String]) -> Result<Updated, String> {
    if !plugin.state.git {
        return Ok(Updated::Skipped(
            "installed from a path; `tome plugin rm` and install it again to pick up changes",
        ));
    }
    let dir = Some(plugin.dir.as_path());
    let old = git(dir, &["rev-parse", "HEAD"])?;
    git(dir, &["fetch", "--quiet", "--tags", "--force", "origin"])?;
    match &plugin.state.reference {
        None => {
            git(dir, &["merge", "--quiet", "--ff-only", "@{u}"])?;
        }
        Some(r) => {
            let remote = format!("origin/{r}");
            let target = if git(
                dir,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("{remote}^{{commit}}"),
                ],
            )
            .is_ok()
            {
                remote
            } else {
                r.clone()
            };
            git(dir, &["checkout", "--quiet", "--detach", &target])?;
        }
    }
    let new = git(dir, &["rev-parse", "HEAD"])?;
    if new == old {
        return Ok(Updated::UpToDate);
    }

    let verdict = check_folder(&plugin.dir)
        .map_err(|e| e.message)
        .and_then(|m| {
            if m.name != plugin.name {
                return Err(format!("the update renames the plugin to `{}`", m.name));
            }
            let mine = m.claims(&plugin.source_label());
            let others =
                store::claims_except(builtin, Some(&plugin.name)).map_err(|e| e.message)?;
            let clashes = store::conflicts(&mine, &others);
            if clashes.is_empty() {
                Ok(m)
            } else {
                Err(clashes
                    .iter()
                    .map(Conflict::message)
                    .collect::<Vec<_>>()
                    .join("; "))
            }
        });
    match verdict {
        Ok(_) => Ok(Updated::Updated {
            from: old[..old.len().min(8)].to_string(),
            to: new[..new.len().min(8)].to_string(),
        }),
        Err(reason) => {
            git(dir, &["reset", "--quiet", "--hard", &old])?;
            Err(format!("the update was rolled back: {reason}"))
        }
    }
}

pub fn update(name: Option<&str>, builtin: &[String]) -> CliResult<Report> {
    let targets = match name {
        Some(name) => vec![locate(name)?],
        None => store::installed(),
    };
    let mut rows = Vec::new();
    let mut data = Vec::new();
    let mut failed = false;
    for plugin in &targets {
        let (status, detail) = match update_one(plugin, builtin) {
            Ok(Updated::Skipped(why)) => ("skipped", why.to_string()),
            Ok(Updated::UpToDate) => ("up-to-date", String::new()),
            Ok(Updated::Updated { from, to }) => ("updated", format!("{from} → {to}")),
            Err(e) => {
                failed = true;
                ("failed", e)
            }
        };
        data.push(json!({ "name": plugin.name, "status": status, "detail": detail }));
        rows.push(vec![plugin.name.clone(), status.to_string(), detail]);
    }
    let human = if rows.is_empty() {
        "no plugins installed".to_string()
    } else {
        table(&["NAME", "RESULT", "DETAIL"], rows)
    };
    let report = Report::new(json!({ "plugins": data }), human);
    Ok(if failed {
        report.with_exit(exit::FAILURE)
    } else {
        report
    })
}

// ---------------------------------------------------------------------------
// ls

/// A plugin's state as `tome plugin ls` shows it.
fn state_of(plugin: &Installed, incompatible: &Option<Incompatible>) -> &'static str {
    match (&plugin.manifest, plugin.state.enabled) {
        (_, false) => "disabled",
        (Err(_), _) => "invalid",
        (Ok(_), _) if incompatible.is_some() => "incompatible",
        (Ok(_), true) => "enabled",
    }
}

pub fn ls(builtin: &[String]) -> CliResult<Report> {
    let plugins = store::installed();
    let user = store::user_claims(builtin)?;
    let by_plugin: Vec<Vec<Claim>> = plugins.iter().map(Installed::claims).collect();

    let mut rows = Vec::new();
    let mut notes = Vec::new();
    let mut data = Vec::new();
    for (i, plugin) in plugins.iter().enumerate() {
        let incompatible = plugin
            .manifest
            .as_ref()
            .ok()
            .and_then(|m| manifest::compatibility(m, &store::tome_version()).err());
        let mut others = user.clone();
        for (j, claims) in by_plugin.iter().enumerate() {
            if j != i {
                others.extend(claims.iter().cloned());
            }
        }
        let clashes = store::conflicts(&by_plugin[i], &others);
        let state = state_of(plugin, &incompatible);

        let m = plugin.manifest.as_ref().ok();
        rows.push(vec![
            plugin.name.clone(),
            state.to_string(),
            m.and_then(|m| m.version.clone()).unwrap_or_default(),
            m.map(|m| m.hook_names().join(", ")).unwrap_or_default(),
        ]);
        if let Err(e) = &plugin.manifest {
            notes.push(format!("{}: invalid: {e}", plugin.name));
        }
        if let Some(i) = &incompatible {
            notes.push(format!(
                "{}: {} ({})",
                plugin.name,
                i.message(),
                i.hint(&plugin.name)
            ));
        }
        for c in &clashes {
            notes.push(format!("{}: conflict: {}", plugin.name, c.message()));
        }

        data.push(json!({
            "name": plugin.name,
            "state": state,
            "enabled": plugin.state.enabled,
            "version": m.and_then(|m| m.version.clone()),
            "description": m.and_then(|m| m.description.clone()),
            "protocol": m.map(|m| m.protocol),
            "hooks": m.map(|m| m.hook_names()).unwrap_or_default(),
            "subcommands": m.map(|m| m.subcommands.keys().cloned().collect::<Vec<_>>()).unwrap_or_default(),
            "providers": m.map(|m| m.providers.clone()).unwrap_or_default(),
            "workflows": m.map(|m| m.workflows.clone()).unwrap_or_default(),
            "harnesses": m.map(|m| m.harnesses.keys().cloned().collect::<Vec<_>>()).unwrap_or_default(),
            "source": plugin.state.source,
            "git": plugin.state.git,
            "ref": plugin.state.reference,
            "dir": plugin.dir,
            "error": plugin.manifest.as_ref().err(),
            "incompatible": incompatible.as_ref().map(|i| json!({
                "message": i.message(),
                "hint": i.hint(&plugin.name),
            })),
            "conflicts": clashes.iter().map(conflict_json).collect::<Vec<_>>(),
        }));
    }

    let human = if rows.is_empty() {
        "no plugins installed (`tome plugin install <path|git-url>` adds one)".to_string()
    } else {
        let mut out = table(&["NAME", "STATE", "VERSION", "HOOKS"], rows);
        if !notes.is_empty() {
            out.push('\n');
            for note in &notes {
                out.push_str(note);
                out.push('\n');
            }
        }
        out.trim_end().to_string()
    };
    Ok(Report::new(json!({ "plugins": data }), human))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<Source, String> {
        Source::parse(s, |_| false)
    }

    #[test]
    fn paths_and_git_urls_are_told_apart() {
        assert_eq!(
            parse("./plugins/x").unwrap(),
            Source::Path(PathBuf::from("./plugins/x"))
        );
        assert_eq!(
            parse("https://github.com/a/b").unwrap(),
            Source::Git {
                url: "https://github.com/a/b".into(),
                reference: None
            }
        );
        assert_eq!(
            parse("git@github.com:a/b.git#v1.2").unwrap(),
            Source::Git {
                url: "git@github.com:a/b.git".into(),
                reference: Some("v1.2".into())
            }
        );
        // A `#` in a local path is part of the path.
        assert_eq!(
            parse("./a#b").unwrap(),
            Source::Path(PathBuf::from("./a#b"))
        );
        // `.git` is a repo unless a local path by that name exists.
        assert!(matches!(parse("repos/x.git").unwrap(), Source::Git { .. }));
        assert!(matches!(
            Source::parse("repos/x.git", |_| true).unwrap(),
            Source::Path(_)
        ));
    }

    #[test]
    fn risky_sources_are_refused() {
        assert!(matches!(parse("ext::sh -c evil"), Ok(Source::Path(_))));
        assert!(parse("--upload-pack=evil.git")
            .unwrap_err()
            .contains("isn't a git URL"));
        assert!(parse("ftp://host/x")
            .unwrap_err()
            .contains("unsupported git URL scheme"));
        assert!(parse("https://h/x#--upload-pack=evil")
            .unwrap_err()
            .contains("can't start with `-`"));
        assert!(parse("  ").is_err());
    }
}
