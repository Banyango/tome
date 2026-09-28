# Primitives — Task Plan

Source feature: [feature.md](./feature.md)

> This feature was implemented without a task plan: the work was broken down inline as it was done. This file records that breakdown after the fact, taken from the commits, so tooling doesn't treat 003 as needing to be taskified. All tasks are done.

## Tasks

### 003-1. Store schema for workers, groups and queues, plus worktree helpers

**Blocked by:** none · **Status:** done (`ea9ff70`)

Store tables and access for workers, groups and run-scoped queues (`src/store/workers.rs`, `src/store/queues.rs`), session helpers, and git worktree helpers (`src/worktree.rs`). Recovery is updated for the new tables.

### 003-2. Workers, groups, worktrees and queues

**Blocked by:** 003-1 · **Status:** done (`94299d8`)

Daemon RPCs and the CLI for `tome worker|group|worktree|queue`. Workers get sessions on the run's backend with an exit-code wrapper. A monitor pass handles workers that have ended, and nudges are typed into the orchestrator's pane. Groups support fail-fast. `run finish` is refused while workers are running. Cancel, orchestrator exit and a daemon restart end the workers. Worker and group events stream alongside step events and show in `runs show`. The orchestrator and worker prompts document the new commands.

### 003-3. gc of worker branches

**Blocked by:** 003-2 · **Status:** done (`fcb660e`)

`gc` deletes worker branches that have been merged and keeps the unmerged ones.
