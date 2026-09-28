# Project message bus and topic triggers

## Description

Today a [[trigger]] reacts to one of two things, a file change or a cron time. Each trigger watches for its event itself. To reach another [[workflow]] from outside, you have to fire it by name with `tome triggers fire`, and a workflow can't react to what another run did.

This feature adds a **message bus** to each project. Anyone can publish an [[event]] to a named **topic**: the user, a [[run]]'s [[orchestrator]] or [[worker]]s, or tome itself. A workflow subscribes to topics with an **`on:` trigger**. Every workflow subscribed to a topic gets its own copy of each event published to it (fan-out). Each copy is then handled through the existing firing path: the `to:` rules, `concurrency`, and signalling running runs.

The bus covers three things:
- **Custom events:** `tome publish review.requested "…"` starts every workflow that listens for it.
- **Chaining:** tome publishes run lifecycle events, so `on: tome.run.implement.succeeded` starts a review after each successful implement run.
- **Backlogs:** deliveries are durable. A workflow with `concurrency: 1` works through the events published to its topic one at a time. This replaces the project queues this feature used to specify.

### Topics and events

- **Scope:** topics belong to a registered project (the directory holding `.tome/`, feature 004). Different projects can use the same topic name without clashing.
- **Names:** one or more dot-separated segments of `[a-z0-9_-]`, e.g. `review.requested`. Topics are created when they're first used and need no declaration. The `tome.` prefix is reserved for tome's own events. Publishing to it is refused with exit `2`.
- **Event:** an id, the topic, a payload, the sender and a timestamp. The payload is any text up to about 1 MiB, as with queue messages (feature 003). Bigger payloads are refused with a hint to pass a file path instead. The sender is `user` outside a run, `run N` (or `run N worker W`) inside one, and `tome` for built-in events.
- **Order:** events on a topic are delivered to each subscription in the order they were published.

### Subscriptions and deliveries

- **Subscription:** each `on:` trigger of an armed or disabled project workflow is a subscription. It's identified by the workflow name and the trigger's topic pattern, so editing other parts of the workflow keeps it.
- **Delivery:** when an event is published, the daemon records one delivery for each subscription whose pattern matches its topic. Deliveries are the durable part. Each one is in one of these states:

| state | meaning |
|---|---|
| `pending` | waiting for its workflow to take it |
| `claimed` | taken by a run (started for it or signalled with it) |
| `done` | handled; kept for history and removed by `tome gc` |
| `failed` | the run that claimed it failed or was cancelled; it's parked and not handed out again |

- **No subscribers:** an event published to a topic nobody subscribes to is still recorded, so `tome events show` lists it, but nothing is delivered. A subscription added later doesn't receive events published before it existed.
- **Settling:** when a run ends, the deliveries it claimed are settled:
  - If the run succeeded, they become `done`.
  - If the run failed, was cancelled, or hit `orchestrator_exited` or `daemon_restart`, they become `failed`. They aren't retried automatically, so an event that breaks its workflow can't loop. A [[notification]] goes out through the existing channel, naming the topic, the payload's first line, the workflow and the run.

### Publishing

- `tome publish <topic> <text|->` publishes an event and prints its id and the workflows it was delivered to. `-` reads the payload from stdin.
- It works outside a run and inside one. Inside a run, it publishes to the run's project.
- `--dry-run` prints which subscriptions match and what each would do ("would start a run of review", "would wait: implement at concurrency limit", "would signal run 12"), without publishing.
- The push starts the daemon if it isn't running, as other commands do.

### Topic triggers

```yaml
triggers:
  - on: review.requested
    to: new          # new (default) | running | running-or-new
    params: {base: main}
```

- **Patterns:** `*` matches exactly one segment and a trailing `**` matches one or more. For example, `tome.run.*.failed` matches any workflow's failed runs, and `review.**` matches `review.requested` and `review.done.ok`.
- **`to: new`:**
  - For each `pending` delivery, the daemon starts a detached run and claims the delivery for it before the orchestrator starts. Each run gets exactly one event.
  - **Capacity:** the workflow's `concurrency` decides how many of these runs go at once. While it's at its limit, deliveries stay `pending`. They don't become queued runs. When a run finishes, the daemon starts one for the next pending delivery. With `concurrency: 1`, events are handled strictly one at a time and in order.
  - `on_conflict` doesn't apply, because a delivery always waits for a free slot.
