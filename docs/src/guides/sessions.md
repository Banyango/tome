# Sessions and backends

Every agent tome starts runs in a **session** on a **backend**. This guide covers choosing the backend, adding agent harnesses, and deciding where sessions open.

## Choosing a backend

tome supports tmux, cmux and herdr. It picks one for a run in this order:

1. `defaults.backend` in the workflow
2. the `TOME_BACKEND` environment variable
3. `backend:` in `.tome/config.yaml` (project) or `~/.tome/config.yaml` (global)
4. cmux, if the daemon runs inside cmux; otherwise herdr, if the daemon or caller is inside herdr; otherwise tmux

### tmux

Sessions are ordinary tmux sessions on your default tmux server. List them with `tmux ls` and attach with `tmux attach -t <name>`. tmux doesn't need to be running when you start a run.

### cmux

Sessions are tabs, splits and workspaces in the cmux app. cmux only lets its own processes control it, so **start the daemon from a terminal inside cmux**: `tome daemon start`. The daemon inherits access from there. A daemon started elsewhere can't open sessions in cmux.

### herdr

Sessions open as tabs, splits and workspaces in the herdr app. tome talks to herdr over its local socket, found from `HERDR_SOCKET_PATH`, `HERDR_SESSION`, then `herdr.session` in `~/.tome/config.yaml`, then the default socket (`~/.config/herdr/herdr.sock`), in that order.

To pin a herdr session, set it in the global config (it isn't accepted in a project's config):

```yaml title=~/.tome/config.yaml
herdr:
  session: work
```

Things that differ from the other backends:

