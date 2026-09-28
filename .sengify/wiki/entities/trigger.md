# Trigger

## Definition

A rule in [[workflow]] frontmatter that makes the [[daemon]] react to an event, either by starting a new detached [[run]] or by signalling runs of that workflow that are already going.

## Attributes

- kinds (v1):
  - `manual`: documentation only; `tome run` always works
  - `file`: a glob, `on: [created, modified]`, `debounce`, `ignore`, `while_running`; changes are batched into one event
    - `while_running:` what a file trigger does with changes while a run of its workflow is active (`to: new` only):
      - `queue` (default): queues one real run (listed in `tome runs`, cancellable) that starts once no run of the workflow is running or queued ahead of it, whatever `concurrency` says; later batches from the same trigger merge into it, each path listed once (`created` wins over `modified`); its placeholders are filled in when it starts
      - `parallel`: each batch starts a run through `concurrency` / `on_conflict`
      - `mute`: the changes are dropped (the default before 006; workflows that relied on it must now say `while_running: mute`)
    - tome has no loop guard: a run that edits files its own trigger watches queues or starts another run. Avoid it with `ignore:`, `on: [created]`, or `while_running: mute`
  - `cron`: a 5-field expression in local time; missed times are skipped
  - `on`: a topic pattern on the project's message bus (`*` one segment, trailing `**` one or more); each subscribing workflow gets its own durable delivery of every matching [[event]]; `to: new` starts one run per delivery, limited by `concurrency`; project workflows only (feature 009)
- `to:` `new` (default), `running`, or `running-or-new`, for cron and file triggers alike; a file trigger signalling runs passes its changed paths as event data
- `params:` fills in workflow params
- event data: `{{trigger.*}}` placeholders in the body
- fire outcomes: `started`, `queued`, `merged` (names the run the paths went into), `signalled`, `no_target`, `muted`, `rejected`, `error`; shown in `tome triggers ls` and `tome triggers fire --dry-run`
- deferred: webhooks, matching agent output

## Relationships

- [[daemon]]: arms triggers for auto-registered projects and global workflows
- [[workflow]]: workflows declare their triggers in frontmatter
- [[run]]: a trigger starts runs, or signals running ones through their `events` [[queue]] and a nudge to the [[orchestrator]]'s pane
- [[notification]]: sent when a trigger fails to start a run
- [[event]]: `on:` triggers subscribe to events published on the project's bus

## Planned Features

- Triggers: may be split into file triggers, message triggers, and matching agent output _(source: .sengify/sources/feature-set-2026-09-26.md)_

## Sources

- intent.md
- .sengify/sources/interview-2026-09-26.md
- .sengify/sources/feature-set-2026-09-26.md
- features/004-triggers/feature.md
- features/009-message-bus/feature.md
