# Remote workers

`tome worker spawn --node <name>` runs a worker's session on a node while
the run, and the worker's record, stay with the daemon that owns the run.
Two daemons are involved: the **run's daemon** (owns the worker) and the
**node's daemon** (hosts its session).

## Files

- `src/features/remote/mod.rs`: `METHODS` (`hosted.spawn`, `hosted.close`),
  the `hosted_workers` ping capability, `TOME_HOSTED`, `node_unreachable`,
  and hosted session names (`tome-hosted-<origin tag>-<run>-<name>`, which a
  node run's own `tome-<id>-` cleanup never matches).
- `src/features/remote/link.rs`: the run's side. `Links` holds one link per
  node: a thread that owns the node's `rpc::Client` (`ssh <node> tome rpc
  --stdio`), loops on `hosted.poll`, answers relayed calls, ends workers
  whose sessions ended, and runs commands queued for it (`with_node`,
  `node_call`, `node_close`). It closes after `LINGER` with no running
  worker on its node. A failed poll fails the node's running workers with
  `node_unreachable`.
- `src/features/remote/spawn.rs`: `remote_node` (refuses providers and
  unknown nodes, opens the link, checks the capability) and
  `launch_remote` (locates the project on the node, calls `hosted.spawn`).
- `src/features/remote/host.rs`: the node's side. `Host` queues hosted
  workers' calls per origin and wakes the waiting poll; `hosted_spawn`,
  `hosted_poll`, `hosted_close`, and `check_hosted` (from the monitor:
  marks ended sessions, drops upstreams that stopped polling and orphans
  their workers).
- `src/store/hosted.rs`: the node's `hosted_workers` table. The run's
  `workers` table has a `node` column.

## Flow

1. The run's daemon reserves the worker, records its node and calls
   `hosted.spawn {origin, run_id, name, command, project, ...}`.
2. The node launches the session with `TOME_RUN_ID`, `TOME_WORKER_ID` and
   `TOME_HOSTED=<origin>`. `rpc::caller()` adds `hosted` to every call the
   worker makes; `Daemon::handle` sends those to `Host::relay` instead of
   dispatching them.
3. Each `hosted.poll` returns queued calls and ended sessions, and carries
   the replies to earlier calls. The run's daemon dispatches relayed calls
   (only worker/step/agent methods, only from a live worker on that node)
   and ends exited workers through `Engine::end_exited`.
4. When a worker ends, `after_end` sends `hosted.close`; cancelling the run
   (`Engine::kill_sessions`) closes all of its hosted workers.

The node's daemon notices its upstream is gone when the poll connection
closes (`Connection` guard in `daemon.rs`) or after `LOST_AFTER` without a
poll; it kills those sessions unless they were kept open.
