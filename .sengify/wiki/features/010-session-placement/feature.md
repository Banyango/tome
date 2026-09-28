# Session placement

## Description

Today, where a [[session]] goes is one `layout` setting (`tab | split | workspace`, feature [008](../008-session-layout/feature.md)). It's chosen once per [[run]], and splits always open to the right at the multiplexer's default size. This feature replaces it with **placement settings**. A [[workflow]] declares them, can build them on named **presets**, and can override them per role, per [[worker]] name pattern, and per command.

### Placement settings

| setting | values | notes |
|---|---|---|
| `layout` | `tab`, `split` | `workspace` stays valid as an alias for `workspace: own` |
| `workspace` | `project` (default), `focused`, a name, `own` | see below |
| `split.direction` | `right`, `down`, `left`, `up` | |
| `split.size` | a percent (`30%`) or cells (`80`) | best effort |
| `from` | `orchestrator`, `last`, `first` | the anchor pane, scoped to the run |
| `preset` | a preset name | the base; settings next to it override it |

- **`workspace: project`:** the project's tome workspace, `<project>-orchestrator` (as in 008).
- **`workspace: focused`:** the workspace or tmux session that's focused when the run starts. It's recorded and kept for the whole run, even if the user switches away.
- **`workspace: <name>`:** a workspace named `<project>-<name>`. It has the same lifecycle as `<project>-orchestrator`: it's created on first use, found again by its recorded id, and left open with a shell when it's empty. `project`, `focused` and `own` are reserved and can't be used as names.
- **`workspace: own`:** each session gets its own workspace (tmux: its own session). This is the behaviour of 008's `layout: workspace`.
- **`from`:**
  - `orchestrator`: this run's orchestrator pane
  - `last`: the pane this run created most recently
  - `first`: the workspace's first pane (the tome shell)
  - If the anchor pane is gone, `from` falls back to `last`, then to `first`.
- **`layout: split`:** the new pane splits off the `from` pane in `split.direction`, at `split.size`.
- **`layout: tab`:** the new tab joins the `from` pane. Without `from`, it goes to 008's shared tab split, which is created with `split.direction` and `split.size`. On tmux, a tab is still a window in the target session and `from` is ignored.

### Presets

- **Where they're defined:** a preset is a named bundle of placement settings under `layout_presets` in `~/.tome/config.yaml` or in the project's `.tome/config.yaml`.
- **Built-in presets:** `tab`, `split` and `workspace`, which match 008's layouts.
- **Where they're referenced:** `preset:` is allowed in every placement block (`defaults.layout`, role blocks, worker rules) and as a `--preset` flag. Settings next to `preset:` override the preset's own values.
- **No nesting:** a preset can't reference another preset.

### Project config

- **A new file:** a project-level `.tome/config.yaml` that accepts every key `~/.tome/config.yaml` takes (backend, harnesses, `layout`, `layout_presets`, …).
- **Merging with the global file:** the project wins per top-level key or entry. A preset or harness with the same name in the project file replaces the global one entirely, the same way project workflows override global ones.

### Workflow form

```yaml
defaults:
  layout:
    preset: wide
    split: { size: 30% }
    orchestrator: { layout: split, split: { direction: right, size: 40% } }
    workers:
      - match: "review-*"      # glob on the worker's spawn name; first match wins
        preset: quiet
        from: orchestrator
      - match: "*"
        layout: split
        split: { direction: down }
```

Workers are named when they're spawned (`tome worker spawn --name`), so `workers[]` rules match on that name.

### Precedence

Each setting resolves on its own, from the highest level down. A level that sets only `direction` leaves every other setting to the levels below it.

| order | for workers | for the orchestrator |
|---|---|---|
| 1 | `tome worker spawn` flags | `tome run` flags |
| 2 | the first matching `workers[]` rule | the `orchestrator` role block |
| 3 | the `workers` role block | |
| 4 | `tome run` flags | |

Both kinds of session continue down the same list:

1. The settings written directly under the workflow's `defaults.layout`
2. The `defaults.layout.preset` that those settings are built on
3. `TOME_LAYOUT`
4. The project `.tome/config.yaml`
5. The global `~/.tome/config.yaml`
6. The built-in default, `tab`

### Commands

- **`tome run` and `tome worker spawn`:** both accept `--preset --layout --workspace --direction --size --from`.
- **`tome session move <run>/<name> [flags]`:** re-places a live orchestrator or worker, using the same flags. The agent keeps running and tome updates the session's recorded handle.
- **`tome layout presets`:** lists the built-in, global and project presets with their settings.
- **`tome runs show`:** shows each session's resolved placement, where each setting came from (a preset, a rule, a flag, config, …), and any placement warnings.

