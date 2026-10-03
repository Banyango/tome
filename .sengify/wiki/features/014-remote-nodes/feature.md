# Remote nodes

## Description

Today tome runs on one machine. The CLI talks to the [[daemon]] over a Unix socket in `~/.tome`, and the daemon opens every [[session]] with the local tmux or cmux. To run a workflow on another computer, such as a desktop with a better GPU or a headless box that runs nightly work, you have to ssh in and start it there by hand. Workflows on two machines can't react to each other.

This feature adds **nodes**: other machines with tome installed that you can reach over SSH. Each node runs its own daemon, which owns its own runs, sessions, worktrees and run store. Nothing is shared or synced between daemons. This feature only lets you:

- **Drive a node from here:** any command that talks to the daemon can be sent to a node's daemon with `--on <node>`, e.g. `tome run nightly --on mini`.
- **Watch a node's agents from here:** `tome session view` opens a local tab attached to a remote session.
- **Chain workflows across machines:** a project can forward bus [[event]]s (feature 009) to a node, so a run finishing here can start a workflow there.

A [[run]] stays on the machine that started it, from start to finish. Spreading one run's [[worker]]s across machines is a later feature.

### Nodes

Nodes are configured in the global `~/.tome/config.yaml`. They aren't set per project, because hosts and SSH access belong to the user, not the repo.

```yaml
nodes:
  mini:
    ssh: kyle@mac-mini.local      # anything `ssh` accepts, including a Host alias from ~/.ssh/config
    tome: ~/.cargo/bin/tome       # optional; the tome binary on the node (default: `tome` on the node's PATH)
    projects:                     # optional; where this machine's projects are on the node
      tome-cli: ~/src/tome-cli
```

- **Names:** `[a-z0-9_-]`, unique. `local` is reserved and means this machine.
- **`tome node add <name> <ssh-destination> [--tome <path>]`** writes the entry and then runs `tome node check` on it. If the check fails, the entry is still written and the failures are shown. **`tome node rm <name>`** removes it.
- **`tome node ls`** lists the configured nodes. For each one it shows whether the node is reachable, its tome version, whether its daemon is running, and the round-trip time. It checks the nodes in parallel, with each check limited by the connect timeout. Unreachable nodes are listed with the reason, and the command still exits `0`.
- **`tome node check <name>`** runs the steps one at a time and stops at the first one that fails, printing a fix for it:
  1. SSH connects without prompting.
  2. tome is found on the node.
  3. The node's protocol version is compatible.
  4. The node's daemon is running or can be started.
  5. Run inside a project, it also checks that the project maps to a directory on the node that holds `.tome/`.

### Transport

- **SSH only.** tome opens no network port and runs no listener. Reaching a node means running `ssh <dest> <tome> rpc --stdio` in batch mode (no password or host-key prompts) with a 10-second connect timeout. Authentication, keys and host trust come from the user's SSH setup.
- **`tome rpc --stdio`** is a new hidden command. It relays newline-delimited JSON-RPC between stdin/stdout and the local daemon's socket. Like other commands, it starts the daemon first if it isn't running. Streaming requests (attached runs, `run.watch`) are relayed line by line.
- **One connection per command.** Each CLI command opens its own SSH connection. Users who want faster repeat calls can turn on `ControlMaster` in their SSH config, and tome doesn't manage it.
- **Versions:** the first request on each connection is `daemon.ping`, which now returns the tome version and a protocol version. If the protocol versions differ, the command stops before sending anything else. The hint names both versions and says which side to upgrade.

### Driving a node: `--on`

- **`--on <node>`** is a global flag, also read from `TOME_NODE`. It sends the command's daemon requests to the node instead of the local daemon. The output looks the same as for a local run, with the node's name added in human output (`run 12 on mini`).
- **Commands it applies to:** every command that talks to the daemon. These include `tome run` (attached or `--detach`), `tome run cancel`, `tome runs ls|show|logs`, `tome publish`, `tome events …`, `tome triggers ls|fire|enable|disable`, `tome gc` and `tome daemon status|stop`.
- **Commands it doesn't apply to:** commands that only touch local files, such as `tome validate`, `tome workflow new|ls|rm` and `tome layout`. They refuse `--on` with exit `2`. The hint suggests running `ssh <dest> tome …` instead.
- **Agent commands:** the commands agents call inside a run (`tome step`, `tome worker`, `tome group`, `tome worktree`, `tome queue`, `tome ready`, `tome session move`) refuse `--on` with exit `2`. They always act on the run they're in, and that run is on the machine they're running on.
- **Run references:** `<node>:<id>` can be used anywhere a run id is accepted, e.g. `tome runs show mini:12`. It means the same as `--on mini` with id `12`. If `--on` is also given and names a different node, the command exits `2`.
- **Projects:** a command run inside a project acts on that project on the node. The node's copy of the project is found this way:
  1. The node's `projects.<name>` entry, where `<name>` is the project directory's name.
  2. Otherwise, the same path relative to the home directory. For example, `~/Development/Projects/tome-cli` here maps to `~/Development/Projects/tome-cli` on the node.

  If neither exists on the node, the command exits `4`. The message names the paths it tried and the config key to set.
