# Remove a workflow — Task Plan

Source feature: [feature.md](./feature.md)

## Tasks

### 012-1. `tome workflow rm`: find and delete the file

**Blocked by:** none

`tome workflow rm <name|path> [--global]` sits next to `tome workflow new` and uses the same scope rules and the same report shape (JSON `{name, path, scope, ...}` plus human text). With a name, it looks for a workflow whose frontmatter `name` matches in the chosen scope (project by default, global with `--global`), and it never crosses scopes. With a path, the path must be a `.md` inside a project or global workflows dir, and the path's own location decides the scope. This is how you remove a file too broken to have a readable name. It deletes only the workflow file and leaves runs, logs, worktrees and directories alone. It never prompts.

It errors in these cases:
- **Not found:** no workflow by that name in the chosen scope. If the name exists in the other scope, the hint says so (e.g. "pass --global").
- **Duplicate name:** two files in the scope declare the same `name`. It refuses, lists both paths, and hints to pass the path of the one to delete.
- **Bad path:** the path isn't a `.md` inside a workflows dir.
- **Conflicting `--global`:** a path is given together with a `--global` that contradicts where the path is.

Removing a project workflow that shadowed a global one succeeds, with a note that `tome run <name>` now uses the global workflow at its path.

It deletes without checking for live runs until 012-2 lands. (UC 1–4)

### 012-2. Refuse while runs are live, with `--force` to override

**Blocked by:** 012-1

Before deleting, `rm` asks the [[daemon]] whether that exact file has [[run]]s that are running or queued. It matches on the run's `workflow_path`, so a project `foo` and a global `foo` aren't mixed up. The daemon has to answer this, since only the daemon opens the run store.
- **Live runs:** it refuses, lists the run ids, and hints `--force` or `tome run cancel`.
- **Daemon not running:** it refuses ("can't check for live runs: daemon is not running"), with a hint to start it or pass `--force`.
- **`--force`:** it skips the check and deletes.

This task also covers what happens after a delete:
- Running and queued runs carry on from their saved workflow snapshots.
- `tome runs show` still shows past runs' snapshots.
- The file's [[trigger]]s disarm on the daemon's next re-arm scan.

(UC 5–6)
