---
name: fan-in-worktrees
description: Do independent tasks on separate branches, then merge the branches.
mode: orchestrated
params:
  tasks: {type: string, description: "One task per line"}
  base: {type: string, default: main}
---
Do each of these tasks on its own branch, then merge the results.

{{params.tasks}}

## Implement
Create a group `tasks`. For each task, spawn an agent worker in it with its
own worktree, branching from {{params.base}}:

    tome worker spawn --name task-<n> --group tasks --worktree --base {{params.base}} --prompt "<the task>"

Tell each worker to commit its work to its branch before it reports done.
Wait for the group.

## Merge
Merge the branch of every worker that succeeded into the current branch, one
at a time, and resolve conflicts as they come. Run the project's tests after
the last merge. Fail the run if a worker failed, or if the tests fail, and
say which.
