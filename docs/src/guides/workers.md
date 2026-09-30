# Workers, worktrees and fan-out

The orchestrator does the work of a run by starting **workers**. This guide covers the commands it uses, and how to write a workflow that uses them. You don't usually run these yourself. You describe what you want in the workflow body, and the orchestrator knows the commands. The examples on this page are files under `docs/examples/` that CI validates.

## Workers

```sh
# an agent worker: gets a prompt and reports back
tome worker spawn --name reviewer --prompt "Review src/lib.rs and report what you find"

# a command worker: everything after `--` is the command
tome worker spawn --name tests -- cargo test
```

An agent worker starts through a harness (`--harness` picks one, otherwise the workflow's, otherwise `claude`). It runs in its own session, and it finishes by calling `tome worker done --summary "..."` or `tome worker fail --summary "..."`. If it exits without doing either, it is marked failed. A command worker is done when it exits 0, and its summary is the exit code and the last lines of output.

Only the orchestrator starts workers. Workers can't start workers, so if a task needs splitting, a worker says so in its summary and the orchestrator decides.

Then:

```sh
tome worker status [name]    # status, summary, branch and worktree
tome worker wait <name>      # block until it finishes, then print its status
tome worker kill <name>      # stop it; it is marked cancelled
```

Sessions are closed when a worker finishes. `--keep-open` keeps one open so you can read the output.

Workers get these environment variables: `TOME_RUN_ID`, `TOME_WORKER_ID`, `TOME_OUTPUT=json` and, with a worktree, `TOME_WORKTREE`. That is why the commands above need no ids.

Here is a workflow that runs the tests as a command worker and starts an agent only if they fail:

```markdown title=.tome/workflows/test-then-fix.md
{{#include ../../examples/command-worker.md}}
```

## Worktrees

A worker started with `--worktree` gets its own git worktree of the project, on a new branch named `tome/<run>/<name>`. It starts from `--base` (a branch or commit), or from the current `HEAD` commit. Uncommitted changes in your checkout are not included.

```sh
tome worker spawn --name task-1 --worktree --base main --prompt "Add a --verbose flag"
```

`--branch` names the branch instead, to follow your team's convention or to open a pull request from it. The branch must not exist yet, so put something unique in the name, such as a task id:

```sh
tome worker spawn --name t004-renderer --worktree --base main --branch task/004-renderer --prompt "..."
```

Say how to name branches in the workflow body, and the orchestrator passes `--branch` for you.

The worker's working directory is the worktree, and it commits its changes to that branch. tome doesn't merge anything for you. The orchestrator merges the branches with git, which is why prompts should tell workers to commit before they report done.

`tome worktree create <name> [--base <ref>] [--branch <name>]` makes a worktree that isn't tied to a worker.

Worktrees stay after a run ends, so you can look at the branches. [`tome gc`](operating.md#cleaning-up) removes them with old runs, and deletes a branch only if it was merged into its base.

## Fan-out and fan-in

**Fan-out** is starting many workers at once. **Fan-in** is waiting for them and combining what they did. A **group** ties the two together:

```sh
tome group create reviews --fail-fast     # optional; a group is also created on first use
tome worker spawn --group reviews --name review-1 --prompt "..."
tome worker spawn --group reviews --name review-2 --prompt "..."
tome group wait reviews                   # closes the group and blocks until every member is finished
tome group status reviews                 # every member's result
```

`--fail-fast` makes the first failure cancel the other members. `tome group close` stops a group taking new members.

Example: one reviewer per changed file, then one report.

```markdown title=.tome/workflows/fan-out.md
{{#include ../../examples/fan-out.md}}
```

When workers change code, give each its own worktree, then merge in a final step:

```markdown title=.tome/workflows/fan-in-worktrees.md
{{#include ../../examples/fan-in-worktrees.md}}
```

## Queues

Use a queue when workers and the orchestrator need to pass work around while they run, instead of at the end. A queue belongs to the project, so it also works from your shell, and a run can leave messages for the next one.

```sh
tome queue push tasks "refactor the parser"     # add a message (`-` reads stdin)
tome queue pull tasks --wait 30s                # claim one; exit 3 if there is none
tome queue peek tasks                           # look at what is waiting, claiming nothing
tome queue ack <id>                             # remove it once handled
tome queue close tasks                          # no more messages; pulls report `closed` when drained
tome queue ls                                   # the project's queues and their counts
```

Outside a run, `tome queue` uses the project of the directory you are in. Inside a run it uses the run's project.

A message is text up to about 1 MiB. Put bigger data in a file and send its path. A claimed message that is never acked goes back on the queue when the worker that claimed it finishes, so another worker can pick it up.

A worker reports once, and its session closes when it does, so a worker only works through a queue if its prompt tells it to loop. Say it in the prompt: pull, do the item, ack, and pull again until `pull` reports `closed`, then report once. `pull` exits 3 in two cases, and the JSON `status` tells them apart: `empty` means nothing is there yet, and `closed` means nothing more will come. That is why the orchestrator has to close the queue after its last push. A closed queue stays closed for good, so a workflow that pushes more work in a later round needs a new queue name for that round. Queues outlive runs, so pick names that won't collide with another run's, such as `tasks-<round>`. Workflows that don't need to stop workers can skip `close` and have them stop on `empty` instead.

If every item gets its own worker, you don't need a queue. Put the item in the worker's prompt.

## Limits and tips

- A worker's summary is the main channel back to the orchestrator. Ask for a useful one.
- There is no limit on how many workers run at once. Each agent worker is a full harness session, so say in the workflow how many your machine can take, for example "never have more than {{params.workers}} workers running", with a param that defaults to 1 or 2.
- Worker names must be unique in a run. Without `--name`, they are `w1`, `w2` and so on. A name shows in the worker's tab title, in `tome runs show` and in its branch, so say in the workflow how to name workers, for example "name each worker `t<id>-<short title>`".
