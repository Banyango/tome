# Orchestrator next to the caller — Task Plan

Source feature: [feature.md](./feature.md)

> All tasks are done: implemented as 011-1 to 011-4 directly on main (f9b2098, a5e74c7, 590370b, 0491034). Follow-ups: [follow-ups.md](./follow-ups.md).

## What changed

- **Placement:** `from` takes `caller`. `tome validate` and the workflow load reject it in a `workers[]` rule or the `workers` block, and `tome worker spawn --from caller` is refused with a hint. At levels both roles share, workers skip it. Each session's placement JSON can carry `caller`, the pane it opened next to.
- **Records:** each run's placement JSON has `caller`. That's `{surface, workspace, pane}` for a `tome run` from a cmux pane, or `{unknown: why}` for a trigger or a run from outside cmux. No store migration was needed.
- **Launch:** on cmux, the orchestrator opens in the caller's workspace, as a tab in its pane (`new-surface --pane`) or a split off it (`new-split --surface`), without focus. With no caller, on tmux, or when the caller's pane is gone, it opens where it would without `from`, with a session warning.
- **`tome session move <run>/orchestrator --from caller`:** it moves the orchestrator next to the pane running the move. It fails, leaving the session where it was, off cmux, on tmux, for a worker, or from the orchestrator's own pane. The human output names the caller's pane.
- **`tome runs show`:** each session that used the anchor shows `caller: opened next to …`, and the run shows `caller at start: pane … (surface …) in workspace …`.
- **Tests:** `tests/layout.rs` (tmux) covers the validation error, workers skipping a shared `from: caller`, the refused spawn, the tmux and not-from-cmux fallbacks, and the failed moves. `tests/cmux.rs` opens a stand-in "chat" workspace, points `CMUX_*` at it, and covers the tab, a move into a split, and the fallback once the caller's workspace is closed.

## Interpretations made while implementing

- **Who reads the caller:** the CLI, not the daemon, since the daemon's environment isn't the caller's. `tome run` and `tome session move` send `CMUX_SURFACE_ID` and `CMUX_WORKSPACE_ID` as `cmux_caller`. At request time the daemon looks up the surface's pane and workspace, which are recorded and shown. At launch it looks the surface up again, since a pane can move between request and launch.
- **`TOME_LAYOUT`** only carries a layout name, so it can't give `from: caller`.
- **The caller's workspace wins:** with `from: caller`, `layout: workspace` (or `workspace: own`) becomes a tab in the caller's pane, with a source saying so.
- **The caller's own workspace gone:** it's handled like a gone `focused` workspace. The session opens in the project workspace, with a session warning.
- **A worker's own levels vs shared ones:** `from: caller` in a worker's spawn flags, its `workers[]` rule, the `workers` block or a preset used there is an error. At a shared level (`tome run` flags, `defaults.layout`, `TOME_LAYOUT`, the config, or a preset used there) workers skip it.
- **Moves:** `from: caller` applies to a move only when the move's own flags (or their preset) give it. A session placed with `from: caller` drops it on a later move that doesn't give it. `--from caller` fails when the pane running the move is the orchestrator's own.
- **`tome runs show`:** the run shows `caller at start` only when a surface was recorded. Why there was no caller shows as the session's warning when `from: caller` was used.

## Tasks

### 011-1. `from: caller` as a placement value, orchestrator only

**Blocked by:** none

`caller` is accepted as a `from` value at every placement level: `tome run --from caller`, the workflow's `orchestrator` role block, `defaults.layout`, `TOME_LAYOUT`, the project and global config, and presets. It resolves for the orchestrator like any other setting, with its source recorded. Setting it for a worker is an error:
- **Workflow:** a `workers[]` rule or the `workers` role block with `from: caller` makes `tome validate` report an error, and the workflow load does too.
- **Spawn:** `tome worker spawn --from caller` is refused.
- **Shared levels:** at a level both roles share (`defaults.layout`, `TOME_LAYOUT`, the config, or a preset used there), it applies to the orchestrator only. Workers skip it and take `from` from the next level down, with no error.

Launch behaviour doesn't change yet. (UC 4)

### 011-2. Record the caller on the run

**Blocked by:** none

When a run is requested (before any queueing, where the focused workspace is recorded), tome reads the calling process's cmux environment (its surface and pane) and records it on the run as the caller. A run started by a trigger, or by a `tome run` outside a cmux pane, records no caller. `tome runs show` shows the recorded caller. `tome run` itself doesn't change. (UC 1, 6)

### 011-3. Open the orchestrator next to the caller on cmux

**Blocked by:** 011-1, 011-2

With `from: caller`, the orchestrator opens in the caller's workspace, which wins over the resolved `workspace`:
- **`layout: tab`:** a new tab in the caller's pane.
- **`layout: split`:** the caller's pane splits, with the orchestrator in `split.direction` at `split.size`.
- **Focus:** the chat keeps focus. The orchestrator's tab shows in the pane's tab bar.
- **Fallback:** if the run has no caller (a trigger started it, or it wasn't requested from a cmux pane), the backend is tmux (the warning says `from: caller` is cmux only), or the caller's pane closed before a queued run launched, the orchestrator opens where placement would have put it without `from: caller`, with a warning on the session. The run still starts.

`tome runs show` shows whether the caller anchor was used, or why it fell back. (UC 1, 2, 3, 6)

### 011-4. `tome session move --from caller`

**Blocked by:** 011-1, 011-3

`tome session move <run>/orchestrator --from caller` moves a running orchestrator next to the pane that runs the move. It follows the launch rules: a tab in that pane or a split of it, in its workspace, without taking focus. The move fails, and the session stays where it was, like 010's other failed moves, when:
- the command isn't run from a cmux pane
- the session is on tmux
- the target is a worker

(UC 5)
