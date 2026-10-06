# Write a workflow

This page is the reference for workflow files. You rarely need to write one yourself: ask your coding agent, with the tome plugin installed, and it writes and validates the file for you. See [Create workflows with an agent](create-with-an-agent.md). Use this page to check what it wrote, or to make a small change by hand.

A workflow is a Markdown file with YAML frontmatter. Put it in `.tome/workflows/` for one project or `~/.tome/workflows/` for all of them. `tome workflow new <name>` writes a starter file in the right place, and `tome validate [name]` checks it.

Every example on this page is a real file under `docs/examples/` in the repo, and CI runs `tome validate` on each one.

## The smallest workflow

Only `name` is required.

```markdown title=.tome/workflows/minimal.md
{{#include ../../examples/minimal.md}}
```

Everything after the closing `---` is the body. You write it in plain English. The run's agent reads it and decides what to do, in what order. tome never parses it, except to fill in placeholders and to check them.

## Steps

Use `## Headings` to name the steps. The run's agent reports each one to tome as it starts and finishes, and you see them in `tome runs show`. Steps are not a rigid script: the text can tell the agent to retry, loop or skip, and it will.

Say what each step does, what counts as done, and what should happen if it fails. If nothing says otherwise, a failed step fails the run.

## Frontmatter fields

`tome validate` rejects keys it doesn't know, and points to the line.

### `name`

Required. Letters, digits, `_` and `-`. It's what you pass to `tome run`.

### `description`

One line, shown in listings.

### `mode`

`single` (the default) or `orchestrated`.

- `single`: tome starts one agent that does the whole workflow itself. It can report steps, use queues and publish events, but it can't spawn workers, groups or worktrees; those commands are refused.
- `orchestrated`: tome starts an orchestrator that hands the work to [workers](workers.md). Use it for any workflow that fans out.

`tome validate` warns when a `single` workflow's body talks about workers, worktrees or fanning out, and when it sets keys only an orchestrator uses (`orchestrator_harness`, `orchestrator_model`, and the `orchestrator` and `workers` placement blocks). See [Workflows, runs and steps](../concepts/workflows.md#mode-one-agent-or-an-orchestrator).

### `params`

Named inputs, filled in from `--param key=value` on `tome run`, from a trigger's `params:`, or from the default. Each param has:

- `type`: `string` (the default), `int`, `float` or `bool`
- `default`: used when nothing sets it. A param without a default is required.
- `description`

```markdown title=.tome/workflows/summarize.md
{{#include ../../examples/params.md}}
```

```sh
tome run summarize --param file=notes.txt --param words=50
```

In the body, `{{params.<name>}}` inserts a value and `{{run.id}}` inserts the run's id. A placeholder that names no param is an error at validation time. In a workflow started by a trigger, `{{trigger.<field>}}` inserts details of the event; see [Triggers and the message bus](triggers.md).

### `defaults`

Workflow-wide settings.

```markdown title=.tome/workflows/defaults-tour.md
{{#include ../../examples/defaults.md}}
```

| Key | Meaning |
| --- | --- |
| `backend` | `tmux`, `cmux` or `herdr`. Falls back to `TOME_BACKEND`, then config, then cmux inside cmux, herdr inside herdr, and tmux otherwise. |
| `harness` | The agent CLI for a single run's agent and for workers. Defaults to `claude`. Defined in config; see [Sessions and backends](sessions.md). |
| `orchestrator_harness` | The agent CLI for the orchestrator (`mode: orchestrated` only). Defaults to `harness`. |
| `model` | The model a single run's agent and workers run, such as `opus`. Passed to the harness; unset uses the harness's own default. Override for one worker with `tome worker spawn --model`. |
| `orchestrator_model` | The model the orchestrator runs (`mode: orchestrated` only). Defaults to `model`. |
| `layout` | Where sessions open. A name (`tab`, `split`, `workspace`) or a block of settings; see [Sessions and backends](sessions.md). |
| `start_timeout` | How long the run's agent and each agent worker have to make their first `tome` call. A duration like `90s` or `2m`, or `off`. The default is 60s. If it passes, tome nudges the agent once and fails it after a second wait. |
| `timeout` | A duration, like `30m`, or a number of seconds. tome accepts and records it, but doesn't enforce a run timeout today. |
| `on_failure` | A string, accepted and recorded. tome doesn't act on it today; say in the body what should happen on failure. |

### `concurrency` and `on_conflict`

`concurrency: N` limits how many runs of this workflow go at once. Runs are unlimited by default. When the limit is reached, `on_conflict: queue` (the default) makes the new run wait, and `on_conflict: reject` refuses it.

### `orchestrator`

Extra instructions for the orchestrator only, added to its prompt. Use it for how the orchestrator should behave, and keep the body for what the workflow does.

```markdown title=.tome/workflows/one-at-a-time.md
{{#include ../../examples/limits.md}}
```

### `triggers`

A list of rules that start the workflow or signal its runs. This example has one of each kind:

```markdown title=.tome/workflows/triggers-tour.md
{{#include ../../examples/triggers-tour.md}}
```

Every field of a trigger is explained in [Triggers and the message bus](triggers.md).

## Check, then run

```sh
tome validate my-workflow
tome run my-workflow --param key=value
```

`tome validate` with no name checks every workflow it can see and shows which project workflows override global ones. It exits with code 2 if anything is wrong, and prints each problem with its line number.

For every command and flag, run `tome --help`.
