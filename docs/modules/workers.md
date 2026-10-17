# Workers

What an orchestrator calls to delegate work: workers, groups, git worktrees and
message queues.

## Files

- `src/workers.rs`: daemon side. `METHODS` (RPCs behind `tome worker|group|
  worktree|queue`), spawning a worker into a session, the monitor pass that
  notices workers whose session ended, and the queue handlers. Dispatched from
  `Engine::dispatch_primitive`.
- `src/primitives.rs`: CLI client for those commands. Agents find their run
  through `--run <id>` / `TOME_RUN_ID`; a worker is `TOME_WORKER_ID`.
- `src/store/workers.rs`: `workers`, `worker_groups`, `worker_events` tables.
- `src/store/queues.rs`: `queues` and `queue_messages`. Queues belong to the
  project, not a run; a claimed message returns to the queue if its worker ends.
- `src/worktree.rs`: creating a run's git worktree/branch (`tome/<run>/<name>`).

## Notes

- Per-run files (prompt, log, exit code) live under the run dir from
  `workers.rs` helpers (`run_dir`, `exit_file`, `prompt_file`, `log_file`).
- Where a worker's terminal opens is [sessions.md](sessions.md) (placement).
- Queues are listed here, not under the bus: the bus is topic events and
  deliveries, queues are work hand-off between a run's agents.

## Tests

- Unit: `cargo test --bin tome workers::` (also `store::workers`, `store::queues`, `worktree::`)
- Integration: `cargo test --test workers`

## User docs

`docs/src/concepts/primitives.md`, `docs/src/guides/workers.md`
