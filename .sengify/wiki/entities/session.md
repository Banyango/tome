# Session

## Definition

A tome [[primitive]] for a single terminal multiplexer session, pane or window that runs an agent or command.

## Attributes

- backend: cmux, tmux or herdr (via [[backend]])
- output: can be pattern-matched to fire a message-received [[trigger]]
- layout: `tab` (default), `split` or `workspace` (part of its placement). It decides where in the backend the session opens; `tab` and `split` use the project's tome workspace `<project>-orchestrator` _(source: [008](../features/008-session-layout/feature.md))_
- placement: layout plus workspace (`project`, `focused`, `own` or a name for `<project>-<name>`), split direction and size, and the `from` anchor pane, each with the level it came from and any warnings. It resolves from flags, presets, per-worker rules, role blocks, the workflow's `defaults.layout` and the project and global config. `tome session move` re-places a live session without restarting it _(source: [010](../features/010-session-placement/feature.md))_
- `from: caller` (orchestrator, cmux only): anchors on the pane that ran `tome run`, whose workspace wins; the placement records the pane it opened next to _(source: [011](../features/011-caller-placement/feature.md))_
- handle: the session's own pane or tab (tmux pane, cmux surface). Liveness checks, kill, nudges and crash recovery go by this handle _(source: [008](../features/008-session-layout/feature.md))_

## Relationships

- [[backend]]: sessions are created through a backend
- [[step]]: a step may run inside a session
- [[harness-adapter]]: agents launched in a session go through the adapter

## Planned Features

- Terminal backends: one session interface over cmux, tmux and herdr _(source: .sengify/sources/feature-set-2026-09-26.md)_

## Sources

- intent.md
- .sengify/sources/interview-2026-09-26.md
- .sengify/sources/feature-set-2026-09-26.md
- features/011-caller-placement/feature.md
