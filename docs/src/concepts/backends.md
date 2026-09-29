# Backends

A **backend** is the terminal multiplexer tome uses as its user interface. tome starts every agent in a **session** on a backend, so you can watch it, type into it, and attach from anywhere.

tome supports two backends:

| Backend | Sessions are |
| --- | --- |
| `tmux` | tmux sessions, panes and windows. Attach with `tmux attach -t <name>`. |
| `cmux` | tabs, splits and workspaces in the cmux app. |

A run's backend comes from, in order: `defaults.backend` in the workflow, the `TOME_BACKEND` environment variable, `backend:` in `.tome/config.yaml` or `~/.tome/config.yaml`, then cmux if the daemon runs inside cmux, and tmux otherwise.

## Sessions

A **session** is one terminal that runs an agent or a command. The orchestrator and each worker get their own. When an agent finishes, its session closes, unless the worker was started with `--keep-open`.

Where a session opens is its **placement**: a layout (`tab` or `split`, or a workspace of its own), a workspace, a split direction and size, and an anchor pane. Placement can be set on the command line, in presets, and in the workflow. See [Sessions and backends](../guides/sessions.md).

## Harnesses

An agent starts through a **harness**: a command template that launches an agent CLI with a prompt. Claude Code (`claude`) is built in. Others are defined in config, so tome isn't tied to one agent. Workflows pick one with `defaults.harness`, and can give the orchestrator a different one with `defaults.orchestrator_harness`.