- **Workflows are resolved on the node.** `tome run <name> --on mini` runs the workflow called `<name>` from the node's copy of the project, or from the node's global `~/.tome/workflows`. The local workflow file isn't sent. A workflow given as a path is refused with `--on` (exit `2`), because local paths mean nothing on the node. The command prints the path of the workflow file the node used.
- **Code drift:** when the project is a git repo on both machines, `tome run --on` compares the two `HEAD`s. If they differ, it warns, e.g. "mini's tome-cli is at abc123, yours is at def456". It also warns when either side has uncommitted changes. The run starts anyway, because tome doesn't sync code.
- **Placement and backend** resolve on the node, from the node's config, as if the run had been started there. `tome run`'s placement flags are passed through. `--from caller` is refused (exit `2`), because the caller pane is on this machine.

### Watching remote sessions

- **`tome session view <run>[/<worker>]`**, also with `--on`, `<node>:<id>/<worker>`, or a local run, shows a run's orchestrator or worker session from this machine:
  - **Remote tmux session, called from a cmux pane:** it opens a local cmux tab running `ssh -t <dest> tmux attach -t <session>`. The tab uses 010's placement flags. The node returns the exact attach command, including its `TOME_TMUX_SOCKET`.
  - **Remote tmux session, called outside cmux:** the attach happens in the current terminal.
  - **Remote cmux session:** it's refused (exit `2`), because cmux sessions can't be attached over SSH. The hint is to use the tmux backend on that node.
  - **Local session:** it focuses the session's tab or pane, or prints the `tmux attach` command.
- **`tome run --on <node> --view`** starts the run detached, waits for the orchestrator's handshake (feature 007), and then runs `session view` on it.
- Viewing is read-write, as attaching to tmux always is. Typing in the view types into the agent.

### Cross-machine events

A project can forward bus events to nodes in its `.tome/config.yaml`:

```yaml
bus:
  forward:
    - topic: tome.run.build.succeeded    # a 009 topic pattern
      to: mini                           # a node name, or a list of them
```

- **Delivery:** forwarding is a delivery like a subscription's (feature 009). When a matching event is published, the daemon records a delivery for each target node. It sends the delivery by publishing the event to the same topic on the node's copy of the project, so the node's `on:` triggers handle it there.
- **Durable:** if the node can't be reached, the delivery stays `pending` and is retried with backoff, up to 1 minute between attempts, for 24 hours. After that it becomes `failed` and a [[notification]] is sent. `tome events retry` and `tome events remove` work on these deliveries as on others. `tome events show` lists them with the node as the subscriber, e.g. `→ mini`.
- **Sender and depth:** on the node, a forwarded event's sender is `node <origin> (<original sender>)`. Its depth carries over, so 009's depth limit of 8 also applies to chains that cross machines.
- **No echo:** an event is never forwarded back to the node it came from. Forwarding rules that make a cycle across three or more machines are stopped by the depth limit.
- **Built-in topics:** `tome.*` lifecycle events can be forwarded. On the node they keep their topic, so `on: tome.run.build.succeeded` works there as if the run had been local. The payload gains `node: <origin>`.
- Inside a workflow, an agent can also run `tome publish --on <node> …` to hand work to another machine on the spot.
- `tome validate` checks that each forwarding rule names a configured node and uses a valid topic pattern. An unknown node is a warning, not an error, because the global config can differ from machine to machine.

### Listing across nodes

- **`tome runs ls --nodes`** lists runs from this machine and every configured node, with a `NODE` column. Nodes are queried in parallel. Unreachable nodes are reported as warnings on stderr, and the command exits `0`.
- `--nodes` combines with the existing filters (`--status`, `--workflow`, `--limit`). The limit applies to each node separately.

### Out of scope

