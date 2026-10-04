# Remote nodes

A **node** is another machine with tome on it that you can reach over SSH: a Mac mini on the desk, a build box, a cloud VM. With one set up, the commands you use here work there too. Add `--on <node>` to start a run on it, watch it, list its runs or publish to its bus. A project can also forward its events to a node, so a run finishing here starts a workflow there.

tome doesn't run a server for this. Each command runs `ssh <node> tome rpc --stdio`, which talks to the node's daemon (and starts that daemon if it isn't running). Your SSH keys, `~/.ssh/config` and jump hosts all work as they do for `ssh`.

## Adding a node

The node needs tome installed, the same version as here, and a key-based SSH login. Then:

```sh
tome node add mini kyle@mini.local
```

This adds the node to `~/.tome/config.yaml` and checks it. If tome isn't on the node's `PATH` for a non-interactive shell (common with Homebrew or `~/.cargo/bin`), give its path:

```sh
tome node add mini kyle@mini.local --tome /opt/homebrew/bin/tome
```

The config entry looks like this:

```yaml
# ~/.tome/config.yaml
nodes:
  mini:
    ssh: kyle@mini.local        # anything `ssh` accepts, including a Host from ~/.ssh/config
    tome: /opt/homebrew/bin/tome  # optional; default `tome`
    projects:                   # optional; where projects are on the node
      my-app: ~/src/my-app
```

Node names use `a-z`, `0-9`, `-` and `_`. `local` is reserved: it means this machine.

```sh
tome node ls             # every node: reachable, version, daemon, round trip
tome node check mini     # step by step, with a fix for the first thing that fails
tome node rm mini
```

`tome node check` checks SSH, then tome on the node, then the version, then the node's daemon, then whether the current project is there. When something fails it prints what to fix. To keep the node's daemon running across reboots, run `tome daemon install` on the node.

## Running commands on a node

```sh
tome run build --on mini              # resolve and run `build` on mini, attached
tome run build --on mini --detach
tome runs list --on mini
tome runs show mini:12                # `<node>:<id>` names a run on a node
tome runs logs mini:12 --tail 50
tome run cancel mini:12
tome daemon status --on mini
tome triggers ls --on mini
tome events ls --on mini
```

Set `TOME_NODE=mini` to make `--on mini` the default for a shell. `--on local` overrides it.

These commands only work on this machine, so they refuse `--on`: `validate`, `workflow …`, `layout …`, `node …`, and `daemon start|run|install|uninstall`. Run them on the node over `ssh`. The commands agents use inside a run (`ready`, `step`, `worker`, `run finish` and so on) refuse it too, because they act on the run the agent is in.

### Which project

A command run inside a project acts on the node's copy of that project. tome looks for it in two places, in order:

1. `nodes.<node>.projects.<directory name>` in the config.
2. The same path relative to your home directory. `~/src/my-app` here is `~/src/my-app` there.

If neither has a `.tome/` directory, the command exits `4` and says what to set.

### Which workflow

`tome run <name> --on mini` resolves the name on the node, using the node's project and global workflows, and prints the path it used. tome doesn't copy workflow files. To run your latest changes on the node, commit them and pull them there. When the node's checkout is at a different commit, or has uncommitted changes, tome prints a warning and starts the run anyway. If the workflow exists here but not on the node, the error says so. Workflow paths and `--from caller` don't work with `--on`.

An attached remote run keeps going if the SSH connection drops. Check on it with `tome runs show mini:<id>`.

## Watching a remote run

```sh
tome session view mini:12            # the orchestrator's session (or the single agent's)
tome session view mini:12/w1         # a worker's
tome run build --on mini --view      # start it, then open its session
```

When the node uses tmux, tome attaches to its session with `ssh -t`. Inside cmux, that happens in a new tab, and you can place the tab with `--preset`, `--layout`, `--workspace`, `--direction` and `--size`. Anywhere else, it runs in the current terminal. You can't view a remote cmux session this way, because cmux is a desktop app.

## Listing runs everywhere

```sh
tome runs list --nodes
```

This lists runs from this machine and every node, with a `NODE` column. `--limit` applies per node. Nodes that can't be reached are listed as warnings on stderr, and the command still exits `0`.

## Forwarding events

A project can forward bus events to nodes in its `.tome/config.yaml`:

```yaml
bus:
  forward:
    - topic: tome.run.build.succeeded   # a topic pattern, as in `on:`
      to: mini                          # a node, or a list of them
    - topic: deploy.*
      to: [mini, ci]
```

Each matching event gets a delivery to each node, listed in `tome events show` as `→ mini`. The daemon sends it by publishing the event to the same topic on the node's copy of the project. The node's `on:` triggers then handle it.

- **Waiting for the node.** If the node can't be reached, the delivery stays `pending` and is retried with backoff (at most a minute apart) for 24 hours. After that it becomes `failed` and you get a notification. `tome events retry` and `tome events remove` work on these deliveries too: `--workflow mini` names the forward to mini.
- **Unknown nodes.** If a rule names a node that isn't in this machine's config, the delivery fails straight away and you get a notification. `tome validate` warns about such rules.
- **Signing in.** A daemon run as a launchd or systemd service may not see your ssh-agent. If the first attempt can't authenticate, you get a notification. Set an `IdentityFile` for the node in `~/.ssh/config`.
- **On the node,** the sender is `node <origin> (<original sender>)`, for example `node laptop (run 12)`. The event's depth carries over, so the limit of 8 also applies to chains that cross machines. An event is never forwarded back to the machine it came from.
- **Built-in events.** `tome.*` events keep their topic, so `on: tome.run.build.succeeded` on the node works as if the run had been there. Their JSON payload gets a `node` field naming the machine the run was on.

An agent can also hand work to another machine directly:

```sh
tome publish --on mini deploy.requested "build 1.4.2 is ready"
```

## Errors

| Exit | Meaning |
|---|---|
| `1` | The node runs a different protocol version. The message says which side to upgrade. |
| `2` | `--on` on a command that only works here, or `--on` and a `<node>:<id>` reference that disagree. |
| `3` | The node can't be reached, or tome isn't installed on it. Run `tome node check <node>`. |
| `4` | The project or workflow isn't on the node. |
