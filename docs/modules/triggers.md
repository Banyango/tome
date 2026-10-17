# Triggers

What starts a run: file changes, cron, bus topics, or `tome triggers fire`.
Every source goes down one firing path.

## Files

- `src/triggers.rs`: daemon side. The single firing path (`Event`, `FireRequest`,
  `Fired`), the `tome triggers` RPCs (`METHODS`), the fields a trigger injects
  into a run, `waits_for_idle`. Dispatched from `Engine::dispatch_triggers`.
- `src/arming.rs`: which triggers are live. `scan` reads the workflows of every
  registered, enabled project plus global ones; `CronState` tracks schedules;
  re-reads workflow files when they change.
- `src/watch.rs`: `Watcher`. OS file events (`notify`) per armed file trigger,
  with a polling fallback; debounces and batches paths.
- `src/triggerscmd.rs`: CLI client: `tome triggers ls|enable|disable|fire`.
- `src/store/triggers.rs`: `trigger_fires` outcomes and the `projects` registry.
- `src/features/bus/admin.rs`: `tome triggers fire` for topic triggers (bus side).

## Notes

- A file trigger's `while_running:` (`workflow::WhileRunning`: queue, parallel, mute;
  only with `to: new`) and a workflow's `on_conflict` (`workflow::OnConflict`)
  decide what a fire does when a run is already active.
- The daemon re-arms on a tick (`arming::tick`), so workflow edits take effect
  without a restart.
- Topic triggers are delivered via the bus; see [bus.md](bus.md).

## Tests

- Unit: `cargo test --bin tome triggers::` (also `arming::`, `watch::`)
- Integration: `cargo test --test triggers`

## User docs

`docs/src/concepts/triggers.md`, `docs/src/guides/triggers.md`
