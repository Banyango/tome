# Worker

## Definition

An agent or command that the [[orchestrator]] spawns to do part of a [[run]]'s work. Each worker runs in its own visible [[session]].

## Attributes

- name: unique within the run (`--name`, or generated as `w1`, `w2`, …)
- kind: agent (a prompt, launched through the [[harness-adapter]]) or command (a shell command)
- status: pending, running, done, failed, cancelled; with a summary
- completion:
  - agent: `tome worker done|fail`; exiting without reporting counts as failed (`worker_exited`)
  - command: exit code (0 = done)
- optional [[worktree]] on the branch `tome/<run>/<name>`
- optional [[group]] membership
- env: `TOME_RUN_ID`, `TOME_WORKER_ID`, `TOME_OUTPUT=json`, `TOME_WORKTREE`
- placement: `tome worker spawn --preset --layout --workspace --direction --size --from`, above the workflow's matching rule and `workers` block; a live worker's session can be re-placed with `tome session move <run>/<name>` _(source: [010](../features/010-session-placement/feature.md))_
- `from: caller` is for the orchestrator only: an error in a worker's rule or the `workers` block, refused by `tome worker spawn`, and skipped at levels shared with the orchestrator _(source: [011](../features/011-caller-placement/feature.md))_

## Relationships

- [[orchestrator]]: the only thing that spawns workers; there's no nesting
- [[session]]: each worker has its own session, closed after done/fail unless `--keep-open`
- [[group]]: workers join groups for fan-out and fan-in
- [[queue]]: workers push, pull and ack messages on the run's queues
- [[worktree]]: a worker may get an isolated worktree

## Sources

- features/003-primitives/feature.md
- features/010-session-placement/feature.md
- features/011-caller-placement/feature.md
