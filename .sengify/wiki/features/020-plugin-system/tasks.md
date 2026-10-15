# Plugin System — Task Plan

Source feature: [feature.md](./feature.md)

## Tasks

### 020-1. Plugin manifest, install and management

**Blocked by:** none

A plugin is a folder with a `plugin.yaml` manifest declaring its name, `protocol`, optional `tome:` range, subcommands, hooks, provider names, and workflows and harnesses. `tome plugin install <path|git-url[#ref]>` validates the manifest and copies or clones the folder into `~/.tome/plugins/<name>/` without running any plugin code. It refuses with exit `2` when a subcommand, provider, workflow or harness name already exists, naming both sources. Unknown hook names are errors, and `worker.place` is rejected as "not yet supported". `tome plugin update|rm|enable|disable` manage installed plugins, and a first `tome plugin ls` shows version, hooks, compatibility and name conflicts. Plugin management commands refuse `--on` with exit `2`. Covers use cases 1 and 2.

### 020-2. Plugin subcommands

**Blocked by:** 020-1

`tome <plugin-cmd> …` execs the plugin's command from the manifest directly, with the terminal passed through, so plugins can be interactive and call back into tome through the normal CLI. Only enabled, compatible plugins contribute subcommands, and built-in command names always win. Covers use case 3.

### 020-3. Plugin workflows and harnesses

**Blocked by:** 020-1

Workflows and harnesses shipped in a plugin folder use the existing formats and are available by their plain names. Names must be unique across the user's own definitions and all plugins. A collision that appears after install makes `tome validate` and any use of the name error, listing both sources, and `tome plugin ls` flags it. `tome workflow ls` and `tome harness ls` show where each item came from. Covers use case 9.

### 020-4. Daemon plugin supervision and `plugin check`

**Blocked by:** 020-1

The daemon starts each enabled plugin as a persistent child process speaking newline-delimited JSON-RPC over stdio, and sends `initialize` with the protocol and tome versions first. A protocol or tome version mismatch disables the plugin but leaves it installed, shown as incompatible with a hint naming which side to upgrade. Calls have timeouts, logs go to the plugin's `plugin.log`, and the plugin is given a state directory and nothing more. A crashed plugin restarts with backoff, and after N failures tome marks it failed and sends a notification. In-flight calls to a dead or hung plugin fail, and a bad plugin never breaks tome or runs that don't depend on it. `tome plugin ls` gains the running, disabled, failed and incompatible states, and `tome plugin check <name>` validates the manifest, compatibility, name conflicts and that the plugin starts and completes `initialize`. Covers use case 10 and the live-state half of use case 2.

### 020-5. Daemon and run lifecycle notification hooks

**Blocked by:** 020-4

Enabled plugins are told about `daemon.started`, `daemon.stopping`, `run.started` and `run.finished` (succeeded, failed or cancelled), each fired by the daemon that owns the run or the lifecycle event, using that machine's plugins. Tome doesn't wait on the reply, and a failing or slow plugin is logged and notified without affecting any run. Covers use case 8.

### 020-6. Plugin-initiated notifications

**Blocked by:** 020-4

A plugin can raise a user notification through tome, both from its persistent hook process and from its subcommands. A new plugin-to-tome notify request on the hook channel and a `tome notify <title> <message>` command send through the daemon's existing notification path, so the message reaches cmux and the other configured channels. Plugins can hand an event to an orchestrator by publishing to the project's message bus with `tome publish`, and the docs describe that route.

### 020-7. Blocking `run.before_start` hook

**Blocked by:** 020-4

Before a run starts, the daemon asks each plugin that declares the hook to allow, reject (with a reason) or modify the run's params and environment. The call has a timeout, and a timeout or plugin crash fails closed (the run is rejected) unless the manifest sets `on_error: allow`. A reject stops the run with a clear message and a non-zero exit. When a hook changes params or environment, the run records what changed and which plugin changed it, and `tome runs show` displays it. The hook runs on the daemon that owns the run, so with `--on` it uses the node's plugins. Covers use case 7.

