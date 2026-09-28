# Project message bus and topic triggers — Follow-ups

Source feature: [feature.md](./feature.md) · Tasks and what changed: [tasks.md](./tasks.md)

Gaps and rough edges found while implementing 009-1 to 009-7 (2026-09-28). None of them block the feature; each is a candidate for a later task.

- **Unsubscribed lifecycle events:** every run records about two events even when nothing subscribes to them, and only `tome gc` clears them. Consider skipping the record when there are no deliveries.
- **Lost end events:** a run's end is marked handled before its event is published, so a failed publish (such as a store error) is logged and lost rather than retried.
- **Workflow names that aren't topic segments:** a name with uppercase letters or dots gets no lifecycle events, only a daemon log line. `tome validate` could warn about it.
- **Refused publishes from a run:** these return exit `2` to the caller, but don't show in the run's stream the way successful publishes do.
- **`to: running` deliveries on long-lived runs:** they stay `claimed` until one of the signalled runs ends. A run that never finishes holds them indefinitely. `tome events remove` only takes `pending` or `failed` deliveries.
- **`event_id` in the queue body is a string:** it comes from the `{{trigger.*}}` fields, which are all strings. Consumers that expect a number need to parse it.
- **Stalled subscriptions:** these clear only on a rescan (a workflow edit, or `tome triggers enable`). A transient error therefore waits for the next rescan before it's retried.