- **Unavailable.** If herdr isn't running, a run fails with `backend_unavailable`, and you get a notification. A daemon that can't reach herdr later doesn't treat that as the agent exiting; it checks again.
- **Agent status.** The daemon reads each agent's status from herdr. `tome runs show` and `tome worker status` list it in an `AGENT` column. When an agent is waiting for input it shows `blocked since <time>`.
- **Blocked agents.** tome publishes `tome.run.<workflow>.blocked` (see [Triggers](triggers.md#chaining-workflows)) when an agent becomes blocked. If it stays blocked for 5 seconds you get a herdr notification, repeated at most once a minute. Notifications also announce a run's end. Set `TOME_NOTIFY=off` to turn them off.
- **Attaching.** `tome session view` focuses the pane in herdr. Herdr sessions on another node can't be viewed over SSH.

## Harnesses

A harness is a command template that tome uses to launch an agent CLI with a prompt. `claude` (Claude Code) is built in: it runs `claude --allowedTools "Bash(tome:*)" -- "<prompt>"`, so the agent can call `tome` without asking you each time.

To add or change harnesses, put a `harnesses:` block in `~/.tome/config.yaml` or the project's `.tome/config.yaml`:

```yaml
harnesses:
  claude-opus:
    command: ["claude", "--model", "opus", "--allowedTools", "Bash(tome:*)", "--", "{{prompt}}"]
  aider:
    command: aider --message-file {{prompt_file}}
  codex:
    command: ["codex", "{{prompt}}"]
    model_flag: --model
```

A harness chooses the agent CLI; `backend` chooses the terminal multiplexer that hosts it. The `codex` harness can run inside either tmux or cmux. Install the agent CLI and complete its sign-in before using it with tome. Installing the [tome plugin](../agents/skill.md) teaches the agent how to use tome; configure its launch command here as well.

A list is an argv: one element per argument, with no shell involved. A string is run with `sh -c`, with each value shell-quoted. The variables are:

| Variable | Value |
| --- | --- |
| `{{prompt}}` | the agent's instructions |
| `{{prompt_file}}` | a file that holds them |
| `{{run_id}}` | the run's id |
| `{{session}}` | the session's name |
| `{{cwd}}` | the directory the agent starts in |
| `{{model}}` | the workflow's model for this agent, or empty |

### Models

A workflow picks a model with `defaults.model` (a single run's agent, and workers) and `defaults.orchestrator_model` (the orchestrator, defaulting to `model`). tome passes it to the harness in one of two ways:

- `model_flag`, for a harness whose command is a list: tome inserts `<flag> <model>` right after the program. The built-in `claude` harness has `model_flag: --model`.
- `{{model}}`, a variable you put in the command wherever the agent wants the model. It is empty when no model is set.

```yaml
harnesses:
  codex:
    command: ["codex", "{{prompt}}"]
    model_flag: --model
  aider:
    command: aider --message-file {{prompt_file}} --model {{model}}
```

A harness that does neither refuses a model, and `tome run` and `tome worker spawn` say so. If you redefine `claude` in config, add `model_flag: --model` again, or the model can't reach it. `tome worker spawn --model <m>` overrides the workflow's model for one worker.

A project harness replaces a global one of the same name. Pick a harness in a workflow with `defaults.harness` (a single run's agent, and workers) and `defaults.orchestrator_harness` (the orchestrator), or per worker with `tome worker spawn --harness`.

To change the harness or model for one run without editing the workflow:

```sh
tome run my-workflow --harness codex
tome run my-workflow --harness codex --model <model-name>
```

`--harness` overrides both `defaults.harness` and `defaults.orchestrator_harness`; `--model` overrides both `defaults.model` and `defaults.orchestrator_model`. These choices apply to the main agent and workers. A worker's explicit `--harness` or `--model` still takes precedence. Each run flag overrides only its own setting: changing the harness keeps the workflow's models unless you also pass `--model`. Use a model accepted by the selected agent CLI.

Any agent works as long as it can run `tome` commands. An agent must call `tome ready` first and `tome worker done` or `fail` last, and tome tells it so in its prompt.

If a harness won't open, run `tome harness validate <name>` to check its
configuration, find its executable on the current `PATH`, and see the command
tome will launch. Add `--model <name>` to check model forwarding too. This
prints a preview without starting the agent; the agent itself opens inside the
configured session backend.

```sh
tome harness validate                 # every known harness, including claude
tome harness validate codex
tome harness validate codex --model <model-name>
```

Validation checks whether tome can forward a model, not whether the agent provider supports it. For a shell command template it checks `sh` and shell syntax; check the agent executable separately. It doesn't check sign-in or launch an agent. The command exits with code 2 if a check fails and supports `--json` for structured results.

## Placement

**Placement** is where a session opens. It has these settings:

| Setting | Values |
| --- | --- |
| `layout` | `tab` (the default) or `split` |
| `workspace` | `project` (the default: one shared workspace for the project), `focused` (whatever is focused when the run starts), `own` (a new workspace for each session), or a name, which becomes the workspace `<project>-<name>` |
| `split.direction` | `right`, `left`, `down` or `up` |
| `split.size` | a percentage like `30%`, or a number of cells |
| `from` | the pane a new tab or split opens from: `orchestrator`, `last`, `first`, or `caller` (cmux or herdr, for the run's agent or orchestrator: the pane where you ran `tome run`) |
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

Or in the workflow. Under `defaults.layout`, `agent` sets a single run's agent's placement, `orchestrator` sets the orchestrator's, `workers` sets the workers', and a workers list matches each worker's name against a glob, first match wins:

```markdown title=.tome/workflows/review-layout.md
{{#include ../../examples/layout.md}}
```

Each setting resolves on its own, from the most specific source down. A source that sets only `direction` leaves every other setting to the next one.

| Order | For workers | For the orchestrator | For a single run's agent |
| --- | --- | --- | --- |
| 1 | `tome worker spawn` flags | `tome run` flags | `tome run` flags |
| 2 | the first matching rule under `workers` | the `orchestrator` block | the `agent` block |
| 3 | the `workers` block | | |
| 4 | `tome run` flags | | |

After that, all of them use `defaults.layout` and its preset, then `TOME_LAYOUT`, the project's `.tome/config.yaml`, the global config, and the built-in default. By default the orchestrator gets a tab in the project workspace, and each worker splits below the run's last pane (`split`, `direction: down`, `from: last`), so a run reads as one tab. A worker that sets a `workspace` but no layout keeps a tab, since the orchestrator isn't there to split from.

`from: caller` is for the run's agent or orchestrator only. A worker's rule can't set it, and `tome worker spawn` refuses it.

`tome runs show <id>` shows where each session ended up, and which source each setting came from, plus any warnings. Split sizes are best effort.

### Moving a session

A live session can be moved without restarting it. Settings you don't give stay as they were:

```sh
tome session move 12/orchestrator --layout split --direction right
tome session move 12/review-1 --workspace own
```

A single-agent run's main session is `<run>/agent`, such as `12/agent`.
