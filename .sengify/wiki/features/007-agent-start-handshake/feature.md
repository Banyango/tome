# Agent start handshake

## Description

The [[daemon]] checks that every agent it launches actually starts. That means each [[orchestrator]] and each agent [[worker]]. Today, an agent that never gets its prompt leaves its [[run]] `running` forever. For example, Claude Code can auto-update on launch and drop the prompt it was given on the command line. A run stuck like that also keeps its workflow's file [[trigger]]s muted.

### Proof of start

- An agent has started once it makes **any tome call** carrying its identity:
  - the run's `TOME_RUN_ID` for an orchestrator;
  - `TOME_WORKER_ID` as well for a worker.
- **First-action ack:** both bootstrap prompts (orchestrator and worker) tell the agent to run `tome ready` before anything else.
  - `tome ready` does nothing except record the call.
  - It works out from its env vars whether the caller is the orchestrator or a worker.
  - Any other tome call counts as well.
- Command workers aren't checked, because they report by exit code.

### Timeout

- **Default:** 60s from launch.
- **Per workflow:** set with `defaults.start_timeout`. It applies to the workflow's orchestrator and agent workers, and `start_timeout: off` turns the check off.
- **Env var:** `TOME_START_TIMEOUT` overrides the timeout, for tests, in the same style as `TOME_TRIGGER_TICK_MS`.

### Recovery

1. **Nudge:** when the timeout passes with no call, the daemon types one line into the agent's pane, e.g. "tome: read `<prompt file>` and follow it". This is the same mechanism as the existing nudges. The daemon then waits another timeout.
2. **Fail:** if there's still no call, the agent is failed and its session is killed. The captured log is kept in `~/.tome/runs/<id>/`.
   - **Orchestrator:** the run fails with reason `orchestrator_no_start` and sends the usual failure [[notification]]. Because the run has ended, its file triggers are no longer muted.
   - **Agent worker:** the worker is failed with reason `worker_no_start`. The orchestrator gets the usual worker-finished nudge and decides what to do (retry or fail the run). The user isn't notified.

### Out of scope

- Relaunching the agent
- Health checks after start, such as hung-agent detection or step timeouts (feature: step completion and failure handling)
- Handshakes for command workers

## Use Cases

1. As a user, I want a triggered run whose orchestrator never got its prompt to be failed and reported to me, so that it doesn't sit `running` forever.
2. As a user, I want a stuck run to end, so that its workflow's file triggers are unmuted and new files aren't dropped.
3. As a user, I want tome to nudge a stalled agent once before failing it, so that a transient launch glitch (such as an auto-update) recovers without me.
4. As an orchestrator, I want an agent worker that never starts to be marked `worker_no_start`, so that I can retry it or fail the run instead of waiting forever.
5. As a workflow author, I want to set or turn off `start_timeout`, so that slow harnesses or long startups aren't failed by mistake.
6. As a user, I want the stuck agent's log kept, so that I can see why it didn't start.

## Constraints

- **Harness-independent:** the check relies only on tome calls and the existing pane nudge, not on reading a harness's output.
- **Bootstrap prompts:** both gain the `tome ready` first-action instruction and add it to their command references.
- **Timer start:** the timer starts when the agent's session is launched, not when a run is queued.
- **Recording:** the handshake state (waiting, nudged, ready, no_start) goes in the run's event stream. `tome runs show` shows it.

## Edge Cases

- **The agent calls tome after the nudge but before the second timeout** → it counts as started, and the run carries on normally.
- **The agent calls tome after it has been failed** → the call is refused, because the run or worker is already final.
- **The pane is gone when the nudge is due** → the nudge is skipped, and the agent is failed when the second timeout ends. If the session process exited, the existing `orchestrator_exited` / `worker_exited` handling applies instead.
- **The user is typing in the pane when the nudge is due** → the nudge line is still typed, as with existing nudges.
- **A queued run** → no timer runs until its orchestrator is launched.
- **The daemon restarts during the wait** → the run is running, so crash recovery fails it with `daemon_restart`, as today.
- **`start_timeout` is invalid (neither a duration nor `off`)** → `tome validate` exits `2`.
- **`start_timeout: off`** → there's no check, and the agent is never nudged or failed for not starting.

## Related Entities

- [[orchestrator]]
- [[worker]]
- [[run]]
- [[daemon]]
- [[harness-adapter]]
- [[session]]
- [[notification]]
- [[trigger]]
