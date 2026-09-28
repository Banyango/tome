# Session placement — Follow-ups

Source feature: [feature.md](./feature.md) · Tasks and what changed: [tasks.md](./tasks.md)

Gaps and rough edges found while implementing 010-1 to 010-7 (2026-09-28). None of them block the feature; each is a candidate for a later task.

- **A move resets `created_at`:** `tome session move` stores the session with `INSERT OR REPLACE`, which stamps `created_at` with the move time. `tome runs show` then lists the moved session as newest, and `from: last` can pick it. Keep the original time on a move.
- **The tmux move to `own` takes no workspace lock:** the tab and split moves hold `workspace::lock()`, but the own move makes its tmux session outside it. Two concurrent moves of sessions with the same name could race on the session name.
- **Tab to tab in the same workspace:** this isn't a no-op the way own to own is. On tmux the pane is broken into a new window of the same session; on cmux it moves into the tabs pane again. Consider skipping a move whose resolved placement matches the current one.
- **`focused` on tmux is a guess:** it picks the most recently active client, so with several clients attached it may not be the one the user is looking at.
- **cmux `split.size` on unseen workspaces:** it's skipped, with a warning, when cmux has no geometry for a workspace it hasn't shown. A retry after the workspace is first shown would make it stick.
- **Named workspaces pile up in `workspaces.json`:** each `<project>-<name>` gets a record that is only replaced when its workspace is gone, never removed. `tome gc` could prune records whose workspaces no longer exist.
- **Fallbacks during a move are session warnings only:** a `focused` or `from` fallback during `tome session move` goes on the session's placement warnings, not on the run's notes the way a `focused` fallback at run request does.
- **Untested tmux split edge:** joining a pane as a split when it's the only pane of the target's first window isn't covered by `tests/layout.rs`.
