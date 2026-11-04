# Remove a run

## Description

`tome runs rm <id>` deletes one [[run]] and everything tome made for it: the run record, its log directory, its [[worktree]]s and its merged tome branches. It removes the same things `tome gc` removes for each run, but it targets a single run by id instead of using an age cutoff.

Branches named `tome/<run>/…` that aren't merged are kept and listed. `--force` deletes them as well. If the run is queued or running, rm cancels it first, closing its [[session]]s, and then removes it. Only the given run is removed, even when it's part of a resume chain. It takes a single id, doesn't ask for confirmation, and supports `--dry-run`. The command is listed in `reach` so `--on <node>` works.

## Use Cases

1. As a user, I want to remove one finished run along with its logs, worktrees and merged branches, so that I can clean up a specific run without waiting for a `gc` age cutoff.
2. As a user, I want `--force` to also delete unmerged tome branches, so that I can throw away a run's work completely.
3. As a user, I want rm to cancel a run that's still live and then remove it, so that one command gets rid of a run I no longer want.
4. As a user, I want `--dry-run`, so that I can see what would be removed first.
5. As a user, I want to remove one run in a resume chain without touching the others, so that the runs that came after it keep their state.

## Constraints

- Worktrees are removed only through `git worktree remove --force`, never by deleting a directory tree directly (same as gc).
- Reuse gc's per-run deletion instead of duplicating it.
- One id per call, with no interactive prompt.
- New RPC method listed in the owning module's `METHODS` and routed in `Engine::dispatch`. The command is added to `reach` in `main.rs`.

## Edge Cases

- Unknown id → error, non-zero exit.
- git can't remove a worktree → the run is kept, the git error is reported, non-zero exit; rm can be run again after the fix.
- Unmerged tome branch without `--force` → the branch is kept and listed with a note.
- Run is queued or running → cancel it (closing its sessions), then remove it. If the cancel fails, nothing is removed and the error is reported.
- Run is part of a resume chain → only that run is removed. Worktrees adopted by a newer run in the chain stay with that run.
- `--dry-run` → print what would be removed and change nothing (no cancel either).

## Related Entities

- [[run]]
- [[worktree]]
- [[session]]
- [[daemon]]
