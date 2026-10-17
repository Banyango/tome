# Bus

The project message bus: events published to topics and a delivery recorded for
each subscribing workflow (`on:` triggers).

## Files

- `src/bus.rs`: daemon side. `Publish`, `Published`, subscription matching
  (`subscriptions`, `matching`), `METHODS`; dispatched from `Engine::dispatch_bus`.
- `src/bus/lifecycle.rs`: tome's own events: `tome.run.<workflow>.started|
  succeeded|failed|cancelled`.
- `src/bus/forward.rs`: `bus.forward` rules in `.tome/config.yaml`: re-publish
  matching events on another node.
- `src/bus/admin.rs`: inspect/repair (`tome events ls|show|retry|rm`) and manual
  topic trigger fires.
- `src/eventscmd.rs`: CLI client: `tome publish`, `tome events ...`.
- `src/store/events.rs`: `bus_events` and `deliveries` tables.
- `src/topic.rs`: topic name rules and subscription `Pattern`.

## Notes

- A delivery becomes a run via the trigger path ([triggers.md](triggers.md)).
- `bus::depth_from` tracks how deep an event chain is (an event published by a run
  that an earlier event started is one deeper).
- `bus` calls into `engine` to start runs, and `engine` dispatches into `bus`:
  a known cycle.

## Tests

- Unit: `cargo test --bin tome bus::` (also `topic::`, `store::events`)
- Integration: `cargo test --test bus`
