# Session

## Definition

A tome [[primitive]] for a single terminal multiplexer session, pane or window that runs an agent or command.

## Attributes

- backend: cmux, tmux or herdr (via [[backend]])
- output: can be pattern-matched to fire a message-received [[trigger]]

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
