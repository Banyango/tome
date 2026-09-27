# Triggers

## Description

[[trigger]]s are rules in [[workflow]] frontmatter that let the [[daemon]] react to events. A trigger can start a new detached [[run]] or signal runs that are already going. This feature has two event sources: **file changes** and **cron schedules**.

### Syntax

Each trigger is a mapping keyed by its kind, with its options beside it. `manual` stays a bare name.

```yaml
triggers:
  - manual
  - file: "specs/**/*.md"
    on: [created, modified]
    debounce: 2s
    ignore: ["specs/drafts/**"]
  - cron: "0 9 * * 1-5"
    to: running-or-new
    params: {base: main}
```

- `manual` is documentation only. `tome run` always works, whether or not it's listed.

### Starting or signalling (`to:`)

- `to: new` (default): start a detached run.
- `to: running`: signal every running run of the workflow. If none are running, nothing happens, and the fire is recorded as "no target".
- `to: running-or-new`: signal running runs if there are any, and start a run otherwise.
- **Signal delivery:**
  - The event goes as JSON onto the run's `events` [[queue]] (claim and ack, from feature 003).
  - The daemon types a nudge line into the [[orchestrator]]'s pane, e.g. "tome: trigger file fired — `tome queue pull events`".
  - Signals are recorded in the run's event stream.

### Event data

- **Placeholders:** the workflow body can use `{{trigger.kind}}`, `{{trigger.paths}}` (each path with its event type), `{{trigger.event}}`, `{{trigger.time}}` and `{{trigger.scheduled}}`. They're empty for manual runs, and validation accepts them.
- **Params:** the trigger's `params:` fills in the workflow's params. A required param that's missing fails validation when the workflow loads, not when the trigger fires.
- **Recorded cause:** the run records which trigger started it, and `tome runs show` displays it.

### File triggers

- **Paths:**
  - In a project workflow, globs are relative to the project root (the directory holding `.tome/`), and runs start there.
  - In a global workflow, file triggers must use absolute or `~/` paths. A relative glob fails validation.
- `on:` is `created`, `modified`, or both (default: both).
- **Never watched:** `.git/`, `.tome/` (including [[worktree]]s), gitignored paths, and the trigger's `ignore:` globs.
- **Batching:** changes within the debounce window (default 2s after the last change) become **one** event, and `{{trigger.paths}}` lists them all.
- **Muting:** a file trigger is muted while any run of its workflow is active. Changes made during that time are dropped, not queued. This stops agents that edit watched files from looping.
  - `to: running` on a file trigger is therefore a validation error.
  - `to: running-or-new` on a file trigger behaves like `new`.

### Cron triggers

- Standard 5-field cron expressions, in local time.
- **Missed times are skipped:** a time that passes while the daemon is stopped or the machine is asleep doesn't fire later.

### Arming

- **Registration:** any tome command run inside a project registers that project with the daemon automatically. The daemon arms the triggers of every registered project, plus the global workflows' triggers.
- **Reloading:** the daemon watches workflow files and re-arms their triggers when they change.
- **Commands:**
  - `tome triggers disable|enable` (current project, or `--project <path>`) pauses or resumes a project's triggers, and the setting sticks.
  - `tome triggers ls` shows the armed triggers for each project, with when each last fired and the result.
- **Missing projects:** a project whose directory no longer exists is dropped automatically.

### Testing

- `tome triggers fire <wf> [--index N] [--path p]… [--dry-run]` sends a synthetic event down the real path: the `to:` rules, muting, concurrency and placeholders.
- `--dry-run` prints what would happen (start a run, or signal which runs, with the resolved params) without doing it.
- `tome validate` checks trigger syntax, cron expressions and globs.

### Errors

- **When it happens:** a trigger fires but can't act. The workflow is now invalid, a param is missing, `on_conflict: reject` refuses the run, or the project directory is gone.
- **What the user sees:** the daemon records the error, `tome triggers ls` shows it as the last result, and a [[notification]] goes out through the existing channel (cmux; `TOME_NOTIFY=off` turns it off).
- **`on_conflict: reject`:** the refusal is recorded but doesn't notify, since that's the configured behaviour.

### Out of scope

- Webhook triggers
- Matching agent output
- Global or cross-run queue triggers
- Catching up on missed cron times
- Notification channels beyond what already exists (feature: notifications and human approval)

## Use Cases

1. As an author, I want a workflow to start when matching files are created or edited, so that work like "implement new specs" kicks off by itself.
2. As an author, I want cron triggers, so that recurring jobs (a nightly review, a weekday triage) run unattended.
3. As an author, I want `to: running` / `running-or-new`, so that an event reaches a run that's already going instead of starting a duplicate.
4. As an orchestrator, I want trigger events on my `events` queue plus a pane nudge, so that I can react mid-run without losing events.
5. As an author, I want `{{trigger.*}}` placeholders and per-trigger `params:`, so that a triggered run knows what caused it.
6. As a user, I want a burst of file changes batched into one event, so that a checkout or refactor doesn't start ten runs.
7. As a user, I want file triggers muted while their workflow is running, so that agents editing watched files don't loop.
8. As a user, I want projects registered automatically and a way to disable them, so that triggers work without setup and I can switch them off.
9. As an author (or authoring agent), I want `tome triggers fire --dry-run`, so that I can test a trigger without waiting for a real event.
10. As a user, I want to be notified when a trigger fails to start a run, so that broken automations don't fail silently.

## Constraints

- Runs started by a trigger are always detached (feature 002).
- One daemon per user watches every registered project.
- Triggered runs go through `concurrency` / `on_conflict` like any other run.
- The workflow body is still never parsed. Triggers live only in the frontmatter.

## Edge Cases

- **Workflow edited while armed:** it's re-read and re-armed. If it's now invalid, its triggers are disarmed and the error is recorded (and notified).
- **Project and global workflow share a name:** the project one wins in that project (feature 001), and only its triggers are armed there.
- **`to: running` with no active run:** nothing happens, and the fire is recorded as "no target".
- **Several runs active:** all of them are signalled.
- **Orchestrator's pane is gone when a signal arrives:** the event stays on the queue, and the nudge is dropped.
- **File changes while the workflow's run is active:** dropped (muted).
- **Daemon restarts:** triggers are re-armed from the registry, and cron times missed while it was down are skipped.
- **Glob matches nothing yet:** allowed. Files created later still fire it.
- **Invalid cron expression or glob, relative glob in a global workflow, or `to: running` on a file trigger:** `tome validate` exits `2`, and the trigger isn't armed.
- **Registered project directory deleted:** the project is dropped from the registry.
- **Trigger can't start a run:** recorded and notified, except rejections under `on_conflict: reject`, which are recorded only.

## Related Entities

- [[trigger]]
- [[workflow]]
- [[daemon]]
- [[run]]
- [[orchestrator]]
- [[queue]]
- [[notification]]
- [[worktree]]
