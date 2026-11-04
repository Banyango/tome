# Remove a run — Task Plan

Source feature: [feature.md](./feature.md)

## Tasks

### 022-1. Remove one finished run

**Blocked by:** none

`tome runs rm <id> [--dry-run]` removes one finished run: its record, log directory, worktrees and merged tome branches, reusing gc's per-run deletion, and works with `--on <node>`. Unmerged branches are kept and listed. An unknown id is an error with a non-zero exit. If git can't remove a worktree, the run is kept and the git error is reported with a non-zero exit, so rm can be run again. `--dry-run` prints what would be removed and changes nothing. Only the given run is removed, even in a resume chain: worktrees adopted by a newer run in the chain stay with that run. Following gc's ownership rule, removing the newest run that holds an adopted worktree removes that worktree too, even though an older run still records it. User docs cover the new command.

### 022-2. Force-delete unmerged branches

**Blocked by:** 022-1

`tome runs rm <id> --force` also deletes the run's unmerged `tome/<run>/…` branches, so a run's work can be thrown away completely. With `--dry-run`, the output shows those branches as ones that would be deleted, and nothing changes.

### 022-3. Cancel and remove a live run

**Blocked by:** 022-1

`tome runs rm <id>` on a queued or running run cancels it first, closing its sessions, and then removes it. If the cancel fails, nothing is removed and the error is reported. With `--dry-run`, the output says the run would be cancelled and removed, and neither happens.
