# Resume failed runs — Task Plan

Source feature: [feature.md](./feature.md)

## Tasks

### 017-1. Resume a failed or cancelled run

**Blocked by:** none

`tome run resume <id>` creates a new run linked to the old one through `resumed_from`, using the old run's workflow snapshot exactly as stored (same params, `{{run.id}}` and mode). It works only for runs that ended as `failed` (any reason) or `cancelled`. It is refused with exit `2` when the run is succeeded, running or queued, when it was already resumed ("run 5 was resumed as 9", with a hint to resume 9), or when the project path no longer exists. It streams the new run by default and takes `--detach`, `--view` and the placement flags, reusing the old run's recorded placement when none are given. It has no `--param`. The snapshot's concurrency and on-conflict rule apply as for `tome run`. A resumed run that fails can itself be resumed, which extends the chain. Agents and other workflows can call the command.

### 017-2. Show resume chains in `tome runs show`

**Blocked by:** 017-1

`tome runs show` displays `resumed_from` and `resumed_as`, so a user can follow a resume chain in both directions.

### 017-3. Give the resumed agent a "Resuming" prompt section

**Blocked by:** 017-1

The built-in prompt (the agent in `single` mode, the orchestrator in `orchestrated` mode) gets a "Resuming" section. It contains the merged step history of the whole chain: each step's latest outcome with its done/fail messages, plus each earlier attempt's failure reason and summary. It also says where to start. With `--from`, that is the user's text passed through unchecked. Without it, it is the step that failed or was still running, or else the step after the last finished one. The agent is told to skip steps that succeeded and to check any state the interrupted step left behind before redoing it.

### 017-4. Adopt the old run's worktrees

**Blocked by:** 017-1, 017-3

The old run's worktrees are re-recorded under the new run with the same paths and branches. `tome worktree create <name>` with a name the old run used returns the adopted worktree instead of making a new one. A worktree that was deleted or is missing isn't adopted, and the Resuming section says it's gone. The adopted worktrees (paths and branches) are listed in the Resuming section.

### 017-5. Summarize old workers and groups for a resumed orchestrator

**Blocked by:** 017-3

For orchestrated runs, the Resuming section includes a summary of the old run's workers and groups with their final status and messages, including workers that were mid-task. Workers are not relaunched. The orchestrator re-spawns only the ones it still needs.

### 017-6. Re-attach a failed trigger delivery on resume

**Blocked by:** 017-1

If the old run was started by a topic trigger and its delivery is `failed` (parked), resuming re-attaches the delivery to the new run. It goes back to in progress and settles when the new run finishes, so there is no duplicate fresh run from `tome events retry`. A delivery that was already retried or removed is left alone.

### 017-7. Collect resume chains as one unit in gc

**Blocked by:** 017-1, 017-4

gc treats a resume chain as one unit. It collects the chain only when its newest run is finished and older than the given age, and then removes every run in the chain together. The adopted worktrees follow the newest run.

### 017-8. Document resume and update agent guidance

**Blocked by:** 017-1, 017-3

Update `docs/src` for `tome run resume`, its flags and the `runs show` fields. Update the tome skill and plugin guidance so agents and workflows know they can script recovery by resuming a run.
