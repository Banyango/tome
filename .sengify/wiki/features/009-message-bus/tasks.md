# Project message bus and topic triggers — Task Plan

Source feature: [feature.md](./feature.md)

## Tasks

### 009-1. Topic trigger syntax, validation and placeholders

**Blocked by:** none

Workflow frontmatter accepts `on: <pattern>` triggers, with `to:` (`new` by default, `running`, `running-or-new`) and `params:`. Topic names are dot-separated segments of `[a-z0-9_-]`. In patterns, `*` matches exactly one segment and a trailing `**` matches one or more. The workflow body may use `{{trigger.topic}}`, `{{trigger.payload}}`, `{{trigger.event_id}}` and `{{trigger.sender}}`, which resolve to empty on manual runs. `tome validate` checks names and patterns and the trigger's options. It exits `2` for a topic trigger in a global workflow. It warns when a workflow has two `on:` triggers matching the same topic, and when a workflow's pattern matches its own `tome.run.<wf>.*` events.

### 009-2. Events, deliveries and `tome publish`

**Blocked by:** 009-1

Events and deliveries are stored in DuckDB, per project. `tome publish <topic> <text|->` works inside and outside a run. It records an event with an id, the topic, the payload (up to about 1 MiB, refused with the file-path hint when bigger), the sender (`user`, `run N`, `run N worker W`) and a timestamp. For each subscription of the project's workflows whose pattern matches, it records one `pending` delivery, whether the project's triggers are enabled or not. Events with no match are recorded with no deliveries. Publishing to `tome.*` exits `2`. The command prints the event id and the workflows it was delivered to. A publish from inside a run appears in that run's stream and NDJSON. `--dry-run` prints the matching subscriptions without publishing. When a workflow is deleted or its `on:` trigger removed, re-arming drops its pending deliveries and marks them `dropped`.

### 009-3. Topic triggers starting runs (`to: new`) and backlog

**Blocked by:** 009-2

An armed `to: new` topic trigger takes each `pending` delivery of its subscription in publish order. It claims the delivery for a new detached run before the orchestrator starts, so each run gets exactly one event and no delivery goes to two runs. The workflow's `concurrency` limits how many of these runs go at once. Deliveries over the limit stay `pending` rather than becoming queued runs, and the daemon starts the next one when a run finishes. `on_conflict` doesn't apply. Pending deliveries are picked up whenever the trigger is armed: on daemon start, when a workflow is added or edited, and on `tome triggers enable`. If the workflow is invalid, nothing is claimed and the error is recorded and notified. The run records the event and topic that started it, and `tome runs show` displays them. `tome publish --dry-run` now reports what each subscription would do.

### 009-4. Settling deliveries when a run ends

**Blocked by:** 009-3

When a run ends, the deliveries it claimed are settled. If the run succeeded they become `done`. If it failed, was cancelled, or ended with `orchestrator_exited` or `daemon_restart`, they become `failed` and are never handed out again automatically. Each delivery that becomes `failed` sends a notification through the existing channel, naming the topic, the payload's first line, the workflow and the run. `tome gc` removes `done` deliveries and events with no deliveries left.

### 009-5. Inspecting, repairing and testing

**Blocked by:** 009-4

`tome events ls` lists the project's topics with event counts, the time of the last publish and each subscriber's pending, claimed and failed counts. `tome events show <topic> [--all]` lists events with id, sender, age, a first-line preview and each subscriber's delivery state and run, hiding fully settled events unless `--all` is given. `tome events retry <event> [--workflow <wf>]` puts a `failed` delivery back to `pending`. `tome events remove <event> [--workflow <wf>]` drops `pending`/`failed` deliveries. Both refuse other states, or an ambiguous event without `--workflow`, with exit `2`. `tome triggers fire <wf>` on a topic trigger takes the workflow's next pending delivery. `--payload <text>` fires with a synthetic event (sender `test`) delivered only to that workflow, and `--dry-run` reports the outcome without acting. `tome triggers ls` shows each topic trigger's pending count and the last event it started a run for.

### 009-6. Signalling running runs (`to: running` / `running-or-new`)

**Blocked by:** 009-3

With `to: running`, a delivery is claimed by each running run of the workflow and signalled to it. The event goes as JSON onto the run's `events` queue, and the daemon types a nudge into its orchestrator's pane. The delivery settles with the first of those runs to end. If no run is going, the fire is recorded as `no_target` and the delivery becomes `done`. With `to: running-or-new`, running runs are signalled if there are any, and otherwise a run is started as with `to: new`. `tome triggers fire --dry-run` and `tome publish --dry-run` report "would signal run N" for these triggers.

### 009-7. Built-in run lifecycle events and the chain guard

**Blocked by:** 009-4

Tome publishes `tome.run.<workflow>.started` (after the start handshake), `.succeeded`, `.failed` (including `orchestrator_exited`, and `daemon_restart` for runs recovered on daemon start) and `.cancelled`, with sender `tome` and a JSON payload: run id, workflow, and params/cause, summary/duration, or reason as fits. They go through the same delivery path as user events. Every event records its depth: `0` from the user, and one more than the event that started the publishing run, including that run's lifecycle events. An event deeper than `8` isn't delivered. Its publish is refused (exit `2` for `tome publish`), recorded and notified.
