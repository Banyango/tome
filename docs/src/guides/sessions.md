# Sessions and backends

Every agent tome starts runs in a **session** on a **backend**. This guide covers choosing the backend, adding agent harnesses, and deciding where sessions open.

## Choosing a backend

tome supports tmux and cmux. It picks one for a run in this order:

1. `defaults.backend` in the workflow
2. the `TOME_BACKEND` environment variable
3. `backend:` in `.tome/config.yaml` (project) or `~/.tome/config.yaml` (global)
4. cmux, if the daemon runs inside cmux, and tmux otherwise

### tmux

Sessions are ordinary tmux sessions on your default tmux server. List them with `tmux ls` and attach with `tmux attach -t <name>`. tmux doesn't need to be running when you start a run.

### cmux

Sessions are tabs, splits and workspaces in the cmux app. cmux only lets its own processes control it, so **start the daemon from a terminal inside cmux**: `tome daemon start`. The daemon inherits access from there. A daemon started elsewhere can't open sessions in cmux.

## Harnesses

A harness is a command template that tome uses to launch an agent CLI with a prompt. `claude` (Claude Code) is built in: it runs `claude --allowedTools "Bash(tome:*)" -- "<prompt>"`, so the agent can call `tome` without asking you each time.

To add or change harnesses, put a `harnesses:` block in `~/.tome/config.yaml` or the project's `.tome/config.yaml`:

```yaml
harnesses:
  claude-opus:
    command: ["claude", "--model", "opus", "--allowedTools", "Bash(tome:*)", "--", "{{prompt}}"]
  aider:
    command: aider --message-file {{prompt_file}}
```

A list is an argv: one element per argument, with no shell involved. A string is run with `sh -c`, with each value shell-quoted. The variables are:

| Variable | Value |
| --- | --- |
| `{{prompt}}` | the agent's instructions |
| `{{prompt_file}}` | a file that holds them |
| `{{run_id}}` | the run's id |
| `{{session}}` | the session's name |
| `{{cwd}}` | the directory the agent starts in |

A project harness replaces a global one of the same name. Pick a harness in a workflow with `defaults.harness` (workers) and `defaults.orchestrator_harness` (the orchestrator), or per worker with `tome worker spawn --harness`.

Any agent works as long as it can run `tome` commands. An agent must call `tome ready` first and `tome worker done` or `fail` last, and tome tells it so in its prompt.

## Placement

**Placement** is where a session opens. It has these settings:

| Setting | Values |
| --- | --- |
| `layout` | `tab` (the default) or `split` |
| `workspace` | `project` (the default: one shared workspace for the project), `focused` (whatever is focused when the run starts), `own` (a new workspace for each session), or a name, which becomes the workspace `<project>-<name>` |
| `split.direction` | `right`, `left`, `down` or `up` |
| `split.size` | a percentage like `30%`, or a number of cells |
| `from` | the pane a new tab or split opens from: `orchestrator`, `last`, `first`, or `caller` (cmux only, orchestrator only: the pane where you ran `tome run`) |
| `preset` | the name of a preset to build on |

`layout: workspace` still works and means `workspace: own`. In tmux, a workspace is a tmux session, and a tab is a window.

### Presets

A preset is a named bundle of placement settings. `tab`, `split` and `workspace` are built in. Define more in config:

```yaml
layout_presets:
  wide:
    layout: split
    split: { direction: right, size: 40% }
```

`tome layout presets` lists all of them. Settings next to `preset:` override the preset's own.

### Setting placement

Give a run its placement on the command line:

```sh
tome run my-workflow --preset wide
tome worker spawn --name review-1 --layout split --direction down --size 30% --prompt "..."
```

Or in the workflow. Under `defaults.layout`, `orchestrator` sets the orchestrator's placement, `workers` sets the workers', and a workers list matches each worker's name against a glob, first match wins:

```markdown title=.tome/workflows/review-layout.md
{{#include ../../examples/layout.md}}
```

Each setting resolves on its own, from the most specific source down. A source that sets only `direction` leaves every other setting to the next one.

| Order | For workers | For the orchestrator |
| --- | --- | --- |
| 1 | `tome worker spawn` flags | `tome run` flags |
| 2 | the first matching rule under `workers` | the `orchestrator` block |
| 3 | the `workers` block | |
| 4 | `tome run` flags | |

After that, both use `defaults.layout` and its preset, then `TOME_LAYOUT`, the project's `.tome/config.yaml`, the global config, and the built-in default (a tab in the project workspace).

`from: caller` is for the orchestrator only. A worker's rule can't set it, and `tome worker spawn` refuses it.

`tome runs show <id>` shows where each session ended up, and which source each setting came from, plus any warnings. Split sizes are best effort.

### Moving a session

A live session can be moved without restarting it. Settings you don't give stay as they were:

```sh
tome session move 12/orchestrator --layout split --direction right
tome session move 12/review-1 --workspace own
```
