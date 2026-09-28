# Agent start handshake — Task Plan

Source feature: [feature.md](./feature.md)

> All tasks are done: implemented as 007-1 to 007-3 directly on main (7a2c286, c7baffd, 4597194).

## Tasks

### 007-1. Orchestrator start handshake, nudge and fail

**Blocked by:** none

A new `tome ready` command does nothing except record the call. It works out from its env vars whether the caller is the orchestrator or a worker. Any tome call carrying the run's `TOME_RUN_ID` counts as proof that the orchestrator has started. The orchestrator bootstrap prompt tells the agent to run `tome ready` before anything else, and lists it in its command reference. A start timer begins when the orchestrator's session is launched, not when the run is queued. It defaults to 60s, and `TOME_START_TIMEOUT` overrides it for tests. A queued run has no timer until its orchestrator is launched. If the timeout passes with no call, the daemon types one line into the orchestrator's pane ("tome: read `<prompt file>` and follow it"), using the existing nudge mechanism, and waits another timeout. If the pane is gone, the nudge is skipped. A call during that second wait counts as a start, and the run carries on normally. If there's still no call, the run fails with reason `orchestrator_no_start` and its session is killed. The captured log is kept in `~/.tome/runs/<id>/` and the usual failure notification is sent. Because the run has ended, its workflow's file triggers are unmuted. Tome calls made after the fail are refused. If the session process exits first, `orchestrator_exited` applies as today. The handshake states (waiting, nudged, ready, no_start) go in the run's event stream and show in `tome runs show`.

### 007-2. Agent worker start handshake

**Blocked by:** 007-1

The same check covers agent workers. A tome call carrying the worker's `TOME_WORKER_ID` (with `tome ready` as the first action) proves that the worker has started. The worker bootstrap prompt gains the `tome ready` first-action instruction and lists it in its command reference. The timer starts when the worker's session is launched, and the nudge and second timeout work as they do for the orchestrator. If there's still no call, the worker is failed with reason `worker_no_start` and its session is killed, and its log is kept. The orchestrator gets the usual worker-finished nudge and decides whether to retry the worker or fail the run. The user isn't notified. Command workers aren't checked. If the worker's session process exits first, `worker_exited` applies as today. Worker handshake states go in the run's event stream alongside the orchestrator's.

### 007-3. Per-workflow `defaults.start_timeout`

**Blocked by:** 007-1

Workflows can set `defaults.start_timeout` to a duration or to `off`. The value applies to the workflow's orchestrator and, once 007-2 lands, to its agent workers. `off` turns off the check completely, so the agent is never nudged or failed for not starting. If the value is neither a duration nor `off`, `tome validate` exits `2`. `TOME_START_TIMEOUT` still overrides the value for tests.
