# Plugin system

## Description

Tome gains a plugin system so features that don't belong in the main executable, such as a cloud VM provisioner built on Packer and Tailscale, can live outside it. The cloud VM plugin is the motivating example, but this feature is the general mechanism, not the plugin.

A plugin is a folder in `~/.tome/plugins/<name>/` with a `plugin.yaml` manifest and its own executables. Plugins are installed only by the user, globally. There are no project-level plugins, so a cloned repo can never introduce code that tome runs.

A plugin can contribute:

- **Subcommands.** `tome <plugin-cmd> …` execs the plugin's command from the manifest directly, with the terminal passed through, like git or cargo external subcommands. The plugin calls back into tome through the normal CLI (e.g. `tome node add`).
- **Hooks.** The [[daemon]] starts each enabled plugin as a persistent child process when the daemon starts. They speak newline-delimited JSON-RPC over stdio, the same style as `tome rpc --stdio`. The first call is `initialize`, which carries the protocol and tome versions.
- **Workflows and harnesses.** Plugin folders can include [[workflow]]s and harness definitions in the formats tome already has.

### Manifest and compatibility

- `plugin.yaml` declares the plugin's name, `protocol: N` (the plugin-RPC version it speaks), an optional `tome: ">=x.y"` range, its subcommands, its hooks, its provider names, and its workflows and harnesses.
- Tome checks `protocol` and `tome:` at install and again in the `initialize` handshake. A mismatch disables the plugin. It stays installed and is shown as incompatible in `tome plugin ls`, with a hint naming which side to upgrade. It never stops tome itself from working.
- An unknown hook name in a manifest is an error. `worker.place` is reserved and is rejected as "not yet supported".

### Management

- **`tome plugin install <path|git-url[#ref]>`** validates the manifest and copies or clones the folder into `~/.tome/plugins/<name>/`. Install runs no plugin code and has no build step. A plugin that needs a build ships its built binaries. It refuses with exit `2` if a subcommand, provider, workflow or harness name already exists, naming both sources.
- **`tome plugin update`** re-pulls git installs. **`rm`**, **`enable`** and **`disable`** manage installed plugins.
- **`tome plugin ls`** shows each plugin's state (running, disabled, failed, incompatible), version, hooks and any name conflicts.
- **`tome plugin check <name>`** validates the manifest, version compatibility, name conflicts and that the plugin starts and completes `initialize`.
- Plugin management commands refuse `--on` (exit `2`), because plugins are per machine.

### Process supervision

- Enabled plugins start with the daemon, so daemon hooks work, and run until it stops. Each gets `initialize` first.
- Calls have timeouts. Logs go to `~/.tome/plugins/<name>/plugin.log`. Tome passes the plugin a state directory, `~/.tome/plugins/<name>/data`, and nothing more.
- A crashed plugin restarts with backoff. After N failed restarts tome gives up, marks it failed and sends a [[notification]]. In-flight calls to a dead or hung plugin fail.
- A plugin crash or hang never breaks tome or runs that don't depend on it.
- **Configuration and secrets belong to the plugin.** Tome has no config schema for plugins and does not store their secrets.

### Hooks

| Hook | Kind | Notes |
|---|---|---|
| `provide` / `release` | Request and response | The node provider (below). |
| `run.before_start` | Blocking | Reply is `allow`, `reject` (with a reason), or a modification of the run's params and environment. |
| `run.started`, `run.finished` | Notification | `run.finished` covers succeeded, failed and cancelled. Tome doesn't wait on the reply. |
| `daemon.started`, `daemon.stopping` | Notification | |
| `worker.place` | Reserved | Not available in this feature. |

- **Blocking before-hooks** have a timeout. A timeout or plugin crash fails closed (rejects the run), unless the hook sets `on_error: allow` in the manifest. A reject stops the run with a clear message and an exit code.
- **Modifications are recorded and shown.** When a before-hook changes params or environment, the run records what changed and which plugin changed it, and `tome runs show` displays it.
- **Notification hooks** can't affect a run. A failing or slow plugin is logged and notified.
- **Each daemon fires its own hooks.** `provide` and `release` fire on the machine where the command was typed (the originator, which has the provider plugin). Run lifecycle hooks fire on the daemon that owns the run, using that machine's plugins, which is the node for `--on`. Plugin installs are not synced between machines.

### Node providers

A plugin can declare **provider** names. A provider supplies a machine that tome can run on, which makes `tome run` work on machines tome doesn't know how to create.

- **`tome run <workflow> --on <provider> [--provider-opt key=value …]`**: `--on` means a provider when its name is declared by an installed plugin. `--provider-opt` is repeatable and sent to `provide`, and the plugin validates the options. A workflow or project config can set defaults (`defaults.provider_opts`).
- **Name clashes** between a provider and a configured node are errors, at install and in `tome node add`.
- **Flow:**
  1. Tome calls `provide`. It returns only when the node is ready to use, and may stream progress notifications that tome shows (e.g. "booting…"). It has a manifest-set timeout, 10 minutes by default.
  2. Tome registers the result as an ephemeral node and runs the normal `tome node check` steps once.
  3. The run happens on that node, as in feature 014.
  4. Before releasing, tome fetches the run's final status, summary and log tail from the node and stores a stub record locally. `tome runs ls` lists it as `<provider>:<id> (released)`.
  5. Tome calls `release` when the run ends: succeeded, failed or cancelled. `--keep` skips it so the node can be inspected. The `release` request tells the plugin where the run's logs are, so the plugin can save them.
