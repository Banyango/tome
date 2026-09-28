# Queue

## Definition

A named tome [[primitive]] that agents push messages or tasks onto and pull them from.

## Attributes

- name
- scope: a [[run]] (feature 003), or a project (feature 009); project queues outlive runs and are created by their first push
- messages: text up to ~1 MiB with id, sender and timestamp; delivered by claim and ack
- message states (project queues): `pending`, `claimed`, `done`, `failed` (parked when the claiming run fails; `tome queue retry` requeues it)

## Relationships

- [[trigger]]: a message on a project queue fires `queue:` triggers, which start one run per message
- [[action]]: the send-message action pushes to a queue

## Planned Features

- Project queues and queue triggers _(feature 009)_

## Sources

- intent.md
- .sengify/sources/interview-2026-09-26.md
- .sengify/sources/feature-set-2026-09-26.md
- features/003-primitives/feature.md
- features/009-project-queues/feature.md
