# Queue

## Definition

A named tome [[primitive]] that agents push messages or tasks onto and pull them from.

## Attributes

- name
- scope: a [[run]] (feature 003); project-wide messaging is the message bus instead (see [[event]], feature 009)
- messages: text up to ~1 MiB with id, sender and timestamp; delivered by claim and ack

## Relationships

- [[trigger]]: signals to running runs go onto the run's `events` queue
- [[event]]: bus events signalled to a run with `to: running` arrive on its `events` queue
- [[action]]: the send-message action pushes to a queue

## Planned Features

- _(feature 009 was re-scoped from project queues to a project message bus; see [[event]])_

## Sources

- intent.md
- .sengify/sources/interview-2026-09-26.md
- .sengify/sources/feature-set-2026-09-26.md
- features/003-primitives/feature.md
- features/009-message-bus/feature.md
