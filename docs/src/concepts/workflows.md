# Workflows, runs and steps

## Workflow

A **workflow** is a Markdown file that describes a process. It has two parts:

- **Frontmatter**: YAML between `---` lines. It names the workflow and sets parameters, defaults, triggers and limits.
- **Body**: plain English. tome never parses it. An agent reads it.

Workflows live in two places:

| Location | Scope |
| --- | --- |
| `.tome/workflows/` in a project | that project |
| `~/.tome/workflows/` | every project |

If a project workflow and a global one share a name, the project one wins. That makes copying a workflow to a new project a matter of copying one file. `tome workflow new <name>` writes a starter file, `tome workflow ls` lists the ones you can run, and `tome validate` checks your files and reports problems with line numbers.

## Step

A **step** is a named unit of work: a `## Heading` in the workflow body, written in plain English. Steps are not a fixed script. The run's agent decides the order, and it can branch, loop or retry as the text says.

The run's agent reports each step to tome with `tome step start "<name>"`, then `tome step done` or `tome step fail`. That is how a step's status and history end up in the run's record.

## Mode: one agent or an orchestrator

Each run is carried out by one main agent. tome starts it in its own visible session, gives it a built-in prompt (the tome commands it may use and its duties), and gives it the workflow body. The frontmatter's `mode` says which kind of agent that is:

| `mode` | The run's agent | Can it delegate? |
| --- | --- | --- |
| `single` (the default) | one **agent** (session role `agent`) that does the whole workflow itself | no: `tome worker`, `tome group` and `tome worktree create` are refused |
| `orchestrated` | an **orchestrator** that coordinates | yes: it spawns [workers](primitives.md#worker), groups and worktrees |

Either way the agent reports steps and must end the run with `tome run finish`. Both can use queues, `tome publish` and `tome events`. A single agent runs `defaults.harness` and `defaults.model`; an orchestrator runs `defaults.orchestrator_harness` and `defaults.orchestrator_model`, which fall back to those.

Use `single` unless the workflow fans out to workers. Set `mode: orchestrated` when it does. `tome validate` warns when a single workflow's body talks about workers, worktrees or fanning out, or when it sets keys only an orchestrator uses.

If the agent exits without finishing, the run fails with the reason `agent_exited` (`orchestrator_exited` for an orchestrator).

## Run

A **run** is one execution of a workflow. Start one with `tome run <workflow>`, or let a [trigger](triggers.md) start it. A run has a status:

| Status | Meaning |
| --- | --- |
| `queued` | waiting for a free slot |
| `running` | the run's agent is working |
| `succeeded` | the agent finished it successfully |
| `failed` | the agent failed it, or it stopped without finishing |
| `cancelled` | someone cancelled it |

A failed or cancelled run can be resumed with `tome run resume <id>`. That starts a new run from the old one's saved workflow, and its agent is told what the earlier run finished and where to start. See [Resuming a failed run](../guides/operating.md#resuming-a-failed-run).

`concurrency` in the frontmatter caps how many runs of a workflow go at once. When it is full, `on_conflict: queue` (the default) makes a new run wait, and `on_conflict: reject` refuses it.

When the run starts, tome fills in the placeholders in the body: `{{params.<name>}}`, `{{run.id}}` and, for runs started by a trigger, `{{trigger.<field>}}`. The filled-in workflow is saved with the run.

Run state is the only thing tome stores. It lives in an embedded DuckDB database with each run's steps, history and logs. Inspect it with `tome runs`, or query it with `tome query`.

Files on disk and git [worktrees](primitives.md#worktree) carry data between steps.
