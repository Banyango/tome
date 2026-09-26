# Trigger

## Definition

An event that starts a [[workflow]] or moves it forward.

## Attributes

- kinds:
  - file created / edited
  - [[group]] of tasks complete
  - message received, from:
    - a tome [[queue]]
    - an external source (webhooks, etc.)
    - agent output (pattern-matching a [[session]]'s pane output)

## Relationships

- [[daemon]]: evaluates triggers
- [[workflow]]: workflows declare their triggers
- [[queue]]: a source of message-received triggers
- [[group]]: fires the group-complete trigger

## Planned Features

- Triggers: may be split into file triggers, message triggers, and matching agent output _(source: .sengify/sources/feature-set-2026-09-26.md)_

## Sources

- intent.md
- .sengify/sources/interview-2026-09-26.md
- .sengify/sources/feature-set-2026-09-26.md
