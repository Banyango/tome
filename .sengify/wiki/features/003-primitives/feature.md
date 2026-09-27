# Primitives: workers, worktrees, groups and queues

## Description

The [[orchestrator]] can delegate work. It spawns [[worker]]s, gives them their own git [[worktree]]s, tracks them as [[group]]s, and passes messages to them over [[queue]]s. This builds on [[run]] execution from feature 002.

### Workers

- `tome worker spawn --name <n> [--group <g>] [--worktree [--base <ref>]] [--harness <h>] [--keep-open]` starts one of two kinds of worker:
  - an **agent worker** (`--prompt` / `--prompt-file`), launched through the [[harness-adapter]];
  - a **command worker** (`-- <cmd>`).
- **Orchestrator only:** only the orchestrator can spawn workers, so a run has one orchestrator and N workers and never nests.
- **Names:** unique within the run. If `--name` is omitted, tome generates `w1`, `w2`, and so on. Spawn prints the name.
- **No cap** on how many workers run at once. The orchestrator decides how many to spawn.
- **Sessions:** each worker gets its own unfocused [[session]] on the run's [[backend]] (a tmux session or cmux workspace), titled like `tome: <wf> #<id> / <name>`.
- **Agent bootstrap:** a built-in worker preamble (how to report with `tome worker done|fail`, the queue commands, and that workers don't spawn workers), followed by the orchestrator's task prompt. The env vars are `TOME_RUN_ID`, `TOME_WORKER_ID`, `TOME_OUTPUT=json`, and `TOME_WORKTREE` when the worker has one.
- **Completion:**
  - An agent worker calls `tome worker done|fail [--summary …]`. If it exits without calling either, it's marked `failed` with reason `worker_exited`.
  - A command worker is `done` if it exits 0 and `fail` otherwise. Its summary is the exit code plus the last lines of its output.
  - Timeouts and human approval are left to the step completion feature.
- **After done/fail:** the daemon closes the worker's session (its output is already in the run log), unless it was spawned with `--keep-open`.
- `tome worker kill <name>` stops a worker. It's marked `cancelled`.

### Waiting for results

- **Commands:** `tome worker wait|status <name>` and `tome group wait|status <g>` print JSON with each worker's status, summary, branch and worktree path. The `wait` forms block until the worker or group finishes.
- **Nudges:** whenever any worker or group finishes, the daemon types a short line into the orchestrator's pane pointing at the status command. It does this even if the orchestrator is already blocked in a `wait`.

### Groups

- **Membership:**
  - `spawn --group g` adds the worker to group `g` and creates the group on first use.
  - `tome group create g [--fail-fast]` creates a group up front with options.
  - `tome group close`, or the first `group wait`, stops new members from joining.
- **Finishing:**
  - A group finishes when all its members have finished, whatever the outcome. The orchestrator decides what failures mean.
  - `--fail-fast`: the first failure finishes the group and kills the remaining members, marking them `cancelled`. Their worktrees are kept.

### Worktrees

- **Creation:**
  - `spawn --worktree` creates `<repo>/.tome/worktrees/<run>-<name>` on a new branch `tome/<run>/<name>`. It branches from `--base`, or the repo's current HEAD commit if no base is given.
  - `tome worktree create` makes a worktree that isn't tied to a worker.
- **Ignoring:** tome adds `.tome/worktrees/` to the project's `.gitignore` if it isn't already ignored.
- **Fan-in:** tome has no merge logic. It reports each worker's branch and path, and the orchestrator (or a worker it spawns) merges them with plain git.
- **Lifecycle:** worktrees are recorded per run, kept on cancel and failure, and removed by `tome gc`. gc deletes a worker's branch only if it's merged into its base. An unmerged branch is kept and listed in gc's output.

### Queues

- **Scope:** run only, found through `TOME_RUN_ID`. There are no global or cross-run queues in this feature.
- **Commands:** `tome queue push <q> <text|->`, `pull <q> [--wait [<dur>]]`, `ack <msg>`, `close <q>`, `ls`.
- **Messages:** any text up to about 1 MiB, stored in DuckDB with an id, the sender (orchestrator or worker name) and a timestamp. Larger data goes in a file, and the message carries its path.
- **Delivery (claim and ack):**
  - `pull` claims a message for the caller, and `ack` removes it.
  - If the claimer finishes or dies without acking, the message goes back on the queue.
- **Empty queue:**
  - A plain `pull` returns right away with exit code `3`.
  - `--wait` blocks until a message arrives or the duration passes.
  - On a queue that's empty and closed, `pull` returns a "closed" status.

### Run lifecycle

- **`tome run finish`** is refused while any worker is still running. The error lists those workers. The orchestrator waits for them or kills them first.
- **Cancel or `orchestrator_exited`:** worker sessions are killed, the workers are marked `cancelled`, and their worktrees are kept. The failure notification lists the workers that were cut off.
- **Daemon restart:** crash recovery kills the recorded worker sessions, and running workers are marked `failed` with reason `daemon_restart`.

### Visibility

- **Stream:** the attached `tome run` stream and its NDJSON gain worker events (spawned, started, done, failed, cancelled) and group-finished events. Queue traffic isn't streamed.
- **`tome runs show`:** lists workers (status, summary, worktree, branch) and groups. Queue state is available through `tome queue ls` and `tome query`.

### Out of scope

- The herdr backend
- Timeouts, human approval and user-defined failure conditions (feature: step completion and failure handling)
- Global and cross-run queues, and queue-message [[trigger]]s
- Merge tooling
- Workers spawning workers

## Use Cases

1. As an orchestrator, I want to spawn agent workers with a task prompt, so that I can delegate parts of a workflow to other agents.
2. As an orchestrator, I want to spawn command workers, so that tests or builds run in their own visible session and report by exit code.
3. As an orchestrator, I want to spawn workers into a named group and wait on the group, so that I can fan work out and act once it has all finished.
4. As an orchestrator, I want per-worker worktrees on their own branches, so that parallel agents don't clobber each other's changes, and I can merge their branches.
5. As an orchestrator, I want a line typed into my pane whenever a worker finishes, so that I learn about results even when I'm not blocked in a wait.
6. As an author, I want an optional fail-fast group, so that one failure stops the others from burning time.
7. As a worker agent, I want a built-in preamble and env vars, so that I know how to report and where my worktree is, whatever the harness.
8. As a worker, I want to claim and ack messages on a run's queues, so that tasks aren't lost when a worker crashes.
9. As a user, I want each worker in its own titled session, so that I can watch any worker and type into it.
10. As a user, I want `tome run` and `tome runs show` to show worker and group progress, so that I can follow a fan-out.
11. As a user, I want worktrees kept on cancel and their unmerged branches kept by gc, so that no work is lost silently.

## Constraints

- Workers are launched through the generic [[harness-adapter]], with no harness-specific features.
- Only the orchestrator can spawn workers.
- Worker, group and queue state live in DuckDB with the rest of the run state.
- Worker sessions use the run's backend (tmux or cmux).

## Edge Cases

- **Worker exits without reporting:** it's marked `failed` with reason `worker_exited`. For a command worker, the exit code decides the outcome instead.
- **`done`/`fail` reported after the worker is already final, or the process exits after `done`:** the first result stands, and a repeated report is refused.
- **Duplicate worker name, or spawning into a closed group:** exit `2`, and nothing is spawned.
- **`--worktree` outside a git repo, or a `--base` that doesn't exist:** exit `2`.
- **Uncommitted changes in the repo:** the worktree branches from the HEAD commit, without them.
- **`tome run finish` while workers are running:** refused, and the running workers are listed.
- **Run cancelled or orchestrator exits:** workers are killed and marked `cancelled`, and their worktrees are kept.
- **Fail-fast group has a failure:** the remaining members are killed and marked `cancelled`, and their worktrees are kept.
- **Daemon restarts:** worker sessions are killed, and workers are marked `failed` with reason `daemon_restart`.
- **Claimer dies holding unacked messages:** the messages go back on the queue.
- **`pull` on an empty queue:** exit `3`. With `--wait` on a queue that's empty and closed, a "closed" status.
- **Message over 1 MiB:** refused, with a hint to pass a file path instead.
- **Orchestrator's pane is gone when a nudge is sent:** the nudge is dropped silently.
- **`group wait` on a group with no members:** an error.

## Related Entities

- [[worker]]
- [[orchestrator]]
- [[session]]
- [[worktree]]
- [[group]]
- [[queue]]
- [[run]]
- [[step]]
- [[harness-adapter]]
- [[backend]]
- [[action]]
