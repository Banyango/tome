---
name: implement-feature
description: Implement a planned feature, one agent and one branch per task.
mode: orchestrated
params:
  base: {type: string, default: main, description: "Branch the worktrees start from"}
triggers:
  - on: feature.planned
concurrency: 1
---
A feature has a task plan. Implement it.

Feature slug: {{trigger.payload}}

## Order
Read `features/<slug>/feature.md` and `tasks.md`. If the slug is empty, fail
the run and say to fire it with
`tome triggers fire implement-feature --payload <slug>`. Work out the order
from each task's "Blocked by" line.

## Implement
Create a group named `tasks`. A task is ready once every task it's blocked
by has finished successfully. Start each ready task as an agent worker in
the group, named `task-<N>`, on its own worktree:

    tome worker spawn --name task-<N> --group tasks --worktree --base <base> --prompt "<the task>"

`<base>` is {{params.base}} for a task that isn't blocked, and the branch of
its blocker otherwise. If it has several blockers, branch from one and tell
the worker to merge the others' branches first.

Tell each worker to read `feature.md` and `tasks.md`, implement only its
task, run the tests and fix failures, commit as `<slug>-<N>: <title>`, and
report done with a summary of any choices it made. Keep starting tasks as
their blockers finish. Don't start a task whose blocker failed.

## Finish
Don't merge anything: the branches are for review. Finish the run with each
task's branch and its worker's summary. If any task failed or was blocked,
fail the run and name them.
