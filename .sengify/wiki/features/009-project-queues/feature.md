# Project queues and queue triggers

## Description

Today every [[queue]] belongs to one [[run]]. It only exists while that run is going, and to push onto it from outside you need the run id. This feature adds **project queues**: named queues that belong to a project and outlive runs. It also adds a **`queue:` [[trigger]]**, which starts a run for each message pushed onto a project queue.

Together, these let a user keep a backlog of work that a [[workflow]] drains one item at a time. For example, you push feature descriptions onto `features`, and each one starts a run that implements it in its own [[worktree]].

### Project queues

- **Scope:** a project queue belongs to a registered project (the directory holding `.tome/`, feature 004). Different projects can use the same queue name without clashing.
- **Creation:** a project queue is created by its first push. It needs no declaration.
- **Addressing:**
  - Outside a run (`TOME_RUN_ID` unset), `tome queue` commands inside a project act on that project's queues.
  - Inside a run, they act on the run's queues as they do today. `--project` switches to the run's project queues.
  - `--run <id>` still picks a run queue explicitly. `--project` and `--run` together are a usage error (exit `2`).
- **Messages:** the same limits as run queues (feature 003): any text up to about 1 MiB, with an id, a sender and a timestamp. The sender of a push from outside a run is `user`. `push -` reads the message from stdin.
- **Order:** first in, first out per queue.

### Message states

A project queue message is always in one of these states:

| state | meaning |
|---|---|
| `pending` | waiting to be claimed |
| `claimed` | claimed by a run (by its orchestrator, its workers, or a queue trigger on its behalf) |
| `done` | acked; it's kept for history and removed by `tome gc` |
| `failed` | the run that claimed it failed or was cancelled; it's parked and not handed out again |

- **Claim and ack:** `pull` claims a message and `ack` marks it `done`, as with run queues.
- **When the claiming run ends:**
  - If the run succeeded, any messages it claimed and didn't ack are acked.
  - If the run failed, was cancelled, or hit `orchestrator_exited` or `daemon_restart`, its claimed messages become `failed`. They aren't retried automatically, so a message that breaks its workflow can't loop.
- **Claims from outside a run:** a `pull` from a plain shell claims the message for `--lease <dur>` (default `30m`). If it isn't acked in that time, it goes back to `pending`.

### Commands

- `tome queue push <q> <text|->` pushes a message, as today.
- `tome queue pull <q> [--wait [<dur>]] [--lease <dur>]` claims a message, as today. `--lease` only applies outside a run.
- `tome queue ack <msg>` marks a message `done`, as today.
- `tome queue ls` lists the project's queues with their pending, claimed and failed counts. Inside a run, it lists the run's queues too.
- `tome queue show <q> [--all]` lists the queue's messages, with each one's id, state, sender, age, a first-line preview, and the run that claimed it. `done` messages are left out unless `--all` is given.
- `tome queue remove <msg>` deletes a `pending` or `failed` message, for example one that was pushed by mistake. Removing a `claimed` message is refused.
- `tome queue retry <msg>` puts a `failed` message back to `pending`.
- `close` stays a run-queue command. Closing a project queue is an error (exit `2`); use `tome triggers disable` to stop consuming it.

### Queue triggers

```yaml
triggers:
  - manual
  - queue: features
    to: new          # new (default) | running | running-or-new
    params: {base: main}
```

- **`to: new`:**
  - For each `pending` message on the queue, the daemon starts a detached run and claims the message for that run before the orchestrator starts. Each run gets exactly one message.
  - **Capacity:** the workflow's `concurrency` decides how many of these runs go at once. While the workflow is at its limit, messages stay `pending` on the queue. They don't become queued runs. When a run finishes, the daemon starts one for the next pending message. With `concurrency: 1`, the queue is drained strictly one at a time and in order.
  - `on_conflict` doesn't apply to queue triggers, because a message always waits for a free slot.
- **`to: running`:** the daemon doesn't claim anything. It types a nudge into each running run's orchestrator pane, e.g. "tome: message on features — `tome queue pull features --project`". The message stays `pending` until the orchestrator pulls it. If no run is going, nothing happens, and the message waits.
- **`to: running-or-new`:** if a run is going, it's nudged as with `running`. If not, a run is started as with `new`.
- **Backlog:** a queue trigger doesn't only fire on new pushes. Messages that are already `pending` when the trigger is armed (daemon start, a workflow added or edited, `tome triggers enable`) are picked up too, so nothing pushed while the daemon was down or the triggers were disabled is lost.
- **Project workflows only:** a queue trigger in a global workflow fails validation, because a global workflow has no project whose queue it would read.

### Event data

- **Placeholders:**
  - `{{trigger.queue}}`: the queue name.
  - `{{trigger.message}}`: the full message text.
  - `{{trigger.message_id}}`: the message id, for `tome queue ack`.
  - With `to: running`, or on a manual run, they're empty.
