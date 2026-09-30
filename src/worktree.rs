//! Git worktrees for a run: `<repo>/.tome/worktrees/<run>-<name>` on a new
//! branch `tome/<run>/<name>`. tome only creates them and (in gc) removes
//! them; merging their branches is up to the orchestrator.

use crate::output::{CliError, CliResult};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Where worktrees live, relative to the repo root.
pub const DIR: &str = ".tome/worktrees";

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

/// Create `<repo>/.tome/worktrees/<run>-<name>` on branch `tome/<run>/<name>`.
pub fn create(base: &Base, run_id: i64, name: &str) -> CliResult<Created> {
    let path = base.repo.join(DIR).join(format!("{run_id}-{name}"));
    let branch = format!("tome/{run_id}/{name}");
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
        let wt = create(&base, 3, "a").unwrap();
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
        create(&base, 3, "b").unwrap();
        assert_eq!(
            fs::read_to_string(repo.join(".gitignore"))
                .unwrap()
                .matches(".tome")
                .count(),
            1
        );
        assert!(create(&base, 3, "a").is_err());
        assert_eq!(merged(&repo, "tome/3/a", "main"), Some(true));
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
}
