//! Git worktrees for a run: `<repo>/.tome/worktrees/<run>-<name>` on a new
//! branch, `tome/<run>/<name>` unless the caller names one. tome only creates them and (in gc) removes
//! them; merging their branches is up to the orchestrator.

use crate::ids::RunId;
use crate::output::{CliError, CliResult};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Where worktrees live, relative to the repo root.
pub const DIR: &str = ".tome/worktrees";

/// The name a worktree at `path` was made under, if its directory is a
/// run's `<run>-<name>`.
pub fn name_of(path: &Path) -> Option<&str> {
    let file = path.file_name()?.to_str()?;
    let (run, name) = file.split_once('-')?;
    (!run.is_empty() && run.bytes().all(|b| b.is_ascii_digit()) && !name.is_empty()).then_some(name)
}

/// What a new worktree branches from.
#[derive(Debug, Clone)]
pub struct Base {
    /// The repository's top level.
    pub repo: PathBuf,
    /// The commit the branch starts at.
    pub commit: String,
    /// What was asked for (`--base`), or the branch HEAD was on (else the
    /// commit). gc checks a branch is merged into this before deleting it.
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct Created {
    pub path: PathBuf,
    pub branch: String,
}

/// Find the repo `dir` is in and resolve `base` (default: the HEAD commit;
/// uncommitted changes aren't carried over).
pub fn resolve_base(dir: &Path, base: Option<&str>) -> CliResult<Base> {
    let repo = git(dir, &["rev-parse", "--show-toplevel"]).map_err(|_| {
        CliError::invalid(format!("{} isn't inside a git repository", dir.display()))
            .with_hint("worktrees need the project to be a git repo")
    })?;
    let repo = PathBuf::from(repo.trim());
    let (commit, name) = match base {
        Some(b) => {
            let commit = git(
                &repo,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("{b}^{{commit}}"),
                ],
            )
            .map_err(|_| {
                CliError::invalid(format!("base `{b}` doesn't exist in {}", repo.display()))
            })?;
            (commit.trim().to_string(), b.to_string())
        }
        None => {
            let commit = git(
                &repo,
                &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"],
            )
            .map_err(|_| {
                CliError::invalid(format!("{} has no commits yet", repo.display()))
                    .with_hint("commit something first, or pass --base")
            })?;
            let commit = commit.trim().to_string();
            let name = git(&repo, &["symbolic-ref", "--short", "-q", "HEAD"])
                .map(|b| b.trim().to_string())
                .unwrap_or_default();
            let name = if name.is_empty() {
                commit.clone()
            } else {
                name
            };
            (commit, name)
        }
    };
    Ok(Base { repo, commit, name })
}

/// Create `<repo>/.tome/worktrees/<run>-<name>` on a new branch: `branch`,
/// or `tome/<run>/<name>`.
pub fn create(base: &Base, run_id: RunId, name: &str, branch: Option<&str>) -> CliResult<Created> {
    let path = base.repo.join(DIR).join(format!("{run_id}-{name}"));
    let branch = match branch {
        Some(b) => check_branch(&base.repo, b)?,
        None => format!("tome/{run_id}/{name}"),
    };
    if path.exists() {
        return Err(CliError::invalid(format!(
            "{} already exists",
            path.display()
        )));
    }
    if git(
        &base.repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_ok()
    {
        return Err(CliError::invalid(format!(
            "branch `{branch}` already exists in {}",
            base.repo.display()
        )));
    }
    ensure_ignored(&base.repo)?;
    let target = path.to_string_lossy();
    git(
        &base.repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            &branch,
            &target,
            &base.commit,
        ],
    )
    .map_err(|e| CliError::internal(format!("git couldn't create worktree {target}: {e}")))?;
    Ok(Created { path, branch })
}

/// `branch` if git accepts it as a new branch name.
fn check_branch(repo: &Path, branch: &str) -> CliResult<String> {
    match git(repo, &["check-ref-format", "--branch", branch]) {
        Ok(b) if b.trim() == branch => Ok(branch.to_string()),
        _ => Err(
            CliError::invalid(format!("`{branch}` isn't a valid branch name"))
                .with_hint("e.g. `feature/export-csv`; see `git check-ref-format --help`"),
        ),
    }
}

