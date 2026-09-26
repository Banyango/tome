# Daemon

## Definition

The long-running tome process that watches for [[trigger]]s and carries out [[action]]s. Tome CLI commands, whether from agents or humans, talk to it.

## Attributes

- lifecycle: long-running
- responsibilities: evaluate triggers, launch each [[run]]'s [[orchestrator]], record the step progress the orchestrator reports, drive [[backend]]s, send [[notification]]s
- control flow: the daemon does not decide step order; the orchestrator does

## Relationships

- [[run]]: manages run state
- [[trigger]]: evaluates triggers
- [[action]]: carries out actions
- [[backend]]: controls the multiplexers

## Planned Features

- Daemon lifecycle (start, stop, status) and the CLI↔daemon protocol, including what happens when the daemon isn't running _(source: .sengify/sources/feature-set-2026-09-26.md)_

## Sources

- .sengify/sources/interview-2026-09-26.md
- .sengify/sources/feature-set-2026-09-26.md
- features/002-running-a-workflow/feature.md
