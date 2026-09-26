# Group

## Definition

A tome [[primitive]] that tracks a set of tasks together. Fan-out produces groups, and fan-in and the group-complete trigger depend on them.

## Attributes

- members: [[step]]s or tasks
- completion: all members are done (see [[step]] completion conditions)

## Relationships

- [[action]]: created by fan-out, consumed by fan-in
- [[trigger]]: fires the group-complete trigger

## Planned Features

- Groups, fan-out and fan-in: spreading work across agents, tracking the group, consolidating results _(source: .sengify/sources/feature-set-2026-09-26.md)_

## Sources

- intent.md
- .sengify/sources/interview-2026-09-26.md
- .sengify/sources/feature-set-2026-09-26.md
