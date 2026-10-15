# Plugins

A **plugin** is a folder with a `plugin.yaml` manifest and its own executables. Plugins let features that don't belong in the tome binary, such as a cloud VM provisioner, live outside it. A plugin can add subcommands, hooks, workflows and harnesses.

Plugins are installed by you, for the whole machine, into `~/.tome/plugins/<name>/`. There are no project-level plugins, so a repo you clone can never add code that tome runs. Without plugins installed, tome behaves exactly as before.

## Installing

```sh
tome plugin install ./my-plugin                       # a folder
tome plugin install https://github.com/me/plugin.git  # a git URL
tome plugin install git@github.com:me/plugin.git#v1.2 # a branch, tag or commit after `#`
```

Install checks the manifest and copies the folder (or clones the repo) into `~/.tome/plugins/<name>/`. It runs no plugin code and has no build step, so a plugin that needs building ships its built binaries.

It refuses with exit code `2` when:

- the manifest is invalid, or names an unknown hook (`worker.place` is reserved and reported as "not yet supported");
- the plugin speaks a different plugin protocol than this tome, or its `tome:` range doesn't include this version. The error says which side to upgrade;
- a file the manifest names (a subcommand's executable, a workflow) is missing, outside the folder, or not usable;
- a subcommand, provider, workflow or harness name is already taken by a built-in command, a configured node, one of your workflows or harnesses, or another plugin. The error names both sources.

## Managing

```sh
tome plugin ls               # name, state, version, hooks, and any problems
tome plugin update [name]    # pull git installs (all of them without a name)
tome plugin disable <name>
tome plugin enable <name>
tome plugin rm <name>
```

`tome plugin ls` shows each plugin as `enabled`, `disabled`, `incompatible` or `invalid`. A plugin that stops fitting after a tome upgrade stays installed and shows as incompatible, with a hint naming which side to upgrade. A name that clashes after the fact, because you added a workflow with the same name or an update brought one in, is listed under the table.

`tome plugin update` pulls git installs. If the new version is invalid, incompatible or clashes with a name, tome puts the old version back and reports the reason. Plugins installed from a path are skipped; remove and install them again.

Plugins belong to the machine, so these commands refuse `--on` (exit `2`). Run them on the node over `ssh`.

## The manifest

```yaml
name: cloudvm               # the folder name; lowercase letters, digits, `_` and `-`
version: 0.1.0              # optional
description: Cloud VMs      # optional
protocol: 1                 # the plugin protocol this plugin speaks (this tome speaks 1)
tome: ">=0.7, <1"           # optional range of tome versions
command: bin/cloudd         # the process that handles hooks and providers
subcommands:
  cloud: bin/cloud          # `tome cloud …`; a path inside the plugin folder
hooks:                      # a list of names, or a map with options
  provide: { timeout: 10m }
  release:
  run.before_start: { on_error: allow, timeout: 30s }
  run.finished:
providers: [cloud-vm]       # names usable as `--on cloud-vm`
workflows: [provision]      # workflows/provision.md in the plugin folder
harnesses:                  # same entries as `harnesses:` in config.yaml
  mine: ["mine-agent", "{{prompt}}"]
