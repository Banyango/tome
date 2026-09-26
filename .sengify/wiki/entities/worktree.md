# Worktree

## Definition

A tome [[primitive]] for a git worktree allocated to a [[step]] or agent. It isolates changes and carries data between steps.

## Attributes

- path
- branch

## Relationships

- [[step]]: steps may be assigned a worktree
- [[action]]: fan-in may merge results from several worktrees

## Planned Features

- Worktree management: allocation per step, cleanup, merging back for fan-in; lifecycle is still open _(source: .sengify/sources/feature-set-2026-09-26.md)_

## Sources

- .sengify/sources/interview-2026-09-26.md
- .sengify/sources/feature-set-2026-09-26.md