### Out of scope

- The herdr [[backend]]. tome doesn't have one yet, so a separate feature will build it along with placement.
- Presets that reference other presets.

## Use Cases

1. As a workflow author, I want a workflow to declare where its sessions go and how they're arranged, so that it looks the same wherever it runs.
2. As a user, I want to define named presets in global or project config, so that workflows can share an arrangement without repeating it.
3. As a workflow author, I want to start from a preset and override single settings, so that small variations don't need a new preset.
4. As a workflow author, I want different placement for the orchestrator and its workers, so that, for example, the orchestrator sits in a wide split and workers stack as tabs.
5. As a workflow author, I want to place workers by name pattern, so that fan-out workers like `review-*` share one rule.
6. As an agent, I want to pass placement flags to `tome worker spawn`, so that I can put a particular worker somewhere specific.
7. As a user, I want placement flags on `tome run`, so that I can change placement for a single run.
8. As a user, I want to choose split direction, size and anchor pane, so that panes don't always open to the right at the default size.
9. As a user, I want sessions to open in the workspace I had focused when the run started, so that the work shows up where I'm looking.
10. As a user, I want named workspaces (`<project>-<name>`), so that different kinds of work stay in separate workspaces.
11. As a user, I want a project `.tome/config.yaml` that overrides any global setting, so that each project can have its own defaults.
12. As a user, I want to move a running session to a different placement, so that I can rearrange my view without restarting the agent.
13. As a user, I want to see each session's resolved placement and where each setting came from, so that I can tell why a pane ended up where it did.
14. As a user, I want to list the available presets, so that I know what I can refer to.
15. As an existing user, I want `layout: tab|split|workspace` to keep working, so that my current configs don't break.

## Constraints

- **Backends:** only cmux and tmux get placement settings. herdr is out of scope.
- **Split size on cmux:** `cmux new-split` has no size flag, so size is applied after the split with `cmux resize-pane`. tmux sets the size when it splits (`split-window -l`).
- **Split direction and anchor on cmux:** `cmux new-split <left|right|up|down> --surface <anchor>` supports all four directions and splitting from a chosen pane.
- **Moving a session:** on cmux this uses `move-surface`, `drag-surface-to-split`, `break-pane`, `join-pane` and `move-tab-to-new-workspace`. On tmux it uses `join-pane`, `break-pane` and `move-window`. The agent's process isn't restarted.
- **Session handles:** the 008 handle model still applies. Liveness checks, kill, nudges and crash recovery follow a session's handle after it moves.
- **Precedence:** the order and the per-setting merge above are part of the spec.
- **Compatibility:** `layout: tab|split|workspace`, `defaults.layout: <string>` and `TOME_LAYOUT` keep working as they do today.

## Edge Cases

- **A preset is referenced but not defined** → `tome validate` warns and exits `0`, because presets depend on the environment. `tome run`, `tome worker spawn` and `tome session move` fail with a hint listing the known presets.
- **A size can't be honoured** (the percent is too large, tmux has no space for a new pane, or the cmux resize fails) → the pane is still created, the size is clamped or skipped, and a warning is recorded on the session. Only a failure to create the pane is an error.
- **`workspace: focused` can't determine focus** (a trigger started the run from the daemon, or the multiplexer isn't attached) → placement falls back to `project` and a note is recorded on the run.
- **The `from` anchor pane is gone** → placement falls back to `last`, then to `first`.
- **A workspace name is a reserved word** (`project`, `focused` or `own`) → on the command line it's read as the keyword. `{ name: own }` in YAML is an error.
- **A named workspace is renamed or closed by hand** → the same handling as the project workspace: tome finds it by its recorded id, or creates it again if that id is gone.
- **An unknown `layout`, `direction`, `from` or `workspace` value** → in a workflow, `tome validate` exits `2`. In either config file, the command errors with a hint, like an unknown `backend`.
- **An unknown key in the project config** → the same error as for an unknown key in the global config.
- **A `tab` placement with `from` on tmux** → `from` is ignored and a window is created in the target session.
- **A move fails partway** → the session stays where it was, its recorded handle doesn't change, and the command errors.
- **No `workers[]` rule matches a worker's name** → the `workers` role block applies, then the levels below it.

## Related Entities

- [[session]]
- [[backend]]
- [[workflow]]
- [[orchestrator]]
- [[worker]]
- [[run]]
- [[daemon]]
