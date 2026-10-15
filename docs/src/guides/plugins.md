# Plugins

A **plugin** is a folder with a `plugin.yaml` manifest and its own executables. Plugins let features that don't belong in the tome binary, such as a cloud VM provisioner, live outside it. A plugin can add subcommands, hooks, workflows and harnesses.

Plugins are installed by you, for the whole machine, into `~/.tome/plugins/<name>/`. There are no project-level plugins, so a repo you clone can never add code that tome runs. Without plugins installed, tome behaves exactly as before.

> **Status:** installing, listing and managing plugins works, and so do the name and version checks. Running a plugin's subcommands, hooks and providers is not wired up yet, so those parts of a manifest are validated and recorded but nothing runs them.

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

Hooks: `provide`, `release`, `run.before_start`, `run.started`, `run.finished`, `daemon.started` and `daemon.stopping`. `providers` needs both `provide` and `release`, and the other way round. `on_error` (`allow` or `reject`, default `reject`) only applies to `run.before_start`.

Workflows use the usual format, and the `name` in the frontmatter must match the name listed in the manifest. Names must be unique across your own definitions and every plugin; tome never silently picks one.

A plugin owns its configuration and secrets. tome has no config schema for plugins and doesn't store their secrets.
