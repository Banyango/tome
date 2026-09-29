# Orchestrator next to the caller — Task Plan

Source feature: [feature.md](./feature.md)

> All tasks are done: implemented as 011-1 to 011-4 directly on main (f9b2098, a5e74c7, 590370b, 0491034).

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
