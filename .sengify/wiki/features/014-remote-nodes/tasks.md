# Remote nodes — Task Plan

Source feature: [feature.md](./feature.md)

## Tasks

### 014-1. Node config and SSH transport

**Blocked by:** none

Nodes can be configured under `nodes:` in the global `~/.tome/config.yaml`: names follow `[a-z0-9_-]`, `local` is reserved, and each node has `ssh`, an optional `tome` binary path and an optional `projects` map. A new hidden `tome rpc --stdio` command relays newline-delimited JSON-RPC, including streaming requests, between stdin/stdout and the local daemon, and starts the daemon detached if it isn't running. The CLI reaches a node by running `ssh <dest> <tome> rpc --stdio` in batch mode with a 10-second connect timeout, one connection per command. `daemon.ping` now returns the tome version and a protocol version. It is the first request on every connection, and the command stops (exit `1`) if the protocol versions differ, with a hint naming both versions and which side to upgrade. If the node is unreachable, or tome isn't found on it, the command exits `3` with SSH's error and a hint (`tome node check <name>`, or set `tome:`). Without `nodes:`, nothing changes.

### 014-2. Routing daemon commands with `--on`

**Blocked by:** 014-1

The global `--on <node>` flag, also read from `TOME_NODE`, sends a command's daemon requests to that node's daemon. It applies to `tome runs ls|show|logs`, `tome run cancel`, `tome publish`, `tome events …`, `tome triggers ls|fire|enable|disable`, `tome gc` and `tome daemon status|stop`. Output is the same as for a local command, with the node's name added in human output. `<node>:<id>` is accepted anywhere a run id is, and an `--on` that names a different node exits `2`. `--on local` is the same as leaving the flag out. Commands that only touch local files (`validate`, `workflow new|ls|rm`, `layout`) and agent commands (`step`, `worker`, `group`, `worktree`, `queue`, `ready`, `session move`) refuse `--on` with exit `2` and a hint. A command run inside a project acts on the node's copy of it: first the node's `projects.<name>` entry, then the same home-relative path. If neither exists on the node, the command exits `4`, naming the paths it tried and the config key to set. This also delivers `tome publish --on <node>` for orchestrators handing work to another machine.

### 014-3. `tome node add|rm|ls|check`

**Blocked by:** 014-2

`tome node add <name> <ssh-destination> [--tome <path>]` writes the config entry and then runs a check. If the check fails, the entry is kept and the failures are shown. `tome node rm` removes an entry. `tome node ls` checks every configured node in parallel and shows whether it's reachable, its tome version, whether its daemon is running, and the round-trip time. Unreachable nodes are listed with the reason, and the command exits `0`. `tome node check <name>` runs its steps in order and stops at the first failure, printing the exact fix: SSH connects without a prompt, tome is found, the protocol is compatible, the daemon is running or can start (suggesting `tome daemon install` on the node), and, inside a project, that the project maps to a directory on the node that holds `.tome/`.

### 014-4. Starting remote runs: `tome run --on`

**Blocked by:** 014-2

`tome run <name> --on <node>`, attached or `--detach`, starts an ordinary run on the node. The workflow is resolved by name from the node's copy of the project or the node's global workflows, and the command prints the path of the workflow file the node used. A workflow given as a path, or `--from caller`, is refused with exit `2`. Placement flags are passed through, and placement and backend resolve from the node's own config, so backend errors are reported as usual. When the project is a git repo on both machines, the node reports its `HEAD`, and the command warns if the commits differ or either side has uncommitted changes, then starts the run anyway. A workflow that isn't on the node exits `4`, with a hint to commit and pull it if it exists locally. If the SSH connection drops during an attached run, the run keeps going on the node, the same as closing an attached local client.

### 014-5. Watching sessions: `tome session view`

**Blocked by:** 014-4

`tome session view <run>[/<worker>]` (also with `--on` or `<node>:<id>/<worker>`) shows a run's orchestrator or worker session from this machine, using a new `session.attach_command` that returns the exact attach command, including the node's `TOME_TMUX_SOCKET`. For a remote tmux session called from cmux, it opens a local tab running `ssh -t <dest> tmux attach …`, using 010's placement flags. Called outside cmux, the attach happens in the current terminal. A remote cmux session is refused (exit `2`) with a hint to use tmux on that node. For a local session, it focuses the session's tab or pane, or prints the attach command. `tome run --on <node> --view` starts the run detached, waits for the orchestrator's handshake, and then views it.

### 014-6. Listing across nodes: `tome runs ls --nodes`

**Blocked by:** 014-2

`tome runs ls --nodes` lists runs from this machine and every configured node, queried in parallel, with a `NODE` column. It works with `--status`, `--workflow` and `--limit`, and the limit applies to each node separately. Unreachable nodes are reported as warnings on stderr, and the command still exits `0`.

### 014-7. Forwarding events across machines

**Blocked by:** 014-2

A project's `.tome/config.yaml` can list `bus.forward` rules (a topic pattern and one or more target nodes). `tome validate` checks the patterns and warns about unknown nodes. When a matching event is published, the daemon records a delivery for each target node and sends it by publishing the event to the same topic on the node's copy of the project, so the node's `on:` triggers handle it. Deliveries are durable. While a node can't be reached they stay `pending` and retry with backoff (at most 1 minute between attempts) for 24 hours, then become `failed` and send a notification. The first authentication failure is also notified, with an `IdentityFile` hint. A rule naming an unconfigured node fails straight away. `tome events show|retry|remove` treat these deliveries like any other, showing the node as `→ <node>`. On the node, the sender is `node <origin> (<original sender>)`, the depth carries over so the limit of 8 applies across machines, events are never forwarded back to the node they came from, and forwarded `tome.*` lifecycle events keep their topic and gain `node: <origin>` in the payload.