```

`command` is the path of the plugin's long-running process, inside the plugin folder. It is required when the plugin declares hooks or providers. A plugin that only adds subcommands, workflows or harnesses needs none.

Hooks: `provide`, `release`, `run.before_start`, `run.started`, `run.finished`, `daemon.started` and `daemon.stopping`. `providers` needs both `provide` and `release`, and the other way round. `on_error` (`allow` or `reject`, default `reject`) only applies to `run.before_start`.

Workflows use the usual format, and the `name` in the frontmatter must match the name listed in the manifest. Names must be unique across your own definitions and every plugin; tome never silently picks one.

A plugin owns its configuration and secrets. tome has no config schema for plugins and doesn't store their secrets.

## Subcommands

A subcommand listed under `subcommands:` runs as `tome <name> …`. tome replaces itself with the plugin's executable, so the terminal passes straight through and the plugin can be interactive. The executable gets the rest of the arguments, plus these environment variables:

| Variable | Value |
| --- | --- |
| `TOME_PLUGIN` | The plugin's name. |
| `TOME_PLUGIN_DIR` | The plugin's folder. |
| `TOME_PLUGIN_DATA` | A folder the plugin may keep its own state in (`~/.tome/plugins/<name>/data`). It survives updates. |

The plugin calls back into tome through the ordinary CLI. Subcommand names can't be those of built-in commands.

## The process and its protocol

When the daemon starts, it starts each enabled plugin that has hooks, running its `command` as one long-lived child. The two sides speak newline-delimited JSON-RPC 2.0 over the child's stdin and stdout. Write nothing else to stdout; use stderr for logs, which go to `~/.tome/plugins/<name>/plugin.log`.

1. tome sends `initialize` first, with `protocol`, `tome_version`, `plugin` and `state_dir`. The plugin answers `{"protocol": 1}` within 10 seconds. A different protocol marks the plugin `incompatible` and it isn't restarted.
2. tome then sends one request per hook, with the hook's name as the method, and waits for the reply up to the hook's `timeout` (10 seconds unless the manifest says otherwise).
3. A request that takes a while may send `{"method": "progress", "params": {"id": <request id>, "message": "…"}}` notifications first. For `provide` they're shown to the person who ran the command.
4. The plugin can send tome one request: `notify` with `title` and `message`, which shows a desktop notification, prefixed with the plugin's name.

If the plugin exits, tome restarts it with a growing delay. After five failures in a row, the plugin is marked `failed` and you're notified. A call to a plugin that isn't running fails at once, so a broken plugin never holds up tome. `tome plugin ls` shows the live state as `running`, `starting`, `restarting`, `failed` or `incompatible`, with the last error and the log path under the table.

### Hooks

| Hook | When | The plugin's reply |
| --- | --- | --- |
| `run.before_start` | Before a run starts or resumes. | Required; see below. |
| `run.started` | After a run has started. | Ignored. |
| `run.finished` | When a run ends. | Ignored. |
| `daemon.started` | Once, when the daemon is up. | Ignored. |
| `daemon.stopping` | When the daemon shuts down. | Ignored. |
| `provide`, `release` | See [Providers](#providers). | |

The notification hooks never wait for the plugin. Their requests carry `run_id`, `workflow`, `project` and `node`; `run.started` adds `params` and `cause`, and `run.finished` adds `status` (`succeeded`, `failed` or `cancelled`), `reason` and `summary`. `daemon.started` carries `pid`, `version`, `socket` and `home`.

### `run.before_start`

Every plugin that declares it is asked, in order, before a run starts. The request has `workflow`, `workflow_path`, `project`, `params`, `env`, `cause`, `resumes` (the run's id when this is a resume) and `node`. The reply is one of:

```json
{"action": "allow"}
{"action": "reject", "reason": "over budget"}
{"action": "modify", "params": {"size": "large"}, "env": {"REGION": "eu"}}
```

- `reject` refuses the run with exit code `1` and the reason.
- `modify` changes the run's params and adds environment variables for its sessions. Each plugin sees the run as the ones before it left it. Names starting with `TOME_` are reserved. A changed param must still be valid for the workflow. Changes to a resumed run are ignored.
- If the plugin fails or times out, the run is rejected, unless the hook says `on_error: allow`.

What a plugin changed is stored with the run and shown by `tome runs show`: the params before and after, and the names (never the values) of the variables it set.

## Checking a plugin

```sh
tome plugin check cloudvm
```

Checks, in order: the manifest, compatibility, the files it names, name clashes, and that the process starts and answers `initialize`. It stops at the first failure, says how to fix it and exits `2`. It starts a separate process for the check, so it works whether or not the daemon is running.

## Notifications

`tome notify <title> <message>` shows a notification on this machine through the daemon, by the same path a plugin's `notify` request takes. It is for scripts as well as plugins.

## Harnesses

`tome harness ls` lists every harness tome knows and where each comes from: built in, your config, or a plugin.

## Providers

A **provider** makes a machine on demand, runs a workflow on it and throws it away. A plugin declares the provider names it handles, and `--on` accepts them as if they were nodes:

```sh
tome run build --on cloud-vm
tome run build --on cloud-vm --provider-opt size=large --provider-opt region=eu
```

`--on <name>` is a provider when an active plugin declares that name under `providers:`. Otherwise it's a node, as before. A provider name can't also be a configured node: `plugin install` and `tome node add` both refuse the clash.

### What tome does

1. Calls the plugin's `provide` hook and waits until the machine is ready. The plugin may send `progress` messages, which tome prints to stderr. The wait is limited by the hook's `timeout` (default 10 minutes).
2. Registers the machine as an ephemeral node called `<provider>-<id>` and runs the same checks as `tome node check`, once.
3. Runs the workflow there like on any node, attached.
4. Reads the run's final status, summary and the tail of its logs from the machine and stores them locally (see [Records](#records-of-released-machines)).
5. Calls `release`, whatever the outcome, and forgets the node.

If `provide` fails or times out, or the machine fails its check, tome calls `release` and exits `3`. For a failed check the message says which step failed and how to fix it. Pressing Ctrl-C while waiting releases the machine too. A `release` that fails is reported on stderr and as a notification, and is not retried. Its timeout is the hook's `timeout`, 120 seconds by default.

### Options for `provide`

`--provider-opt KEY=VALUE` is repeatable and later ones win. Defaults come from `defaults.provider_opts`, in the workflow's frontmatter or in `config.yaml`:

```yaml
# ~/.tome/config.yaml or a project's .tome/config.yaml
defaults:
  provider_opts:
    size: small
    region: eu