- **Recorded cause:** the run records the queue and message id that started it. `tome runs show` displays them, and `tome queue show` names the run that claimed each message.

### Testing

- `tome triggers fire <wf>` on a queue trigger takes the next `pending` message, exactly as a push would.
- `--message <text>` fires with a synthetic message instead. It's pushed onto the queue first, so it goes through the same path.
- `--dry-run` prints which message would be taken and what would happen ("would start a run for message 42", "would wait: workflow at concurrency limit", "would nudge run 12") without doing it.
- `tome validate` checks the queue name and the queue trigger's options.

### Visibility

- `tome triggers ls` shows each queue trigger with its queue's pending count and the last message it started a run for.
- A message becoming `failed` sends a [[notification]] through the existing channel, naming the queue, the message's first line and the run.

### Example

A workflow that implements features one at a time:

```yaml
---
name: implement-queue
triggers:
  - queue: features
concurrency: 1
---
Implement this feature:

{{trigger.message}}

## Implement
Spawn an agent worker with `--worktree --base main` to implement it and commit...
```

`tome queue push features "Add a --verbose flag to tome runs show"` then starts a run. More pushes wait their turn.

### Out of scope

- Global (`~/.tome`) queues and queues shared between projects
- Priorities, delayed messages and per-message retry counts
- Taking more than one message per run (batching)
- Queue-message triggers on run queues
- Merge tooling (tome still leaves merging to the workflow, as in feature 003)

## Use Cases

1. As a user, I want to push work onto a named project queue from any shell in the project, so that I can build a backlog without knowing a run id.
2. As an author, I want a `queue:` trigger that starts a run per message, so that a workflow drains a backlog by itself.
3. As an author, I want `concurrency: 1` to make a queue trigger work through messages one at a time and in order, so that changes that all merge into `main` don't race each other.
4. As an author, I want `{{trigger.message}}` in the workflow body, so that the run knows which item it's handling.
5. As a user, I want messages from failed runs parked as `failed` rather than retried, so that one bad item doesn't loop, and I can retry or remove it once I've looked.
6. As a user, I want messages pushed while the daemon was down or triggers were disabled to be picked up later, so that my backlog is never silently dropped.
7. As a user, I want `tome queue show`, `remove` and `retry`, so that I can see what's waiting, fix a mistaken push and requeue failures.
8. As an orchestrator, I want `--project` on queue commands and `to: running` nudges, so that a long-running run can also consume a project queue itself.
9. As an author, I want `tome triggers fire --message … --dry-run`, so that I can test a queue workflow without pushing real work.

## Constraints

- Project queue messages are stored in DuckDB with the rest of the run state. They go through the daemon like other queue commands.
- Delivery is at most one run per message. A message is never handed to two runs at once.
- Queue-triggered runs are always detached, like other triggered runs (feature 004).
- Run queues behave exactly as before. Nothing changes for workflows that don't use `--project` or queue triggers.

## Edge Cases

- **Push while the daemon isn't running:** the push starts the daemon, as other commands do. The trigger fires once the daemon has armed it.
- **Push with no workflow triggered by that queue:** allowed. The message stays `pending` until something pulls it.
- **Two workflows trigger on the same queue:** allowed, but each message still goes to exactly one run, whichever workflow has a free slot first. `tome validate` warns about it.
- **Workflow is invalid when a message arrives:** nothing is claimed, and the message stays `pending`. The error is recorded and notified, as with other triggers (feature 004).
- **Run fails or is cancelled:** its claimed messages become `failed` and a notification is sent. `tome queue retry` puts one back.
- **Daemon restarts while queue-triggered runs are going:** those runs fail with `daemon_restart` (feature 002), so their messages become `failed`. Pending messages are picked up when the triggers are re-armed.
- **Triggers disabled for the project:** messages keep accumulating as `pending` and are picked up on `tome triggers enable`.
- **`--project` outside a registered project, or `--project` with `--run`:** exit `2`.
- **`remove` on a claimed message, or `retry` on a message that isn't `failed`:** refused with exit `2`.
- **`close` on a project queue:** exit `2`, with a hint to use `tome triggers disable`.
- **Message over 1 MiB:** refused, with a hint to pass a file path instead (feature 003).
- **Queue trigger in a global workflow:** `tome validate` exits `2`, and the trigger isn't armed.
- **The workflow file is edited while runs are going:** those runs keep their snapshot. The trigger is re-armed and picks up pending messages under the new definition.

## Related Entities

- [[queue]]
- [[trigger]]
- [[workflow]]
- [[run]]
- [[daemon]]
- [[orchestrator]]
- [[worker]]
- [[worktree]]
- [[notification]]