- One run's workers or worktrees on a different machine than its orchestrator (the next phase).
- Syncing code, workflow files or config between machines. Use git and dotfiles.
- Sending notifications to the machine that started a remote run. A forwarding rule plus a local workflow listening for `tome.run.*.failed` can do it.
- Transports other than SSH: a TCP listener, tokens or mTLS.
- Finding nodes automatically (mDNS, Tailscale API).
- Viewing remote cmux sessions.
- A run moving between machines.

## Use Cases

1. As a user, I want to register another machine with `tome node add` and check it with `tome node check`, so that I know it's ready before I send it work.
2. As a user, I want `tome run <workflow> --on mini`, so that I can start a workflow on another computer without leaving my terminal.
3. As a user, I want `tome runs show mini:12`, `tome runs logs mini:12` and `tome run cancel mini:12`, so that I can follow and control remote runs as I do local ones.
4. As a user in cmux, I want `tome session view mini:12` to open a tab attached to the remote orchestrator, so that I can watch and steer remote agents next to my local ones.
5. As a user, I want `tome runs ls --nodes`, so that I can see what's running on all my machines at once.
6. As an author, I want to forward `tome.run.build.succeeded` to another machine, so that a build here starts a deploy or performance test there.
7. As an orchestrator, I want `tome publish --on <node>`, so that a run can hand work to another machine.
8. As a user, I want a warning when the node's copy of the project is at a different commit, so that I don't run a workflow against stale code without knowing.
9. As a user, I want forwarded events to wait and retry while a node is offline, so that a laptop closing its lid doesn't lose events.
10. As a new user, I want `tome node check` to name the exact fix for each failure, so that setting up a node doesn't mean debugging SSH by hand.

## Constraints

- Every run is owned by one daemon, its node, from start to finish. Daemons never share or replicate run state.
- SSH is the only transport. tome opens no listening port.
- SSH runs in batch mode, so tome never waits on a password or host-key prompt. This matters because agents and the daemon call it too.
- The JSON-RPC protocol and the methods themselves don't change. `--on` only changes where requests go. The only additions are the relay command, the version in `daemon.ping`, resolving workflows by name on the node, `session.attach_command` and the git `HEAD` report.
- Without `nodes:` in the config, `--on` or `bus.forward`, tome behaves exactly as before.
- On the node, a run started with `--on` is an ordinary run. Its agents talk to the node's daemon, and its triggers, worktrees and logs stay on the node.

## Edge Cases

- **The node is unreachable** (SSH fails, times out or asks for a password) → exit `3`, as when the daemon isn't running. The message gives SSH's error and the hint `tome node check <name>`.
- **tome isn't installed on the node, or isn't on the PATH of a non-interactive SSH shell** → exit `3`. The hint suggests setting `tome:` to the binary's full path.
- **The protocol versions differ** → exit `1` before any request is sent. The hint says which side to upgrade.
- **The node's daemon isn't running** → the relay starts it. It runs detached, so it outlives the SSH connection. Started over SSH, it isn't inside cmux, so its runs default to tmux. `tome node check` suggests `tome daemon install` on the node so that the daemon runs as a login service.
- **A workflow on the node sets `defaults.backend: cmux`, but the node's daemon isn't running under cmux** → the existing backend error, reported through `--on`.
- **The project isn't found on the node** → exit `4`, naming the paths that were tried.
- **The workflow isn't found on the node** → exit `4`. If the workflow exists locally, the hint says so (e.g. "commit and pull it on mini").
- **The SSH connection drops during an attached `tome run --on`** → the same as closing an attached local run's client. The run keeps going on the node, and `tome runs show mini:<id>` picks it up.
- **The daemon can't authenticate to forward events** (a launchd or systemd daemon has no ssh-agent) → the deliveries stay `pending` and retry. The first failure is notified, with a hint to use an SSH key the daemon can read (`IdentityFile` in `~/.ssh/config`).
- **A forwarding rule names a node that isn't configured on this machine** → `tome validate` warns. At publish time, the delivery is recorded as `failed` straight away and notified.
- **The node's project has no `on:` subscriber for a forwarded topic** → the event is recorded on the node with no deliveries, as in 009.
- **A `--on` node and a `<node>:<id>` reference disagree** → exit `2`.
- **`--on local`** → the same as leaving out `--on`.
- **The node is this machine** (a node configured with its own address) → it works, through an SSH loopback. Nothing detects or prevents it.
- **Two nodes run the same workflow on the same topic** → each node's runs are independent. Concurrency limits apply per node, not across nodes.

## Related Entities

- [[node]]
- [[daemon]]
- [[run]]
- [[session]]
- [[event]]
- [[trigger]]
- [[backend]]
- [[workflow]]