- **`to: running`:** the delivery is claimed by each running run of the workflow and signalled to it, as other triggers signal (feature 004). The event goes onto the run's `events` [[queue]] as JSON, and the daemon types a nudge into its orchestrator's pane. The delivery settles with the first of those runs to end. If no run is going, the fire is recorded as `no_target` and the delivery becomes `done`, as a file or cron signal with no target is dropped today.
- **`to: running-or-new`:** signal running runs if there are any, and otherwise start a run as with `new`.
- **Backlog:** a topic trigger doesn't only fire on new publishes. Deliveries that are already `pending` when it's armed (daemon start, a workflow added or edited, `tome triggers enable`) are picked up too. Nothing published while the daemon was down or triggers were disabled is lost.
- **Project workflows only:** a topic trigger in a global workflow fails validation, because a global workflow has no project bus.

### Built-in events

Tome publishes these on its own, with sender `tome` and a JSON payload:

| topic | when | payload |
|---|---|---|
| `tome.run.<workflow>.started` | a run's orchestrator has started (after the handshake, feature 007) | run id, workflow, params, cause |
| `tome.run.<workflow>.succeeded` | a run finishes with `succeeded` | run id, workflow, summary, duration |
| `tome.run.<workflow>.failed` | a run finishes with `failed`, including `orchestrator_exited` and `daemon_restart` | run id, workflow, reason, summary |
| `tome.run.<workflow>.cancelled` | a run is cancelled | run id, workflow |

