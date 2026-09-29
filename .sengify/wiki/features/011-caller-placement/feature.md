# Orchestrator next to the caller

## Description

A chat window, such as a Claude Code session in a cmux pane, starts a workflow the way any agent runs a command: its shell runs `tome run <workflow> --detach`. The run's [[orchestrator]] should then open right next to the chat, not in tome's project workspace.

This adds a new `from` value, `caller`, to 010's placement system. The caller is the pane that ran the command. tome reads it from the calling process's cmux environment (its surface and pane) and records it on the [[run]] when the run is requested, before any queueing, the same way it records the focused workspace.

- **`layout: tab` with `from: caller`:** the orchestrator opens as a new tab in the caller's pane.
- **`layout: split` with `from: caller`:** the caller's pane splits, with the orchestrator in `split.direction` at `split.size`.
- **Workspace:** the caller's workspace wins over whatever `workspace` resolved to. Using the caller's workspace is the whole point of the setting.
- **Focus:** the chat keeps focus. The orchestrator's tab shows in the pane's tab bar, and the user switches to it when they want to watch.
- **Opting in:** it goes through the normal placement levels: `tome run --from caller`, the workflow's `orchestrator` block or `defaults.layout`, `TOME_LAYOUT`, the config, or a preset. Without `from: caller`, nothing changes.
- **Moving:** `tome session move <run>/orchestrator --from caller` moves an orchestrator that's already running next to the pane that runs the move.
- **Scope:** cmux only, and the orchestrator only.

The chat agent passes `--detach`, gets the run id back, and checks on the run with `tome runs show <id>`. `tome run` itself doesn't change.

**Out of scope:** tmux support; telling the chat when the run ends; a built-in preset (users can add their own under `layout_presets`); detaching automatically when called from a chat.

## Use Cases

1. As a user chatting with an agent, I want the agent to start a workflow with `tome run <workflow> --detach --from caller`, so that I can launch runs without leaving the chat.
2. As a user, I want the orchestrator to open as a tab in my chat's pane without taking focus, so that I can keep chatting and switch to the run when I want to.
3. As a user, I want `from: caller` with `layout: split` to split my chat's pane, so that I can watch the run side by side with the chat.
4. As a workflow author, I want to set `orchestrator: {from: caller}` in a workflow's `defaults.layout` (or in config or a preset), so that runs of it always open next to whoever started them.
5. As a user, I want `tome session move <run>/orchestrator --from caller`, so that I can pull an orchestrator that's already running next to my current pane.
6. As a user, I want `tome runs show` to show whether the caller anchor was used, or why it fell back, so that I can understand where the orchestrator went.

## Constraints

- It's part of 010's placement system: a new `from` value that resolves per setting like the others. Without it, placement behaves exactly as before.
- cmux only. The caller comes from the calling process's cmux environment.
- Orchestrator only. `from: caller` on a worker (a `workers[]` rule, the `workers` role block, or `tome worker spawn --from caller`) is a validation error.
- The caller is recorded on the run when the run is requested, before queueing, and `tome runs show` shows it.
- `tome run` isn't changed. A chat agent is expected to pass `--detach`.
- The orchestrator's tab or split opens without focus.

## Edge Cases

- A trigger started the run, so there's no caller → the orchestrator opens where placement would have put it without `from: caller`, with a warning on the session. The run starts.
- `tome run` wasn't run from a cmux pane (no cmux environment) → the same fallback, with a warning.
- The run's backend is tmux → the same fallback, with a warning saying `from: caller` is cmux only.
- The caller's pane closed before a queued run launched → the same fallback, with a warning.
- `from: caller` is set for a worker → `tome validate` and the workflow load report it as an error. `tome worker spawn --from caller` is refused.
- `from: caller` is set at a level both roles share (`defaults.layout`, `TOME_LAYOUT`, the config, or a preset used there) → it applies to the orchestrator only. Workers skip it and take `from` from the next level down, with no error.
- `tome session move --from caller` can't use the caller (no cmux pane, a tmux session, or a worker) → the move fails and the session stays where it was, like 010's other failed moves.

## Related Entities

- [[session]]
- [[orchestrator]]
- [[run]]
- [[backend]]
- [[workflow]]
