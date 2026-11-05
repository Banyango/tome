# Remote workers

`tome worker spawn --node <name>` runs a worker's session on a node while
the run, and the worker's record, stay with the daemon that owns the run.
Two daemons are involved: the **run's daemon** (owns the worker) and the
**node's daemon** (hosts its session).

## Files

- `src/features/remote/mod.rs`: `METHODS` (`hosted.spawn`, `hosted.close`, `hosted.attach`),
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
  `launch_remote` (locates the project on the node, pushes the base as the
  worker's branch in a git project, calls `hosted.spawn`), and `fetch_back`
  (fetches the branch back when the worker ends; a failure becomes a run
  note).
- `src/features/remote/host.rs`: the node's side. `Host` queues hosted
  workers' calls per origin and wakes the waiting poll; `hosted_spawn`,
  `hosted_poll`, `hosted_close`, and `check_hosted` (from the monitor:
  marks ended sessions, drops upstreams that stopped polling and orphans
  their workers).
- Viewing: `session.attach_command` for a worker with a `node` asks that
  node (`hosted.attach`) and returns `{node, backend, command}`;
  `tome session view` (`nodes::cmd::view`) then runs it over `ssh -t`.
- `src/store/hosted.rs`: the node's `hosted_workers` table. The run's
  `workers` table has a `node` column.

The node a worker runs on comes from `--node`, else
`placement::Spec::node_for` (`defaults.layout.workers` rules' and block's
`node`); `local` means this machine. `tome validate` warns about nodes that
aren't configured (`validate::warnings_json`).

## Flow

1. The run's daemon reserves the worker, records its node and, in a git
   project, pushes the base commit to `<node.ssh>:<project>` as
   `refs/heads/<branch>` (`git push` with `GIT_SSH_COMMAND` set to tome's
   ssh). It then calls `hosted.spawn {origin, run_id, name, command, project,
   worktree: {branch}, agent: {harness, model, prompt}, ...}` (`command` or
   `agent`; the node resolves the harness from its own config); the node adds a worktree at
   `.tome/worktrees/hosted-<tag>-<run>-<name>` and returns its path, which
   the run records as the worker's worktree (`<node>:<path>`). It isn't in
   the run side's `worktrees` table.
2. The node launches the session with `TOME_RUN_ID`, `TOME_WORKER_ID` and
   `TOME_HOSTED=<origin>`, plus `TOME_WORKTREE` when it has a worktree. `rpc::caller()` adds `hosted` to every call the
   worker makes; `Daemon::handle` sends those to `Host::relay` instead of
   dispatching them.
3. Each `hosted.poll` returns queued calls and ended sessions, and carries
   the replies to earlier calls. The run's daemon dispatches relayed calls
   (only worker/step/agent methods, only from a live worker on that node)
   and ends exited workers through `Engine::end_exited`.
4. When a worker ends, `after_end` sends `hosted.close` and fetches its
   branch back; cancelling the run
   (`Engine::kill_sessions`) closes all of its hosted workers.

`gc::collect` calls `host::gc_hosted` on every daemon: hosted workers that
ended before the cutoff lose their row, files, worktree and branch (unless
their session is still open). On the run's side, an unreachable node at
spawn reports the first failing `nodes::cmd::check_steps` step.

The node's daemon notices its upstream is gone when the poll connection
closes (`Connection` guard in `daemon.rs`) or after `LOST_AFTER` without a
poll; it kills those sessions unless they were kept open.
