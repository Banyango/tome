# Queue

## Definition

A named tome [[primitive]] that agents push messages or tasks onto and pull them from.

## Attributes

- name
- messages

## Relationships

- [[trigger]]: a message on a queue can fire a message-received trigger
- [[action]]: the send-message action pushes to a queue

## Planned Features

- Queues: create, push, pull; whether a message is delivered once or can be seen again is still open _(source: .sengify/sources/feature-set-2026-09-26.md)_

## Sources

- intent.md
- .sengify/sources/interview-2026-09-26.md
- .sengify/sources/feature-set-2026-09-26.md
