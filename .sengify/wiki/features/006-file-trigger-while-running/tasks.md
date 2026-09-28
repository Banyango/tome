# File triggers while a run is active — Task Plan

Source feature: [feature.md](./feature.md)

> All tasks are done: implemented as 006-1 to 006-4 and merged into main in e76a146.

## Tasks

### 006-1. Queued runs survive a daemon restart

**Blocked by:** none

When the daemon starts, recovery no longer fails runs that are still queued. Only running runs fail with reason `daemon_restart`, as before. A queued run that survives keeps its original workflow snapshot and starts once its workflow is idle. This covers every queued run, including those from `on_conflict: queue`, not just runs queued by triggers. Pending work is no longer lost when the daemon restarts.

### 006-2. `while_running` option with `mute` and `parallel`

**Blocked by:** none

File triggers accept a new per-trigger `while_running:` key. `tome validate` exits `2` and the trigger isn't armed when the value is invalid, when the key is set on a cron trigger, or when it's combined with `to: running` / `running-or-new`. `while_running` applies only to `to: new`. `mute` is today's behaviour: changes during an active run are dropped. Once 005-1 lands, the file index is still updated so those changes don't fire later. `parallel` starts a run for each debounced batch right away. That run still goes through the workflow's `concurrency` / `on_conflict`, so with `on_conflict: queue` each batch gets its own queued run and batches don't merge. Until 006-3 lands, the default stays `mute`.

### 006-3. `queue` mode as the new default, with merging

**Blocked by:** 006-2

`while_running: queue` becomes the default. A change during an active run creates a real `queued` run, which is listed in `tome runs` and can be cancelled with `tome run cancel`. The queued run starts once no other run of the workflow is queued ahead of it or running. That includes runs started manually, by cron, by another trigger, or by `on_conflict`, whatever `concurrency` is set to. Batches that arrive while it waits merge into it, but only batches from the same trigger. `{{trigger.paths}}` lists each path once. A path that was both created and modified is listed as `created`, and placeholders are filled in when the run starts, so the run sees every merged path. If the queued run is cancelled, the next change creates a new one. There are two new fire outcomes, `queued` and `merged`; `merged` names the run the paths went into. Both show in `tome triggers ls` and in `tome triggers fire --dry-run` (e.g. "would merge into run 12"). The docs cover the change of default. They also explain that tome has no loop guard, so a run that edits files its own trigger watches will queue another run. Authors can avoid this with `ignore:`, `on: [created]`, or `while_running: mute`.

### 006-4. `to: running` / `running-or-new` on file triggers

**Blocked by:** none

File triggers honour `to:` the way cron triggers do. Validation no longer rejects `to: running` on a file trigger, and `running-or-new` is no longer silently turned into `new`. With `to: running`, a debounced batch signals the workflow's active runs and passes the changed paths as event data. With `to: running-or-new`, it signals active runs, or starts a new run if none are active. A running workflow can then react to new files mid-run.
