# Primitives

Primitives are the building blocks the orchestrator uses to split work up. Each is a `tome` command; the orchestrator calls them, and you can too.

## Worker

A **worker** is an agent or a command that the orchestrator starts to do part of a run's work, in its own session.

- An **agent worker** gets a prompt and starts through a harness. It reports back with `tome worker done` or `tome worker fail`, each with a summary. If it exits without reporting, it counts as failed.
- A **command worker** runs a shell command. Exit code 0 is done and anything else is failed.

Only the orchestrator starts workers. Workers can't start workers. A worker has a name that is unique in the run, and a status: pending, running, done, failed or cancelled.

## Worktree

A **worktree** is a git worktree for a worker, so it can change files without touching your checkout or other workers. It gets its own branch, named `tome/<run>/<worker>`, which starts from a base you choose. When the worker finishes, its branch holds its commits, and the orchestrator merges the branches it wants.

## Group

A **group** tracks a set of workers together. Add workers to a group when you start them, then wait for the whole group. This is how fan-out works: start many workers, then fan in when they are all done. A group can fail fast, so the first failure cancels the other members.

## Queue

A **queue** is a named list of messages inside a run. The orchestrator and workers push messages onto it and pull them off. A message is text up to about 1 MiB. Pulling claims a message, and acking removes it; a message that was claimed but never acked goes back on the queue when the worker finishes.

Signals from outside a run, such as a trigger that signals a running run, arrive on the run's `events` queue.

See [Workers, worktrees and fan-out](../guides/workers.md).
