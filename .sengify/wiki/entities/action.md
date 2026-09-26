# Action

## Definition

An operation the [[orchestrator]] carries out through tome primitives while working through a [[step]].

## Attributes

- kinds (v1):
  - fan-out: spread work across multiple agents or sessions
  - fan-in (consolidate): merge results from a [[group]]
  - run agent / command: launch through the [[harness-adapter]], optionally in a new [[session]]
  - notify user: send a [[notification]]
  - send message: push to a [[queue]] or to a running agent
  - branch / loop: decided by the orchestrator at runtime from the workflow's English

## Relationships

- [[orchestrator]]: performs actions
- [[step]]: actions are how a step gets done
- [[group]]: fan-out creates a group; fan-in consumes one
- [[harness-adapter]]: used by run agent
- [[notification]]: produced by notify user

## Planned Features

- Send message action: push to a queue or into a running agent's pane _(source: .sengify/sources/feature-set-2026-09-26.md)_
- Groups, fan-out and fan-in _(source: .sengify/sources/feature-set-2026-09-26.md)_

## Sources

- intent.md
- .sengify/sources/interview-2026-09-26.md
- .sengify/sources/feature-set-2026-09-26.md
- features/002-running-a-workflow/feature.md
