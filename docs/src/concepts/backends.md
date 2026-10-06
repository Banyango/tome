# Backends

A **backend** is the terminal multiplexer tome uses as its user interface. tome starts every agent in a **session** on a backend, so you can watch it, type into it, and attach from anywhere.

tome supports three backends:

| Backend | Sessions are |
| --- | --- |
| `tmux` | tmux sessions, panes and windows. Attach with `tmux attach -t <name>`. |
| `cmux` | tabs, splits and workspaces in the cmux app. |
| `herdr` | workspaces, tabs and splits in the herdr app. |

A run's backend comes from, in order: `defaults.backend` in the workflow, the `TOME_BACKEND` environment variable, `backend:` in `.tome/config.yaml` or `~/.tome/config.yaml`, then cmux if the daemon runs inside cmux, herdr if it runs inside herdr, and tmux otherwise. Herdr is also chosen when `tome run` is started from a herdr pane, even if the daemon isn't inside herdr. tome finds the herdr server from `HERDR_SOCKET_PATH`, `HERDR_SESSION`, then `herdr.session` in `~/.tome/config.yaml`, then the default socket (`~/.config/herdr/herdr.sock`), in that order. See [Sessions and backends](../guides/sessions.md#herdr).

## Sessions

A **session** is one terminal that runs an agent or a command. The orchestrator and each worker get their own. When an agent finishes, its session closes, unless the worker was started with `--keep-open`.

Where a session opens is its **placement**: a layout (`tab` or `split`, or a workspace of its own), a workspace, a split direction and size, and an anchor pane. Placement can be set on the command line, in presets, and in the workflow. See [Sessions and backends](../guides/sessions.md).

## Harnesses

An agent starts through a **harness**: a command template that launches an agent CLI with a prompt. Claude Code (`claude`) is built in. Others are defined in config, so tome isn't tied to one agent. Workflows pick one with `defaults.harness`, and can give the orchestrator a different one with `defaults.orchestrator_harness`.

For example, configure a `codex` harness and use `tome run my-workflow --harness codex` to select it for one run. `--model` selects a model for that run. Both flags apply to the main agent and workers, including the orchestrator in an orchestrated workflow. The harness runs on the chosen tmux or cmux backend. See [Harnesses](../guides/sessions.md#harnesses) for configuration and validation.