### 020-8. Node providers: `tome run --on <provider>`

**Blocked by:** 020-4, [014-3](../014-remote-nodes/tasks.md#014-3-tome-node-addrmlscheck), [014-4](../014-remote-nodes/tasks.md#014-4-starting-remote-runs-tome-run---on)

**Dependency reason:** 014-3 provides `tome node check`, which tome runs once against the provided machine, and `tome node add`, which must refuse a name that is a provider. 014-4 provides starting and running a workflow on a node, which the provided machine uses.

A plugin can declare provider names. `--on` means a provider when an installed plugin declares that name, and a repeatable `--provider-opt key=value` is sent to `provide`, with `defaults.provider_opts` as workflow or project defaults. `provide` returns only when the node is ready, may stream progress that tome shows, and has a manifest-set timeout (10 minutes by default). Tome registers the result as an ephemeral node, runs the `node check` steps once, runs the workflow there as in feature 014, and calls `release` when the run ends, whatever its outcome. `--keep` skips `release` and leaves the node registered as an ephemeral node. If `provide` times out or the check fails, tome calls `release` and fails the run with exit `3` and the check's fix hint. A `release` failure is notified and not retried. A provider name that clashes with a configured node is refused at install and in `tome node add`. Covers use cases 4 and 5.

### 020-9. Local record of released provider runs

**Blocked by:** 020-8

Before releasing a provider node, tome fetches the run's final status, summary and log tail from the node and stores a stub record locally. `tome runs ls` lists it as `<provider>:<id> (released)`, and its outcome stays visible after the machine is gone. The `release` request tells the plugin where the run's logs are so the plugin can save them. Covers use case 6.

## Implementation notes

All nine tasks are implemented on `main` (no worktree). Docs: `guides/plugins.md` (manifest `command`, protocol, hooks, `plugin check`, states, `notify`, `run.before_start`, providers, `--on`), `guides/nodes.md` (providers), `guides/write-a-workflow.md` (`provider_opts`). Tests: `tests/plugins.rs` (020-1 to 020-7) and `tests/providers.rs` (020-8, 020-9).

### Interpretations made while implementing 020-8 and 020-9

- **The provider flow is split between the CLI and the daemon.** Plugin processes live in the daemon, but the ssh checks and the run itself happen in the CLI. The CLI streams `provider.provide` from the daemon (progress arrives as `run.event`s), does the node check and the run, then sends `provider.release`. Both calls go to *this machine's* daemon (`Client::local`), because after `--on` resolves, `Client::connect` would reach the new node's daemon instead.
- **Ephemeral nodes and records are plain JSON files**, `~/.tome/ephemeral/<provider>-<id>.json` and `~/.tome/released/*.json`, so there is no database migration. `node::all()` includes the ephemeral nodes (a configured node wins), so every `--on` command works on a kept machine.
- **`--keep` skips the record and the `release`.** The machine is still running, so there is no outcome to store yet. `--detach` and `--view` need `--keep`, since releasing under a live run would kill it. `tome node rm` forgets a kept node but doesn't release it.
- **Ctrl-C while waiting for `provide` calls `release`** with cause `interrupted`. The `release` request carries `cause` (`finished`, `provide_failed`, `check_failed`, `interrupted`), `run`, and `logs` (`node`, `ssh`, `paths`, `stub`).
- **`release` has a 120 s default timeout** (`provide`: 10 min), both overridable with the hook's `timeout`.
- **A provider name given to any command but `tome run` is refused** ("`x` is a provider, not a node"), e.g. `runs list --on fake-vm`.
- **The spec's `tome runs ls` is `tome runs list`** in this codebase. No alias was added.
- **`defaults` became a config key** (`defaults.provider_opts` only), with the order CLI over workflow over config.
- **`tome notify` also goes to this machine's daemon** whatever the target node, so a release failure still notifies when the run's node is the provided machine.

### Possible follow-ups

- A `tome provider release <name>` command to release a kept machine through its plugin.
- Keeping the whole log (not just the tail) in the record, via the plugin's `logs.paths`.