- **Persistent nodes:** a plugin may also register nodes that outlive any run with `tome node add`, typically from its own subcommands. Tome treats them like any configured node.
- **Leaks are the plugin's responsibility.** Tome calls `release` once at the end of a run and keeps no lease table. A plugin finds orphans from a daemon crash itself, e.g. on `daemon.started`. `release` should be idempotent.

### Plugin workflows and harnesses

- Workflows and harnesses in a plugin use the same formats as user-defined ones. They are available by their plain names.
- **Names must be unique across the user's own definitions and all plugins.** `tome plugin install` refuses on a collision. A collision that appears later (the user adds a workflow, or a plugin update adds one) makes `tome validate` and any use of that name error, listing both sources. Tome never silently picks one. `tome plugin ls` flags it.
- `tome workflow ls` and `tome harness ls` show where each item came from.

### Out of scope

- **Worker placement** (`worker.place`) and running a run's workers on other machines. It is a follow-up feature, and only the hook name is reserved here.
- Project-level plugins and any approval flow for them.
- A plugin registry or marketplace, and install-time build steps.
- Plugin-contributed terminal backends.
- Tome-managed plugin config schemas, secret storage, and a lease table for provider nodes.
- Syncing plugins between nodes.
- The cloud VM (Packer and Tailscale) plugin itself.

## Use Cases

1. As a user, I want to install a plugin from a path or git URL, so that I can extend tome without it being in the main executable.
2. As a user, I want `tome plugin ls` and `tome plugin check` to show each plugin's state, version compatibility, hooks and conflicts, so that I know what is active and what is broken.
3. As a user, I want to run a plugin's subcommand as `tome <cmd>`, so that plugins feel native and can be interactive.
4. As a user, I want `tome run X --on <provider>` to get a machine from a plugin, run there and release it, so that I can use machines tome doesn't know how to create.
5. As a user, I want `--keep` to skip release, so that I can debug a failed run on its machine.
6. As a user, I want a local record of a run after its node was released, so that I still see its outcome.
7. As a plugin author, I want a blocking `run.before_start` hook that can allow, reject or modify a run, so that I can build policy or secret-injection plugins.
8. As a plugin author, I want notifications for run and daemon lifecycle, so that I can do cleanup, reporting or orphan reaping.
9. As a plugin author, I want to ship workflows and harnesses, so that my plugin works without manual setup.
10. As a user, I want a plugin crash or hang never to break tome or other runs, so that a bad plugin only disables itself.
11. As the maintainer, I want a cloud VM (Packer and Tailscale) plugin to be buildable entirely against this system, so that it never enters the main codebase.

## Constraints

- Plugins load only from `~/.tome/plugins`, installed explicitly by the user. A cloned repo can never introduce code that tome runs.
- Subcommands exec the plugin directly. Hooks use a daemon-owned persistent JSON-RPC process. The plugin-RPC protocol is versioned with an integer, separate from tome's version and from the daemon protocol.
- A plugin owns its configuration, secrets and orphan cleanup. Tome passes only a state directory.
- Each daemon fires its own hooks. A run is still owned by one daemon (feature 014), and plugin installs are not synced.
- Without plugins installed, tome behaves exactly as before.
- Plugin code is deterministic at the edges: tome keeps process spawning, I/O and timeouts at the boundary, and the manifest, name-collision and compatibility rules are testable without starting a process.

## Edge Cases

- **A plugin's workflow or harness has the same name as an existing one, or as another plugin's** → `tome plugin install` refuses with exit `2` and names both sources. A later collision makes `tome validate` and any use of the name error. `plugin ls` flags it.
- **A plugin's subcommand or provider name clashes with a built-in command or a configured node** → install refuses (exit `2`). `tome node add` also refuses a name that is a provider.
- **A plugin crashes** → it restarts with backoff, then gives up after N tries and notifies. In-flight calls fail. `run.before_start` fails closed unless the hook sets `on_error: allow`.
- **A plugin hangs on a call** → the call times out. A blocking before-hook then follows its `on_error` setting. Notification hooks are logged and skipped.
- **Protocol or tome version mismatch** → the plugin is disabled but stays installed, shown as incompatible with a hint naming which side to upgrade. An unknown hook name in a manifest is an error.
- **`provide` times out, or `node check` fails after it returns** → tome calls `release` and fails the run with exit `3`, with the check's fix hint.
- **`release` fails** → the failure is notified and the node is left to the plugin. Tome doesn't retry.
- **The daemon dies mid-run** → the provided node may leak. Recovery is the plugin's job, e.g. from `daemon.started`.
- **A node is added later with a provider's name** → refused.
- **A before-hook rejects a run** → the run doesn't start, and the reason is shown with a non-zero exit.
- **A before-hook modifies a run** → what changed and which plugin changed it is recorded and shown in `tome runs show`.
- **A plugin management command is given `--on`** → exit `2`, because plugins are per machine.
- **`--keep` on a provider run** → `release` is skipped. The node stays registered as an ephemeral node until the plugin or the user removes it.
- **A run reaches a provider node and the originating daemon restarts** → the run continues on the node. Releasing it is the plugin's job, since tome keeps no lease record.

## Related Entities

- [[daemon]]
- [[node]]
- [[run]]
- [[workflow]]
- [[harness-adapter]]
- [[notification]]
- [[event]]
