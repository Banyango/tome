# Runs

The run lifecycle: starting a run, recording step transitions, launching the
orchestrator, and the commands to resume, cancel, inspect and clean up.

## Files

- `src/engine.rs`: daemon side. `Engine` owns the DB connection, starts runs,
  records transitions, streams events to watchers (`Sink`, `run_event`,
  `step_event`), and **dispatches RPCs for runs, workers, triggers and the bus**.
- `src/api.rs`: RPC methods backed directly by the store (`run.create`,
  `run.get`, `runs.list`, `worktree.add` ...); `create_run`, `Origin`.
- `src/orchestrator.rs`: launching a run's orchestrator agent via the harness;
  placement resolution, workflow snapshot, run cwd.
- `src/runcmd.rs`: CLI: `tome run`, `run finish|cancel|resume`, `tome step ...`,
  `tome ready`.
- `src/resume.rs`: `tome run resume`: new run with `resumed_from`, reusing the
  old snapshot, worktrees and deliveries.
- `src/recovery.rs`: crash recovery once at daemon start (`RecoveryHooks`).
- `src/handshake.rs`: the start handshake: a launched agent must make a tome
  call carrying its identity before `start-timeout`.
- `src/gc.rs`: `tome gc --older-than`; `src/stop.rs`: daemon shutdown signal.
- `src/inspect.rs`, `src/query.rs`: `tome runs list|show|logs`, `tome query`.
- `src/store.rs`: schema and migrations (all tables), `Run`, `Step`, `Session`,
  `RunStatus`, `StepEvent`; `Store` is the DB handle.

## Notes

- Only the daemon opens DuckDB; CLI commands call it over RPC.
- New RPC method: add it to the owning module's `METHODS`/`dispatch`, and route
  it in `Engine::handles`/`dispatch` (or `api::handles` for store-only methods).
- Schema changes need a migration in `store.rs`.

## Tests

- Unit: `cargo test --bin tome engine::` (also `api::`, `resume::`, `recovery::`, `store::`, `gc::`)
- Integration: `run_control`, `orchestrator`, `resume`, `recovery`, `handshake`,
  `gc`, `inspect`, `store`, `attached_runs`, `single`, `concurrency`

## User docs

`docs/src/concepts/workflows.md`, `docs/src/concepts/daemon.md`
