# Project queues and queue triggers — Task Plan

Source feature: [feature.md](./feature.md)

## Tasks

### 009-1. Project queues: storage, addressing and core commands

**Blocked by:** none

Queues can now belong to a registered project. They're stored in DuckDB with the rest of the run state, live beyond any run, and are created by their first push. Different projects can use the same queue name. Outside a run, `tome queue push|pull|ack|ls` act on the project's queues. Inside a run they act on the run's queues as before, and `--project` switches to the project's queues. `--run <id>` still picks a run queue, and `--project` together with `--run`, or `--project` outside a registered project, exits `2`. Messages follow the run-queue limits: up to about 1 MiB, refused with the file-path hint when bigger. Each has an id, a sender (`user` when pushed from outside a run) and a timestamp, `push -` reads stdin, and messages come out first in, first out. A `pull` from a plain shell claims the message for `--lease <dur>` (default `30m`). An unacked message goes back to `pending` when its lease runs out. `ls` shows pending, claimed and failed counts, plus the run's queues when called inside a run. `close` on a project queue exits `2` with a hint to use `tome triggers disable`. Run queues behave exactly as before.

### 009-2. Message outcomes when the claiming run ends

**Blocked by:** 009-1

When a run ends, the project-queue messages it claimed get settled. If the run succeeded, any it didn't ack are acked (`done`). If it failed, was cancelled, or ended with `orchestrator_exited` or `daemon_restart`, they become `failed`. Failed messages are parked and never handed out again automatically. Each message that becomes `failed` sends a notification through the existing channel naming the queue, the message's first line and the run. `tome gc` removes `done` messages.

### 009-3. Inspecting and repairing queues: show, remove, retry

**Blocked by:** 009-2

`tome queue show <q> [--all]` lists a project queue's messages. For each one it shows the id, state, sender, age, a first-line preview and the run that claimed it. `done` messages are hidden unless `--all` is given. `tome queue remove <msg>` deletes a `pending` or `failed` message and refuses a `claimed` one. `tome queue retry <msg>` puts a `failed` message back to `pending` and refuses any other state. Both refusals exit `2`.

### 009-4. Queue trigger syntax, validation and placeholders

**Blocked by:** none

Workflow frontmatter accepts `queue: <name>` triggers, with `to:` (`new` by default, `running`, `running-or-new`) and `params:`. The workflow body may use `{{trigger.queue}}`, `{{trigger.message}}` and `{{trigger.message_id}}`, which resolve to empty on manual runs and for `to: running`. `tome validate` checks the queue name and the trigger's options. It exits `2` for a queue trigger in a global workflow, and it warns when two workflows in a project trigger on the same queue.

### 009-5. Queue triggers starting runs (`to: new`), backlog and testing

**Blocked by:** 009-1, 009-2, 009-4

An armed `to: new` queue trigger takes each `pending` message on its queue in order. It claims the message for a new detached run before the orchestrator starts, so each run gets exactly one message and no message goes to two runs. The workflow's `concurrency` limits how many of these runs go at once. Messages over the limit stay `pending` rather than becoming queued runs, and the daemon starts the next one when a run finishes. `on_conflict` doesn't apply. When two workflows share a queue, whichever has a free slot first takes the message. Messages that are already pending are picked up whenever the trigger is armed: on daemon start, when a workflow is added or edited, and on `tome triggers enable`. If the workflow is invalid, nothing is claimed and the error is recorded and notified. The run records the queue and message id that started it, and `tome runs show` displays them. `tome triggers fire <wf>` on a queue trigger takes the next pending message. `--message <text>` pushes a synthetic message first and fires through the same path. `--dry-run` reports which message would be taken and what would happen, without doing it. `tome triggers ls` shows each queue trigger's pending count and the last message it started a run for.

### 009-6. Nudging running runs (`to: running` / `running-or-new`)

**Blocked by:** 009-5

With `to: running`, a message arriving on the queue claims nothing. The daemon types a nudge naming the queue and the `tome queue pull <q> --project` command into each active run's orchestrator pane, and the message stays `pending` until an orchestrator pulls it. If no run is going, nothing happens and the message waits. With `to: running-or-new`, active runs are nudged if there are any, and otherwise a run is started as with `to: new`. `tome triggers fire --dry-run` reports "would nudge run N" for these triggers.
