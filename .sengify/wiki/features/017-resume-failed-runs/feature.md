# Resume failed runs

## Description

A [[run]] that fails, or that you cancel, can be picked up again where it stopped instead of starting over:

```sh
tome run resume <id> [--start-at "<text>"] [--detach | --view] [placement flags]
```

Resuming creates a **new run linked to the old one** (`resumed_from`). The old run's status, history and logs aren't changed. A run can be resumed only if it ended as `failed` (for any reason, such as `daemon_restart`, `agent_exited` or a handshake failure) or as `cancelled`. A user can resume a run, and so can any agent, such as another workflow, by running the command. Nothing resumes a run automatically.

### What the resumed run gets

- **Workflow:** the old run's **workflow snapshot, exactly as stored**. Params and `{{run.id}}` keep the values the old run had, so paths, branches and queue names built from them still point at the old run's work. The agent's `TOME_RUN_ID` is the new run's id. Its `mode` comes from the snapshot too (feature 016).
- **A "Resuming" section in the built-in prompt** (for the agent in `single` mode, the [[orchestrator]] in `orchestrated` mode), containing:
  - the **merged step history of the whole resume chain**: each [[step]]'s latest outcome across all attempts, with its done/fail messages, plus each earlier attempt's failure reason and summary;
  - **where to start:**
    - with `--start-at`, the text the user gave, passed through unchecked. The agent works out what it means and treats the work before it as done;
    - without `--start-at`, the step that failed or was still running when the run ended. If there's none, the step after the last step that finished.
    
    The agent is told to skip steps that succeeded and to check any state the interrupted step left behind before redoing it;
  - the **adopted [[worktree]]s** (paths and branches);
  - for orchestrated runs, a **summary of the old [[worker]]s and [[group]]s** and their final status and messages.
- **Worktrees are adopted.** The old run's worktrees are re-recorded under the new run with the same paths and branches (`.tome/worktrees/<old>-<name>`, `tome/<old>/<name>`). `tome worktree create <name>` with a name the old run used returns the adopted worktree instead of making a fresh one.
- **Workers aren't relaunched.** Only the new run's agent or orchestrator starts. The orchestrator re-spawns whatever workers it still needs.

### Starting and following it

- `tome run resume` works like `tome run`. It streams the new run by default and takes `--detach`, `--view` and the placement flags (features 010, 011). Without placement flags it reuses the old run's recorded placement. It has no `--param`.
- **Concurrency:** the snapshot's `concurrency` and on-conflict rule apply exactly as they do to a new run. The resumed run is queued or refused.
- **Trigger deliveries:** if the old run was started by a topic [[trigger]] and its delivery is `failed` (parked), the delivery is re-attached to the new run. It goes back to in progress and settles when the new run finishes (feature 009).
- **Visibility:** `tome runs show` displays `resumed_from` and `resumed_as`, so you can follow a chain.
- **gc:** a resume chain is collected as one unit. That happens only when its newest run is finished and older than the age given. Then every run in the chain is removed together, and the adopted worktrees follow the newest run.

### Out of scope

- Automatic resume: on `daemon_restart`, or through a retry policy in the frontmatter.
- Changing params on resume. Start a fresh run instead.
- Resuming or restarting a run that's still running or queued.
- Using the current, edited workflow file (`--reload`). Resume always uses the snapshot.
- Relaunching the old run's workers.

## Use Cases

1. As a user, I want to resume a failed run from where it stopped, so that I don't redo steps that already succeeded.
2. As a user, I want `--start-at "<step>"` to choose where the resumed run restarts, so that I can redo an earlier step that turned out to be wrong.
3. As a user, I want runs that failed with `daemon_restart` or `agent_exited`, or that I cancelled, to be resumable, so that an interruption doesn't throw away progress.
4. As a resumed run's agent, I want the merged history of earlier attempts and the adopted worktrees in my prompt, so that I can see what's done and where the partial work is.
5. As a resumed run's orchestrator, I want a summary of the old run's workers and groups, so that I re-spawn only what's still needed.
6. As a user whose run was started by a trigger, I want resuming to re-attach the failed delivery, so that the bus settles correctly without a duplicate fresh run from `tome events retry`.
7. As an agent or another workflow, I want to call `tome run resume`, so that recovery can be scripted.
8. As a user, I want `tome runs show` to display `resumed_from` and `resumed_as`, so that I can follow a resume chain.

## Constraints

- A resume always creates a new run. Finished runs are never reopened, and their status never changes.
- The resumed run uses the old run's snapshot verbatim: the same params, `{{run.id}}` and mode.
- Where to start is guidance for the agent, not enforced by tome. That matches tome's model, where the agent decides the step order.
- A resume chain is linear: each run can be resumed at most once.
- Resume follows the same concurrency rules as `tome run`.

## Edge Cases

- **The run is succeeded, running or queued** → refused with exit `2`.
- **The run was already resumed** (e.g. `tome run resume 5` after 5 was resumed as 9) → refused with exit `2`: "run 5 was resumed as 9", with a hint to resume 9 if it also failed.
- **The resumed run fails too** → it can be resumed in turn, which extends the chain. Its prompt carries the history of the whole chain.
- **The workflow is at its concurrency limit** → the resumed run is queued or refused, as `tome run` would be.
- **An old worktree was deleted or is missing** → it isn't adopted, and the prompt says it's gone.
- **The run's project path no longer exists** → refused with exit `2`.
- **The trigger delivery was already retried or removed** → it's left alone and not re-attached.
- **The workflow file has since been edited or deleted** → no effect. The snapshot is used.
- **`--start-at` names something that isn't a step** → it's passed through as is, and the agent interprets it.
- **An orchestrated run's workers were mid-task** → they aren't relaunched. They appear in the orchestrator's summary with their last status.

## Related Entities

- [[run]]
- [[step]]
- [[workflow]]
- [[worktree]]
- [[worker]]
- [[group]]
- [[orchestrator]]
- [[trigger]]
- [[event]]
- [[daemon]]
