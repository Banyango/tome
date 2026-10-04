---
name: tome
description: Write, run and debug tome workflows. Use when the user mentions tome, `.tome/workflows`, a workflow that should start on a file change, a schedule or a message, or wants several agents to work in parallel in tmux or cmux.
---

# tome

tome runs repeatable agentic workflows. A workflow is a Markdown file with YAML
frontmatter: the frontmatter says what starts it, and the body describes the
steps in plain English. The tome daemon starts one agent for each run in a
tmux or cmux session. By default (`mode: single`) that agent does the whole
workflow itself; with `mode: orchestrated` it is an orchestrator that spawns
worker agents.

Full docs as one plain-text file: https://banyango.github.io/tome/llms-full.txt

## Before you start

```sh
tome --version          # installed? If not: https://banyango.github.io/tome/md/agents/install.md
tome daemon status      # exit code 3 means it isn't running
tome daemon start
```

Run commands from inside the project. Project workflows are found by looking
upwards from the current directory.

## Ask before you write a workflow

Don't guess at how a workflow should behave. A wrong guess costs the user a
run: too many agents for their machine, a param they don't understand, or
work that stops halfway. Before writing a new workflow, or changing how one
splits up work, ask the user the questions below that the request and the
repo don't already answer. Ask them together, in one message, with a
suggested answer for each, and use your question tool if you have one.

1. **What starts it?** Run by hand, a file change, a schedule, or a message
   from another workflow.
2. **How many agents at once?** Each agent is a full harness session, and
   most laptops handle 1 or 2 well. Suggest 1. Never start more workers at a
   time than the user said.
3. **How is the work split?** One agent doing every item in turn, one agent
   per item, or a fixed number of agents taking items from a queue. With a
   limit of 1 or 2, one agent per item, started in turn, is usually simplest.
   One agent doing everything is `mode: single` (the default), and the
   questions about worker names don't apply. Anything that spawns workers is
   `mode: orchestrated`.
4. **Where do changes go?** Straight into the current branch, one branch per
   item, and whether the orchestrator merges after the tests pass or leaves
   branches for review.
5. **When does it stop?** After one item, after N items, or when nothing is
   left.
6. **How do you check the work?** The test, type and lint commands, if you
   can't find them in the repo.
7. **What should the workers be called?** A worker's name is what the user
   sees in its tab title (`tome: <workflow> #<run> / <name>`), in
   `tome runs show`, and in its default branch name. Suggest a
   pattern that fits how the work is split, with an example from this repo:
   - one worker per item: the item's id and a short slug, such as
     `t004-renderer` or `fix-login-redirect`
   - a pool taking from a queue: its role and a number, such as `builder-1`
   - one worker per angle or role: the role, such as `review-security` or
     `tests`

   Names are letters, digits, `_` and `-`, at most 64 characters and unique
   in a run. Keep them under about 25 characters so tab titles stay readable.
8. **How should branches be named?** Ask only if the workflow makes git
   branches, through worker worktrees or by telling an agent to branch.
   Look at the repo's branches (`git branch -a`) first and suggest the
   convention it already uses. The choices are usually:
   - tome's default, `tome/<run>/<worker>`: unique for every run, and easy to
     find and clean up with `tome gc`
   - the team's convention, such as `feature/<slug>`, `task/<id>-<slug>` or
     `<user>/<slug>`, when the branches become pull requests

   Pass the name with `--branch` on `tome worker spawn --worktree` or
   `tome worktree create`. Creating a branch that already exists fails, so a
   custom pattern needs something unique in it, such as the task id, or
   `{{run.id}}` when the same item can come round again.

9. **Which model should the agents use?** Ask, and don't pick for them: the
   choice trades cost and speed against quality. A single-agent workflow
   takes one answer, set with `defaults.model`. For an orchestrated one, ask
   about the workers and the orchestrator separately, since the orchestrator mostly coordinates and
   often does fine on a cheaper model while workers doing the hard work may
   want a stronger one. Suggest the model this session is running, or say
   that leaving it out uses the harness's own default. Set the answers with
   `defaults.model` (workers) and `defaults.orchestrator_model`, and a single
   worker can differ with `tome worker spawn --model <m>`. Use the names the
   harness understands (`opus`, `sonnet`, `haiku` for `claude`). A custom
   harness only takes a model if it has `model_flag` or `{{model}}` (see the
   docs), so `tome validate` and `tome run` refuse one that can't.