/// Add `.tome/worktrees/` to the repo's `.gitignore` unless it's already
/// ignored.
fn ensure_ignored(repo: &Path) -> CliResult<()> {
    let probe = format!("{DIR}/probe");
    let ignored = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["check-ignore", "-q", &probe])
        .status();
    if ignored.is_ok_and(|s| s.success()) {
        return Ok(());
    }
    let file = repo.join(".gitignore");
    let existing = fs::read_to_string(&file).unwrap_or_default();
    let mut out = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)?;
    if !existing.is_empty() && !existing.ends_with('\n') {
        writeln!(out)?;
    }
    writeln!(out, "{DIR}/")?;
    Ok(())
}

/// Whether `branch` is merged into `base` (`None` if git can't tell, e.g.
/// one of them is gone).
pub fn merged(repo: &Path, branch: &str, base: &str) -> Option<bool> {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["merge-base", "--is-ancestor", branch, base])
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?;
    match status.code() {
        Some(0) => Some(true),
        Some(1) => Some(false),
        _ => None,
    }
}

/// Run git in `dir`; stdout, or stderr as the error.
pub fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| format!("running git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().canonicalize().unwrap().join("r");
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]).unwrap();
        git(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ],
        )
        .unwrap();
        (dir, repo)
    }

    #[test]
    fn creates_an_ignored_worktree_from_head() {
        let (_d, repo) = repo();
        fs::write(repo.join(".gitignore"), "target").unwrap();
        fs::write(repo.join("dirty.txt"), "uncommitted").unwrap();
        let base = resolve_base(&repo, None).unwrap();
        assert_eq!(base.name, "main");
        let wt = create(&base, RunId::new(3), "a", None).unwrap();
        assert_eq!(wt.path, repo.join(".tome/worktrees/3-a"));
        assert_eq!(wt.branch, "tome/3/a");
        assert!(wt.path.join(".git").is_file());
        assert!(
            !wt.path.join("dirty.txt").exists(),
            "branches from the HEAD commit"
        );
        assert_eq!(
            fs::read_to_string(repo.join(".gitignore")).unwrap(),
            "target\n.tome/worktrees/\n"
        );
        // Already ignored: not added twice; the name is taken.
        create(&base, RunId::new(3), "b", None).unwrap();
        assert_eq!(
            fs::read_to_string(repo.join(".gitignore"))
                .unwrap()
                .matches(".tome")
                .count(),
            1
        );
        assert!(create(&base, RunId::new(3), "a", None).is_err());
        assert_eq!(merged(&repo, "tome/3/a", "main"), Some(true));
    }

    #[test]
    fn a_named_branch_is_used_and_checked() {
        let (_d, repo) = repo();
        let base = resolve_base(&repo, None).unwrap();
        let wt = create(
            &base,
            RunId::new(4),
            "t004-renderer",
            Some("feature/renderer"),
        )
        .unwrap();
        assert_eq!(wt.branch, "feature/renderer");
        assert_eq!(wt.path, repo.join(".tome/worktrees/4-t004-renderer"));
        assert_eq!(merged(&repo, "feature/renderer", "main"), Some(true));
        // Taken, or not a branch name git accepts: nothing is created.
        assert!(create(&base, RunId::new(4), "other", Some("feature/renderer")).is_err());
        let err = create(&base, RunId::new(4), "bad", Some("has space..")).unwrap_err();
        assert_eq!(err.kind, crate::output::ErrorKind::Invalid);
        assert!(!repo.join(".tome/worktrees/4-bad").exists());
    }

    #[test]
    fn bad_base_and_non_repos_are_invalid() {
        let (d, repo) = repo();
        let err = resolve_base(&repo, Some("nope")).unwrap_err();
        assert_eq!(err.kind, crate::output::ErrorKind::Invalid);
        let outside = d.path().join("plain");
        fs::create_dir_all(&outside).unwrap();
        assert_eq!(
            resolve_base(&outside, None).unwrap_err().kind,
            crate::output::ErrorKind::Invalid
        );
    }

    #[test]
    fn names_come_from_the_directory() {
        assert_eq!(name_of(Path::new("/r/.tome/worktrees/12-api")), Some("api"));
        assert_eq!(name_of(Path::new("/r/.tome/worktrees/12-a-b")), Some("a-b"));
        for path in ["/r/x/api", "/r/x/12-", "/r/x/-api", "/r/x/v1-api"] {
            assert_eq!(name_of(Path::new(path)), None, "{path}");
        }
    }
}
