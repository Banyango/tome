---
name: tome
description: Write, run and debug tome workflows. Use when the user mentions tome, `.tome/workflows`, a workflow that should start on a file change, a schedule or a message, or wants several agents to work in parallel in tmux or cmux.
---

# tome

tome runs repeatable agentic workflows. A workflow is a Markdown file with YAML
frontmatter: the frontmatter says what starts it, and the body describes the
steps in plain English. The tome daemon starts an orchestrator agent for each
run, and the orchestrator spawns worker agents in tmux or cmux sessions.

Full docs as one plain-text file: https://banyango.github.io/tome/llms-full.txt

## Before you start

```sh
tome --version          # installed? If not: https://banyango.github.io/tome/md/agents/install.md
tome daemon status      # exit code 3 means it isn't running
tome daemon start
```

Run commands from inside the project. Project workflows are found by looking
upwards from the current directory.

## Write a workflow

Workflows live in `.tome/workflows/<name>.md` (one project) or
`~/.tome/workflows/<name>.md` (every project).

```sh
tome workflow new my-workflow   # writes a starter file
tome validate my-workflow       # always run after editing; exit code 2 on errors
```

```markdown
---
name: my-workflow                 # required: letters, digits, _ and -
description: One line for listings.
params:
  target:
    type: string                  # string (default), int, float, bool
    default: src                  # no default = required
triggers:
  - manual
  - file: "features/*/feature.md"
    on: [created]
---
Say what the run is for. Use {{params.target}}, {{run.id}} and, for
triggered runs, {{trigger.paths}}, {{trigger.payload}} and so on.

## First step
What to do, what counts as done, and what happens if it fails.

## Second step
...
```

Rules that matter:

- The body is read by the orchestrator agent, not parsed. `##` headings become
  the steps reported in `tome runs show`.
- For each step, say what done looks like and what to do on failure. Without
  that, a failed step fails the run.
- To fan out, tell the orchestrator to create a group, spawn named workers in
  it (optionally each on its own git worktree), and wait for the group.
- `tome validate` rejects unknown frontmatter keys and `{{placeholders}}` that
  name no param.
- Other frontmatter: `defaults` (`backend`, `harness`, `orchestrator_harness`,
  `layout`, `start_timeout`), `concurrency` with `on_conflict: queue|reject`,
  and `orchestrator` (extra instructions for the orchestrator only).

## Triggers

```yaml
triggers:
  - manual                        # documentation only
  - file: "docs/**/*.md"          # on, debounce, ignore, while_running
  - cron: "0 9 * * 1-5"           # five fields, local time
  - on: review.requested          # a bus topic; * = one word, ** = one or more at the end
    to: new                       # new | running | running-or-new
```

- A run that edits files its own file trigger watches starts another run. Use
  `ignore:`, `on: [created]` or `while_running: mute`.
- tome publishes `tome.run.<workflow>.started|succeeded|failed|cancelled`.
  Subscribe to one to chain workflows. Subscriptions work in project
  workflows only.

```sh
tome publish feature.planned my-slug      # publish to the bus
tome publish some.topic "..." --dry-run   # which workflows would get it
tome triggers ls                          # armed triggers
tome triggers fire my-workflow --dry-run  # test without touching files
tome triggers fire my-workflow --path docs/a.md
tome triggers fire my-workflow --payload "hello"
```

## Run and inspect

```sh
tome run my-workflow --param target=lib   # attached; Ctrl-C cancels
tome run my-workflow --detach             # prints the run id
tome runs list --status running
tome runs show <id>                       # steps, workers, worktrees, sessions
tome runs logs <id> --tail 100
tome run cancel <id>
tome query "select id, workflow_name, status, reason from runs order by id desc limit 5"
```

Add `--json` to any command, or set `TOME_OUTPUT=json`, for machine-readable
output. Prefer it when you parse results.

## When something goes wrong

- `orchestrator_exited` / `worker_exited`: the agent's session ended without
  reporting. Read `tome runs logs <id>`.
- "never made a first tome call": the harness started too slowly. Raise
  `defaults.start_timeout`, or set it to `off`.
- Workflow not found: run `tome validate` with no name from inside the project.
- Trigger not firing: check `tome daemon status`, `tome triggers ls`, then
  `tome triggers fire <workflow> --dry-run`.
- Stuck bus delivery: `tome events ls`, then `tome events retry` or `remove`.

Don't guess at flags: `tome --help` and `tome <command> --help` list them all.
