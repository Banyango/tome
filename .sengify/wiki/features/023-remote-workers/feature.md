# Remote workers

## Description

014 kept every run on one machine and left spreading a run's [[worker]]s across machines for later. This feature lets the [[orchestrator]] stay on the run's machine and send any worker (agent or command) to a configured [[node]]. For example, a build worker goes to `linux`, a test worker goes to `mini`, and both results come back to the same [[run]].

- **Ownership:** the run's [[daemon]] stays the only owner of the run and its worker records. The node's daemon hosts the worker's [[session]] and keeps only a small "hosted worker" record of it.
- **Link:** while a run has remote workers, the run's daemon holds one `ssh <node> tome rpc --stdio` link open to each node. The node relays every agent command its hosted workers call (`worker done|fail`, `queue`, `group`, `step` and the rest) up that link and returns the replies. No machine opens a listener, and the laptop never needs to accept inbound connections.
- **Choosing a node:** `tome worker spawn --node <name>`, or a default in the workflow's `workers:` block or placement rules (matched by worker name). The flag wins.
- **Code:** a remote worker always gets a [[worktree]]. tome git-pushes the run's base commit (committed state only) over SSH into the node's copy of the project, then creates `tome/<run>/<name>` there. Uncommitted local changes trigger a warning and aren't sent.
- **Results:** status and summary come back through done/fail, as they do today. When the worker ends, tome fetches its `tome/<run>/<name>` branch back into the local repo. Moving other files (binaries, reports) is up to the workflow.
- **Config:** the laptop sends the prompt, the harness name and any placement flags. The node works out the harness command, [[backend]] and layout from its own config, as `--on` does. `from: caller` is refused.
- **Node to node:** any daemon can host remote workers through its own `nodes:` config, so a run started with `--on mini` can use mini's nodes. Nothing extra is built for chaining.
- **Out of scope:** node providers as worker targets, syncing uncommitted code, and copying back artifacts other than the branch.

## Use Cases

1. As an orchestrator agent, I want to spawn a build worker on `linux` and a test worker on `mini` in the same run, so that both results land in one run.
2. As a workflow author, I want to set a default node per worker in frontmatter, so that remote placement is reviewable and repeatable.
3. As a remote worker, I want to use `tome worker done`, queues and groups the same way a local worker does, so that the workflow doesn't depend on where the worker is.
4. As an orchestrator, I want the remote worker's branch fetched back, so that I can merge or inspect its commits locally.
5. As a user, I want `tome session view <run>/<worker>` to open a remote worker's session, so that I can watch it or step in.
6. As a user, I want remote workers to fail clearly when a node can't be reached, so that the orchestrator can respawn them elsewhere.

## Constraints

- SSH only, with no listener. The node reaches the run's daemon only through the link that the run's daemon opened.
- Only nodes from `nodes:` can be targets. Provider targets are refused.
- The project must be a git repo on both machines, and the node must have a mapped copy of the project (014's mapping rules).
- Protocol versions must match: `daemon.ping` gains a hosted-worker capability.
- Workers still can't spawn other workers.

## Edge Cases

- The node fails its checks at spawn → the spawn fails with exit 3 and the fix for the first failing check.
- No project copy on the node, or it isn't a git repo → exit 4, naming the paths tried and the config key to set.
- The harness isn't configured on the node → the spawn fails with a fix.
- `--node` names a provider → exit 2.
- `from: caller` on a remote worker → refused (exit 2).
- The SSH link drops (laptop sleeps, network change, daemon restart) → remote workers fail right away with `node_unreachable`, and the orchestrator can respawn them.
- The node loses its upstream link → the node closes its hosted sessions, unless `--keep-open` was set. The worktree branch stays for inspection, and gc removes it later.
- The run is cancelled → the run's daemon tells each node to close its hosted sessions.
- A worker calls in after it has been marked failed → the call is refused.
- Branch fetch-back fails → the worker's status stands, and the run gets a warning naming the branch on the node.
- `session view` of a remote cmux session → refused, as in 014. Use tmux on the node.
- Uncommitted local changes at spawn → a warning; only the committed base is pushed.

## Related Entities

- [[worker]]
- [[node]]
- [[orchestrator]]
- [[run]]
- [[daemon]]
- [[session]]
- [[worktree]]
- [[queue]]
- [[group]]
- [[harness-adapter]]