Then say back, in two or three lines, what the workflow will do (for example,
"one agent at a time, one branch per task, merged when tests pass, stops when
no task is ready, branches named `task/<id>-<slug>`, sonnet for the workers") before you write it. For the first run, suggest a small
one, such as a single item, so the user sees it work before it does
everything.

Turn the answers into params with real defaults, so the next run needs no
questions, and write the worker and branch naming into the step that spawns
workers, with `--name` and `--branch` in the example command:

- Use `type: int` for numbers, and a default that's safe on its own, such as
  `workers: {type: int, default: 1}`.
- Don't use sentinel values like `0` for "no limit". Pick a real default, or
  leave the limit out.
- tome doesn't cap workers for you. Write the cap into the body, in the step
  that spawns them: "Never have more than {{params.workers}} workers running.
  Start the next when one finishes."

## Write a workflow

Workflows live in `.tome/workflows/<name>.md` (one project) or
`~/.tome/workflows/<name>.md` (every project).

```sh
tome workflow ls                # what can `tome run` see? (--global for ~/.tome only)
tome workflow new my-workflow   # writes a starter file
tome validate my-workflow       # always run after editing; exit code 2 on errors
```

```markdown
---
name: my-workflow                 # required: letters, digits, _ and -
description: One line for listings.
mode: single                      # single (default): one agent does it all;
                                  # orchestrated: an orchestrator spawns workers
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

- The body is read by the run's agent, not parsed. `##` headings become
  the steps reported in `tome runs show`.
- For each step, say what done looks like and what to do on failure. Without
  that, a failed step fails the run.
- A workflow that delegates needs `mode: orchestrated`. In the default
  `mode: single`, `tome worker`, `tome group` and `tome worktree create` are
  refused, and `tome validate` warns when the body mentions them.
- To fan out, set `mode: orchestrated` and tell the orchestrator to create a group, spawn named workers in
  it (optionally each on its own git worktree), and wait for the group.
- `tome validate` rejects unknown frontmatter keys and `{{placeholders}}` that
  name no param.
- Other frontmatter: `defaults` (`backend`, `harness`, `orchestrator_harness`,
  `model`, `orchestrator_model`, `layout`, `start_timeout`), `concurrency` with `on_conflict: queue|reject`,
  and `orchestrator` (extra instructions for the orchestrator only). The
  `orchestrator_*` keys only apply to `mode: orchestrated`.

## Queues

A worker does one job and reports once. Its session closes after
`tome worker done`, so a worker that pulls from a queue stops after one
message unless its prompt tells it to loop. When a worker should work
through a queue, its prompt has to say all of this:

```markdown
Work through the queue `tasks` until it is closed and empty:
1. `tome queue pull tasks --wait 1m`. Exit 3 with status `closed` means
   there's nothing left: go to step 4. Status `empty` means pull again.
2. Do the item, then `tome queue ack <id>`.
3. Go back to step 1. Don't report yet.
4. Report once, with every item and its result.
```

And the orchestrator has to `tome queue close tasks` after its last push, or
the workers wait forever. A closed queue can't be reopened, so a workflow
that loops and pushes more items later needs a new queue name each time
round, such as `tasks-<round>`.

Queues belong to the project, not the run: they outlive runs, and a run, a
worker or the user at a shell can push to and pull from the same queue.
Names are shared across runs, so pick ones that won't collide. `tome queue
peek <q>` shows what's waiting without claiming it.

If each item gets its own worker anyway, don't use a queue: put the item in
the worker's prompt.

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

## Other machines (nodes)

Nodes are machines reached over SSH, listed under `nodes:` in
`~/.tome/config.yaml` (`tome node add <name> <ssh-destination>`). Add
`--on <node>` to run a daemon command there. A workflow is resolved by name on
the node, so commit and pull it there first.

```sh
tome node ls                               # reachable? version? daemon?
tome node check mini                       # what's wrong, and the fix
tome run build --on mini --detach          # `build` as the node resolves it
tome runs show mini:12                     # <node>:<id> names a node's run
tome runs list --nodes                     # every machine, with a NODE column
tome session view mini:12                  # attach to its tmux session
tome publish --on mini deploy.requested "1.4.2"   # hand work to another machine
```

To forward a project's events to a node, add rules to its
`.tome/config.yaml`:

```yaml
bus:
  forward:
    - topic: tome.run.build.succeeded
      to: mini            # or [mini, ci]
```

Agent commands (`ready`, `step`, `worker`, `run finish`) refuse `--on`. So do
local ones (`validate`, `workflow`, `layout`, `node`). Inside a run,
`TOME_NODE` is ignored.

## When something goes wrong

- `agent_exited` / `orchestrator_exited` / `worker_exited`: the agent's session ended without
  reporting. Read `tome runs logs <id>`.
- `tome worker ... isn't available: this run is a single-agent run`: add
  `mode: orchestrated` to the workflow's frontmatter.
- "never made a first tome call": the harness started too slowly. Raise
  `defaults.start_timeout`, or set it to `off`.
- Workflow not found: run `tome validate` with no name from inside the project.
- Trigger not firing: check `tome daemon status`, `tome triggers ls`, then
  `tome triggers fire <workflow> --dry-run`.
- Stuck bus delivery: `tome events ls`, then `tome events retry` or `remove`.
- A node can't be reached (exit 3): `tome node check <node>`.

Don't guess at flags: `tome --help` and `tome <command> --help` list them all.
