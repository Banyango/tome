# Triggers — Task Plan

Source feature: [feature.md](./feature.md)

## Tasks

### 004-1. Trigger syntax, validation and placeholders

**Blocked by:** none

Workflow frontmatter parses triggers into typed kinds: `manual` (bare name), `file` (glob, `on:`, `debounce`, `ignore`) and `cron` (5-field expression). Each also takes `to:` (`new` default, `running`, `running-or-new`) and `params:`. `tome validate` rejects an invalid cron expression or glob, a relative glob in a global workflow, `to: running` on a file trigger, and a trigger whose `params:` leaves a required workflow param unfilled, exiting `2`. The workflow body may use `{{trigger.kind}}`, `{{trigger.paths}}`, `{{trigger.event}}`, `{{trigger.time}}` and `{{trigger.scheduled}}`. Validation accepts them, and they resolve to empty for manual runs.

### 004-2. Firing a trigger to start a run, plus `tome triggers fire`

**Blocked by:** 004-1

This is the single firing path every event source uses. A fired trigger resolves its `params:`, fills in the `{{trigger.*}}` placeholders from the event, and starts a detached run through the normal `concurrency` / `on_conflict` rules. The run records which trigger caused it, and `tome runs show` displays it. The daemon records every fire's outcome (started, rejected, or the error). `tome triggers fire <wf> [--index N] [--path p]… [--dry-run]` sends a synthetic event down this real path. `--dry-run` prints what would happen, with the resolved params, without doing it.

### 004-3. Signalling running runs (`to: running` / `running-or-new`)

**Blocked by:** 004-2

With `to: running`, the fire is delivered to every active run of the workflow. The event goes as JSON onto each run's `events` queue and is written to the run's event stream, and the daemon types a nudge line into the orchestrator's pane. If the pane is gone, the event stays on the queue and the nudge is dropped. If no run is active, nothing happens and the fire is recorded as "no target". `to: running-or-new` signals active runs if there are any and otherwise starts a new run. On a file trigger it behaves like `new`.

### 004-4. Project registry and arming

**Blocked by:** 004-2

Any tome command run inside a project registers that project with the daemon automatically. The daemon arms the triggers of every registered project plus the global workflows' triggers. Where a project workflow and a global workflow share a name, the project one wins in that project. The daemon watches workflow files and re-arms on change. A workflow that becomes invalid has its triggers disarmed and the error recorded. Projects whose directory no longer exists are dropped. Triggers are re-armed from the registry after a daemon restart. `tome triggers disable|enable` (current project or `--project <path>`) pauses or resumes a project's triggers, and the setting persists. `tome triggers ls` shows each project's armed triggers with when each last fired and the result.

### 004-5. Cron triggers

**Blocked by:** 004-4

Armed cron triggers fire through the firing path on their 5-field schedule, in local time. The event carries the scheduled and actual times. A time that passes while the daemon is stopped or the machine is asleep is skipped, not caught up.

### 004-6. File triggers

**Blocked by:** 004-4

Armed file triggers watch their globs. Project workflows resolve globs relative to the project root and start their runs there. Global workflows use absolute or `~/` paths. Created and/or modified events are filtered by `on:`. `.git/`, `.tome/` (including worktrees), gitignored paths and the trigger's `ignore:` globs are never watched. Changes within the debounce window (default 2s after the last change) become one event, and `{{trigger.paths}}` lists each path with its event type. A glob that matches nothing yet still fires for files created later. A file trigger is muted while any run of its workflow is active, so changes during that time are dropped, not queued.

### 004-7. Trigger failure notifications

**Blocked by:** 004-4

When a trigger fires but can't act, or a re-arm finds a workflow now invalid, the recorded error also goes out as a notification through the existing cmux channel, and `TOME_NOTIFY=off` turns it off. Causes include an invalid workflow, a missing param, a rejected run and a missing project directory. Refusals under `on_conflict: reject` are recorded, and show in `tome triggers ls`, but don't notify.
