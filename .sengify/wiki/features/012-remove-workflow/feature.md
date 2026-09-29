# Remove a workflow

## Description

`tome workflow rm <name|path> [--global] [--force]` deletes a [[workflow]] file. It sits next to `tome workflow new` and uses the same scope rules: it works on the project's `.tome/workflows` by default, or on `~/.tome/workflows` with `--global`, and it never crosses scopes. `<name>` is a frontmatter `name` in the chosen scope. A path to a `.md` file inside a workflows dir also works, which is how you delete a file too broken to have a readable name.

It doesn't prompt for confirmation, because agents are the main callers. For project workflows, git is the undo. It deletes the file only, and leaves runs, logs, worktrees and directories alone.

Before deleting, it asks the [[daemon]] whether that exact file has [[run]]s that are running or queued, matching on the run's `workflow_path` so a project `foo` and a global `foo` aren't mixed up. If it does, `rm` refuses. `--force` skips the check. Any runs that are running or queued carry on from their saved workflow snapshots. Once the file is gone, the daemon's re-arm scan drops its [[trigger]]s. Past runs are untouched, and `tome runs show` still has their snapshots.

## Use Cases

1. As a human author, I want to remove a project workflow I no longer need, so that it stops showing up in `tome run`, validation and triggers.
2. As a human author, I want to remove a global workflow with `--global`, so that I can clean up my shared library without touching project copies.
3. As an agent, I want a non-interactive command with JSON output, so that I can remove a workflow during a run without a human.
4. As an author, I want to remove a workflow by file path, so that I can delete a broken file whose frontmatter can't be read.
5. As an author, I want `rm` to refuse while the workflow has running or queued runs, so that I don't pull a definition out from under live work by accident.
6. As an author, I want `--force` to delete anyway, so that I can remove the file when I know the runs are fine or the daemon is down.

## Constraints

- It mirrors `tome workflow new`: same scope flag and the same report shape (JSON `{name, path, scope, ...}` plus human text).
- It deletes the workflow file only: no runs, logs, worktrees, or empty directories.
- The live-run check goes through the daemon, since only the daemon opens the run store.
- It must not break `tome runs show` or queued runs, which rely on the run's workflow snapshot.
- Out of scope: `tome workflow ls`, trash/restore, disabling a workflow, and cancelling runs as part of removal.

## Edge Cases

- No workflow by that name in the chosen scope → error. If the name exists in the other scope, the hint says so (e.g. "pass --global").
- Two files in the scope declare the same `name` → refuse, list both paths, and hint to pass the path of the one to delete.
- The path isn't a `.md` inside a project or global workflows dir → refuse.
- A path given with a conflicting `--global` → error. The path's own location decides the scope.
- The file has running or queued runs → refuse, list the run ids, and hint `--force` or `tome run cancel`.
- Daemon not running → refuse ("can't check for live runs: daemon is not running"), with a hint to start it or pass `--force`.
- `--force` with live runs → delete. The runs continue from their snapshots.
- Removing a project workflow that shadowed a global one → succeed, with a note that `tome run <name>` now uses the global workflow at <path>.
- The file is deleted while the daemon is up → its triggers disarm on the next re-arm scan.

## Related Entities

- [[workflow]]
- [[run]]
- [[daemon]]
- [[trigger]]
