# Trigger

## Definition

A rule in [[workflow]] frontmatter that makes the [[daemon]] react to an event, either by starting a new detached [[run]] or by signalling runs of that workflow that are already going.

## Attributes

- kinds (v1):
  - `manual`: documentation only; `tome run` always works
  - `file`: a glob, `on: [created, modified]`, `debounce`, `ignore`, `while_running`; changes are batched into one event
  - `while_running:` what a file trigger does with changes while a run of its workflow is active (`to: new` only): `mute` (default; dropped) or `parallel` (each batch starts a run through `concurrency` / `on_conflict`)
  - `cron`: a 5-field expression in local time; missed times are skipped
- `to:` `new` (default), `running`, or `running-or-new`
- `params:` fills in workflow params
- event data: `{{trigger.*}}` placeholders in the body
- deferred: webhooks, matching agent output, queue-message triggers

## Relationships

- [[daemon]]: arms triggers for auto-registered projects and global workflows
- [[workflow]]: workflows declare their triggers in frontmatter
- [[run]]: a trigger starts runs, or signals running ones through their `events` [[queue]] and a nudge to the [[orchestrator]]'s pane
- [[notification]]: sent when a trigger fails to start a run

## Planned Features

- Triggers: may be split into file triggers, message triggers, and matching agent output _(source: .sengify/sources/feature-set-2026-09-26.md)_

## Sources

- intent.md
- .sengify/sources/interview-2026-09-26.md
- .sengify/sources/feature-set-2026-09-26.md
- features/004-triggers/feature.md