- **Chaining guard:** tome has no general loop guard (see feature 004), but event chains are easy to loop by accident. Each event records its depth: `0` when published by the user, and one more than the event that started the run publishing it (including tome's lifecycle events for that run). An event deeper than `8` isn't delivered. Its publish is refused (exit `2` for `tome publish`), recorded, and notified.
- **Self-subscription:** `tome validate` warns when a workflow's `on:` pattern matches its own lifecycle events, e.g. `tome.run.*.succeeded` in any workflow.

### Event data

- **Placeholders:**
  - `{{trigger.topic}}`: the event's topic.
  - `{{trigger.payload}}`: the full payload.
  - `{{trigger.event_id}}`: the event id.
  - `{{trigger.sender}}`: who published it.
  - They're empty on manual runs, and validation accepts them. With `to: running`, the event data reaches the run through its `events` queue instead.
- **Recorded cause:** the run records the event and topic that started it. `tome runs show` displays them, and `tome events show` names the run that claimed each delivery.

### Inspecting and repairing

- `tome events ls` lists the project's topics with how many events each has had, when the last one was published, and its subscribers with their pending, claimed and failed counts.
- `tome events show <topic> [--all]` lists the topic's events with id, sender, age, a first-line payload preview and each subscriber's delivery state and run. Events whose deliveries are all `done` (or that had none) are left out unless `--all` is given.
- `tome events retry <event> [--workflow <wf>]` puts a `failed` delivery back to `pending`. It refuses any other state. `--workflow` is needed when the event has more than one failed delivery.
- `tome events remove <event> [--workflow <wf>]` drops a `pending` or `failed` delivery, for example for an event published by mistake. Without `--workflow` it drops all of the event's pending and failed deliveries. It refuses a `claimed` one.
- Refusals exit `2`.
- `tome gc` removes `done` deliveries, and events with no deliveries left.

### Testing

- `tome triggers fire <wf>` on a topic trigger takes that workflow's next `pending` delivery, exactly as the daemon would.
- `--payload <text>` fires with a synthetic event instead. It's recorded with sender `test` and delivered only to that workflow, so it goes through the same path without reaching other subscribers.
- `--dry-run` reports what would happen, as for other triggers.
- `tome validate` checks topic names and patterns, the reserved prefix and the trigger's options.

### Visibility

- `tome triggers ls` shows each topic trigger with its pending delivery count and the last event it started a run for.
- The attached `tome run` stream and its NDJSON gain an event when the run publishes, naming the topic and the workflows it was delivered to.

### Example

Implement features one at a time, then review each one:

```yaml
---
name: implement
triggers:
  - on: feature.requested
concurrency: 1
---
Implement this feature:

{{trigger.payload}}
...
```

```yaml
---
name: review
triggers:
  - on: tome.run.implement.succeeded
---
Review the change made by this run:

{{trigger.payload}}
...
```

`tome publish feature.requested "Add a --verbose flag to tome runs show"` starts an implement run. More publishes wait their turn, and each successful implement run starts a review.

### Out of scope

- Publishing file changes and cron times onto the bus. `file:` and `cron:` triggers keep their own event sources. A later feature can put them on the bus as `tome.file.*` / `tome.cron.*` topics.
- Worker and step lifecycle events (`tome.worker.*`, `tome.step.*`)
- Pulling from the bus directly (`tome events pull`). Runs get events through triggers only.
- Global (`~/.tome`) topics and topics shared between projects
- Filtering events on payload content
- Priorities, delayed events and per-event retry counts
- Taking more than one event per run (batching)
- Merge tooling (tome still leaves merging to the workflow, as in feature 003)

## Use Cases

1. As a user, I want to publish an event to a topic from any shell in the project, so that every workflow listening for it reacts without my naming them.
2. As an author, I want an `on:` trigger that starts a run per event, so that a workflow reacts to events by itself.
3. As an author, I want to subscribe to `tome.run.<workflow>.succeeded` or `.failed`, so that I can chain workflows, e.g. review after implement, or triage after any failure.
4. As an author, I want `concurrency: 1` to make a topic trigger work through events one at a time and in order, so that a backlog of changes that all merge into `main` doesn't race.
5. As an orchestrator or worker, I want `tome publish` inside a run, so that a run can hand work to other workflows.
6. As an author, I want `{{trigger.payload}}` in the workflow body, so that the run knows which event it's handling.
7. As a user, I want deliveries from failed runs parked as `failed` rather than retried, so that one bad event doesn't loop, and I can retry or remove it once I've looked.
8. As a user, I want events published while the daemon was down or triggers were disabled to be picked up later, so that nothing is silently dropped.
9. As a user, I want `tome events ls|show|retry|remove`, so that I can see what's been published, who took it, and fix failures.
10. As an author, I want `tome publish --dry-run` and `tome triggers fire --payload … --dry-run`, so that I can test subscriptions without starting real work.
11. As a user, I want a depth limit on event chains, so that workflows that trigger each other can't run forever.

## Constraints

- Events and deliveries are stored in DuckDB with the rest of the run state. They go through the daemon like other commands.
- Delivery is at most one run per delivery with `to: new`. A delivery is never handed to two new runs.
- Each subscribing workflow gets its own delivery. One workflow's backlog or failures never hold up another's.
- Topic-triggered runs are always detached, like other triggered runs (feature 004).
- Run queues and `file:` / `cron:` triggers behave exactly as before.

## Edge Cases

- **Publish while the daemon isn't running:** the publish starts the daemon. Deliveries are recorded for every subscription of the project's workflows, and they fire once the daemon has armed them.
- **Publish with no subscribers:** allowed. The event is recorded and nothing is delivered.
- **Two workflows subscribe to the same topic:** each gets its own delivery and runs independently.
- **One workflow has two `on:` triggers matching the same topic:** it gets one delivery, for the first matching trigger. `tome validate` warns.
- **Workflow is invalid when a delivery is due:** nothing is claimed and the delivery stays `pending`. The error is recorded and notified, as with other triggers (feature 004).
- **Workflow deleted, or its `on:` trigger removed:** its pending deliveries are dropped when the daemon re-arms, and `tome events show` lists them as `dropped`.
- **Run fails or is cancelled:** its claimed deliveries become `failed` and a notification is sent. `tome events retry` puts one back.
- **Daemon restarts while topic-triggered runs are going:** those runs fail with `daemon_restart` (feature 002), so their deliveries become `failed`, and tome publishes `tome.run.<wf>.failed` for them when it starts.
- **Triggers disabled for the project:** deliveries keep accumulating as `pending` and are picked up on `tome triggers enable`.
- **Chain deeper than 8:** the event isn't delivered. The publish is refused, recorded and notified.
- **Publishing to `tome.*`:** refused with exit `2`.
- **Payload over 1 MiB:** refused, with a hint to pass a file path instead (feature 003).
- **Topic trigger in a global workflow:** `tome validate` exits `2`, and the trigger isn't armed.
- **`retry` on a delivery that isn't `failed`, `remove` on a claimed one, or an ambiguous event without `--workflow`:** exit `2`.
- **The workflow file is edited while runs are going:** those runs keep their snapshot. The trigger is re-armed and picks up pending deliveries under the new definition.

## Related Entities

- [[event]]
- [[trigger]]
- [[workflow]]
- [[run]]
- [[daemon]]
- [[orchestrator]]
- [[worker]]
- [[queue]]
- [[notification]]
