# Event

## Definition

A message published to a named topic on a project's message bus. Every [[workflow]] subscribed to the topic gets its own delivery of it.

## Attributes

- topic: dot-separated segments, e.g. `review.requested`; the `tome.` prefix is reserved for built-in events
- payload: text up to ~1 MiB
- sender: `user`, `run N`, `run N worker W`, `tome`, or `test` (`tome triggers fire --payload`)
- depth: `0` from the user, and one more than the event that started the publishing [[run]]; events deeper than `8` aren't delivered
- deliveries: one per matching subscription, in state `pending`, `claimed`, `done`, `failed` or `dropped`
- built-in topics: `tome.run.<workflow>.started|succeeded|failed|cancelled` with JSON payloads; `.started` follows the orchestrator's first tome call _(source: [009](../features/009-message-bus/feature.md))_
- refused: set when a publish is over the depth limit; the event is kept for inspection but not delivered _(source: [009](../features/009-message-bus/feature.md))_
- inspected with `tome events ls|show`, repaired with `tome events retry|remove`, and cleared by `tome gc` once all its deliveries are settled _(source: [009](../features/009-message-bus/feature.md))_

## Relationships

- [[trigger]]: an `on:` trigger subscribes a workflow to a topic pattern; each delivery starts a run or signals running ones
- [[run]]: a run may be started by an event, claims its delivery, and settles it when it ends; runs publish events with `tome publish` and through their lifecycle
- [[queue]]: with `to: running`, the event is signalled through the run's `events` queue
- [[notification]]: sent when a delivery fails or a chain goes too deep

## Sources

- features/009-message-bus/feature.md
