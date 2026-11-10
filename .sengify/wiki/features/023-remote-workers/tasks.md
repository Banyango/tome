# Remote workers — Task Plan

Source feature: [feature.md](./feature.md)

Context: everything from [014](../014-remote-nodes/tasks.md) is already built (SSH transport, `--on` routing, `tome node check`, project mapping, `tome session view`), so it's context here, not a blocker. Each task brings its own tests, using fake nodes like the `nodes` integration tests do.

## Tasks

### 023-1. Remote command worker over a node link

**Blocked by:** none

`tome worker spawn --node <name>` runs a command worker on a configured node. While the run has remote workers, the run's daemon keeps one `ssh <node> tome rpc --stdio` link open to each node. The node's daemon hosts the worker's session and keeps a small hosted-worker record of it. `worker done|fail` and the command's exit code go back up the link to the run's daemon, which stays the only owner of the worker record. `daemon.ping` reports the hosted-worker capability. The spawn is refused with exit 2 when `--node` names a provider or an unknown node, or when the node doesn't support hosted workers. Until 023-2 lands, the worker runs in the node's mapped copy of the project without a worktree.

### 023-2. Code on remote workers: push base, worktree, fetch back

**Blocked by:** 023-1

A remote worker always gets a worktree. At spawn, tome git-pushes the run's committed base over SSH into the node's copy of the project and creates `tome/<run>/<name>` there. If there are uncommitted local changes, tome warns and sends only the committed base. When the worker ends, its branch is fetched back into the local repo. If the node has no project copy or the copy isn't a git repo, the spawn fails with exit 4, naming the paths it tried and the config key to set. If the fetch-back fails, the worker's status stands and the run gets a warning naming the branch on the node.

### 023-3. Remote agent workers and the full agent command relay

**Blocked by:** 023-1

Agent workers run on nodes. The laptop sends the prompt, the harness name and any placement flags, and the node works out the harness command, backend and layout from its own config. The node relays every agent command its hosted workers call (`worker done|fail`, `queue`, `group`, `step`, blocked reporting and the rest) and returns the replies, so a remote worker behaves the same as a local one. `from: caller` is refused with exit 2. A harness that isn't configured on the node fails the spawn with a fix. A call from a worker that's already marked failed is refused.

### 023-4. Default node in workflow frontmatter

**Blocked by:** 023-1

Workflow authors can set a default `node:` per worker in the `workers:` block or in placement rules, matched by worker name. `--node` overrides it. `tome validate` reports unknown nodes and provider targets. User docs for workflows and nodes cover the key.

### 023-5. Failure, cancel and cleanup

**Blocked by:** 023-1

When the SSH link drops (laptop sleep, network change, daemon restart), the run's remote workers fail right away with `node_unreachable`, so the orchestrator can respawn them elsewhere. A node that loses its upstream link closes its hosted sessions, unless `--keep-open` was set. The worktree branch stays for inspection, and gc removes hosted records and leftover branches later. Cancelling the run tells each node to close its hosted sessions. A node that fails its checks at spawn fails the spawn with exit 3 and the fix for the first failing check.

### 023-6. Viewing remote worker sessions

**Blocked by:** 023-1

`tome session view <run>/<worker>` opens a remote worker's session on its node over `ssh -t` (tmux), reusing [014-5](../014-remote-nodes/tasks.md#014-5-watching-sessions-tome-session-view). Remote cmux sessions are refused, as in 014.