```

The command line beats the workflow, which beats config. Values may be strings, numbers or booleans, and reach the plugin as strings. `--provider-opt` without a provider is an error.

### What the plugin implements

`provide` receives `{provider, opts, workflow, project}` and returns when the machine is ready:

```json
{"id": "i-0abc", "ssh": "root@203.0.113.7", "tome": "/usr/local/bin/tome", "projects": {"my-app": "/work/my-app"}}
```

`id` (letters, digits, `.`, `_`, `-`; up to 64 characters) and `ssh` are required. `tome` and `projects` are optional and mean what they do for a [node](nodes.md#adding-a-node). The machine needs tome installed, the project in place, and a key-based login.

`release` receives:

| Field | Meaning |
| --- | --- |
| `provider`, `id` | What to release. `id` is `null` if `provide` never answered. |
| `cause` | `finished`, `provide_failed`, `check_failed` or `interrupted`. |
| `opts` | The options `provide` got. |
| `run` | `{run, status, summary, reason}` of the run, or `null` if it never started. |
| `logs` | Where the logs are: `node`, `ssh`, `paths` (log files on the machine) and `stub` (the local record). `null` if there was no run. |

Release must tolerate being called for a machine that is already gone.

### Keeping the machine

`--keep` skips the record and the `release`, and leaves the machine registered as an ephemeral node so you can look around:

```sh
tome run build --on cloud-vm --detach --keep
tome runs list --on cloud-vm-i-0abc
tome runs show cloud-vm-i-0abc:12
```

`--detach` and `--view` need `--keep`, because releasing a machine under a run that is still going would kill it. tome never releases a kept machine. Release it yourself with the provider's own tools. `tome node rm <name>` only makes tome forget it.

### Records of released machines

tome writes one JSON file per released machine into `~/.tome/released/`: the run's status, summary and the last 40 lines of each step's log. `tome runs list` shows them after the live runs as `<provider>:<id> (released)`, and the `released` array in `--json` output has the details. `--status`, `--workflow` and `--limit` apply to them. They aren't listed when you use `--on`.

Ephemeral nodes live in `~/.tome/ephemeral/` while a machine exists.
