# File triggers while a run is active

## Description

Today a file [[trigger]] is muted while any [[run]] of its [[workflow]] is active, so changes made during a run are dropped. This feature lets each file trigger choose what happens to those changes with a new `while_running:` option.

### Syntax

```yaml
triggers:
  - file: ".sengify/wiki/features/*/feature.md"
    on: [created]
    while_running: queue   # queue (default) | parallel | mute
```

### Modes

- **`queue` (default):** a change during an active run creates a real `queued` run.
  - **Visibility:** it's listed in `tome runs` and can be cancelled with `tome run cancel`.
  - **When it starts:** once no other run of the workflow is queued ahead of it or running. That includes runs started manually, by cron, or by another trigger.
  - **Merging:** batches that arrive while it waits merge into it instead of creating more runs. Only batches from the same trigger merge. If the queued run is cancelled, the next change creates a new one.
  - **Paths:** `{{trigger.paths}}` lists each path once. A path that was both created and modified is listed as `created`. Placeholders are filled in when the run starts (feature 002), so the run sees every merged path.
- **`parallel`:** each debounced batch starts a run right away. It still goes through the workflow's `concurrency` / `on_conflict`. With `on_conflict: queue`, each batch gets its own queued run, and batches don't merge.
- **`mute`:** today's behaviour. Changes are dropped, and the file index is still updated so they don't fire later (feature 005).

### `to:` on file triggers

- File triggers now honour `to:` the way cron triggers do:
  - `to: running` signals active runs;
  - `to: running-or-new` signals active runs, or starts a run if none are active.
- `while_running` applies only to `to: new`.

### Recording and visibility

- **New fire outcomes:**
  - `queued`: a queued run was created;
  - `merged`: paths were added to an existing queued run, and the outcome names its id.
- **Where they show:** in `tome triggers ls`, and in `tome triggers fire --dry-run`, which prints e.g. "would merge into run 12".

### Crash recovery

- Queued runs now survive a [[daemon]] restart, and they start once the workflow is idle. This covers every queued run, including those from `on_conflict: queue`.
- Only running runs are still failed with reason `daemon_restart`.

### Loops

- tome adds no loop guard. A run that edits files its own trigger watches will queue or start another run.
- Authors avoid this with `ignore:`, `on: [created]`, or `while_running: mute`, and the docs say so.

### Out of scope

- Automatic loop detection or a chain limit
- Telling which run or agent wrote a file
- `while_running` for cron triggers

## Use Cases

1. As a workflow author, I want file changes made during a run to queue a follow-up run, so that nothing I add (such as a new feature file) is silently dropped.
2. As a workflow author, I want `while_running: parallel`, so that each change gets its own run right away when runs are independent.
3. As a workflow author, I want `while_running: mute`, so that workflows that edit their own watched files don't loop.
4. As a user, I want changes made during a run merged into one queued run, so that a burst of edits produces one follow-up run, not many.
5. As a user, I want the queued run visible and cancellable in `tome runs`, so that I can see what will run next and stop it.
6. As a workflow author, I want `to: running` / `running-or-new` on file triggers, so that a running workflow can react to new files mid-run.
7. As a user, I want queued runs to survive a daemon restart, so that pending work isn't lost.

## Constraints

- `while_running` is set per file trigger. There's no workflow-level setting.
- The default changes from muting to `queue`. Existing workflows that relied on muting to avoid loops must add `while_running: mute`.
- `parallel` goes through `concurrency` / `on_conflict` like any other run. `queue` waits for the workflow to be idle, whatever `concurrency` is set to.
- A queued run keeps the workflow snapshot it was queued with, like other queued runs (feature 002).

## Edge Cases

- **The trigger's queued run is cancelled, then more changes arrive** → a new queued run is created.
- **The workflow file is edited while a run is queued** → the queued run keeps its original snapshot. The triggers are re-armed as usual.
- **A path is created and then deleted before the queued run starts** → it stays in `{{trigger.paths}}`, because deletes never fire. The run has to cope with the file being missing.
- **A manual, cron or `on_conflict` run is queued ahead** → the trigger's queued run waits behind it.
- **The daemon restarts while a run is queued** → the run stays queued and starts once the workflow is idle.
- **`while_running` has an invalid value, is set on a cron trigger, or is combined with `to: running` / `running-or-new`** → `tome validate` exits `2`, and the trigger isn't armed.
- **`while_running: mute` and a run is active** → the change is dropped and the index is updated, as in 005.
- **A run edits its own watched files under `queue` or `parallel`** → another run is queued or started. It's documented, and nothing prevents it.

## Related Entities

- [[trigger]]
- [[workflow]]
- [[run]]
- [[daemon]]
- [[queue]]
- [[orchestrator]]
