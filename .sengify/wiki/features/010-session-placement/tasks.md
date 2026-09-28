# Session placement — Task Plan

Source feature: [feature.md](./feature.md)

## Tasks

### 010-1. Project config file

**Blocked by:** none

A project-level `.tome/config.yaml` accepts every key that `~/.tome/config.yaml` takes. The project file wins per top-level key or per entry: a harness (and later a preset) with the same name in the project file replaces the global one entirely, the same way project workflows override global ones. An unknown key in the project config gives the same error as an unknown key in the global config. (UC 11)

### 010-2. Placement settings, presets and resolution

**Blocked by:** 010-1

The single `layout` string is replaced with placement settings:
- **The settings:** `layout`, `workspace`, `split.direction`, `split.size`, `from` and `preset`.
- **Presets:** the built-in `tab`, `split` and `workspace` presets, plus named presets under `layout_presets` in the global and project config. Presets can't reference other presets.
- **The workflow form:** a `defaults.layout` block with `preset:`, an `orchestrator` role block, a `workers` role block and `workers[]` rules that match the worker's spawn name as a glob (the first match wins).

Each setting resolves on its own, following the feature's precedence chain. An unknown `layout`, `direction`, `from` or `workspace` value makes `tome validate` exit `2` in a workflow, and in config it errors with a hint. A preset that's referenced but not defined gives a warning in `tome validate`. The old forms keep working: `layout: tab|split|workspace`, `defaults.layout: <string>` and `TOME_LAYOUT`. Each session's resolved placement is recorded, along with where each setting came from. `tome layout presets` lists the built-in, global and project presets with their settings. Launch behaviour doesn't change yet. (UC 1–5, 14, 15)

### 010-3. Placement flags on `tome run` and `tome worker spawn`

**Blocked by:** 010-2

`tome run` and `tome worker spawn` accept `--preset --layout --workspace --direction --size --from`. `tome worker spawn` flags are the top level for workers, and `tome run` flags are the top level for the orchestrator and level 4 for workers. On the command line, a reserved word passed to `--workspace` is read as the keyword. A preset that isn't defined fails the command with a hint listing the known presets. (UC 6, 7)

### 010-4. Split direction, size and anchor pane

**Blocked by:** 010-2

On tmux and cmux, launches follow the resolved `split.direction`, `split.size` and `from`:
- **Anchor pane:** `from` is `orchestrator`, `last` or `first`, scoped to the run. If the anchor pane is gone, it falls back to `last`, then to `first`.
- **`layout: split`:** the new pane splits off the anchor pane in the given direction and at the given size.
- **`layout: tab`:** with `from`, the new tab joins the anchor pane. Without it, it goes to 008's shared tab split, which is now created with the resolved direction and size. On tmux, `from` is ignored for tabs.
- **Size:** it's best effort. tmux sets it when splitting, and cmux resizes after the split. A size that can't be honoured is clamped or skipped, and a warning is recorded on the session. Only a failure to create the pane is an error.

(UC 8)

### 010-5. Workspace targets

**Blocked by:** 010-2

Launches on tmux and cmux follow the resolved `workspace`:
- **`project`:** the project's `<project>-orchestrator` workspace, as in 008.
- **`focused`:** the workspace or tmux session that's focused when the run starts, recorded once and kept for the whole run. When focus can't be determined (the daemon started the run from a trigger, or the multiplexer isn't attached), it falls back to `project` and records a note on the run.
- **A name:** a workspace named `<project>-<name>`, with the same lifecycle as the project workspace. It's created on first use, found again by its recorded id (or recreated if that id is gone), and left open with a shell when it's empty. `{ name: own }` in YAML is an error, because the reserved words can't be used as names.
- **`own`:** each session gets its own workspace (on tmux, its own session). This replaces 008's `layout: workspace`.

(UC 9, 10)

### 010-6. Placement in `tome runs show`

**Blocked by:** 010-4, 010-5

`tome runs show` shows each session's resolved placement and where each setting came from (a preset, a rule, a role block, a flag, `TOME_LAYOUT`, project or global config, or the built-in default). It also shows any placement warnings recorded on the session (such as a size that was clamped or skipped) and any placement notes on the run (such as `focused` falling back to `project`). (UC 13)

### 010-7. `tome session move`

**Blocked by:** 010-3, 010-4, 010-5

`tome session move <run>/<name>` takes the same placement flags and moves a live orchestrator or worker to its new placement. The agent's process isn't restarted:
- **cmux:** it uses `move-surface`, `drag-surface-to-split`, `break-pane`, `join-pane` and `move-tab-to-new-workspace`.
- **tmux:** it uses `join-pane`, `break-pane` and `move-window`.

tome updates the session's recorded handle and resolved placement, so liveness checks, kill, nudges and crash recovery follow the session to its new place. A preset that isn't defined fails with a hint. If a move fails partway, the session stays where it was, its recorded handle doesn't change, and the command errors. (UC 12)

> All tasks are done: implemented as 010-1 to 010-7 directly on main (2b214b1, 0a30973, 03db0ec, e9ee58b, 51707d1, 55fa914, 390ec6f).
