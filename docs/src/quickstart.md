# Quickstart

In this walkthrough you set up a workflow that runs on its own: whenever you add a feature file to your project, agents investigate the codebase and break the feature into tasks. You need tmux, Claude Code, and tome [installed](install.md), and a git repository with some code in it.

## 1. Start the daemon

The daemon watches for triggers and starts runs. Start it in the background:

```sh
tome daemon start
tome daemon status
```

`status` prints whether it is running, and exits with code 3 if it isn't. To have it start at login, run `tome daemon install` once. See [Operating tome](guides/operating.md).

## 2. Add the workflow

tome looks for workflows in `.tome/workflows` in your project. Create a starter file:

```sh
cd my-project
tome workflow new plan-feature
```

That writes `.tome/workflows/plan-feature.md`. Replace its contents with this:

```markdown title=.tome/workflows/plan-feature.md
{{#include ../examples/plan-feature.md}}
```

The frontmatter holds the trigger: a `feature.md` created under `features/`. The body is plain English, split into steps under `##` headings. tome never parses it; an orchestrator agent reads it. `{{trigger.paths}}` is replaced with the new files when the run starts.

Check it, and confirm the daemon has picked up the trigger:

```sh
tome validate plan-feature
tome triggers ls
```

## 3. Write down a feature

Describe a small change you actually want in this project. For example:

```sh
mkdir -p features/export-csv
cat > features/export-csv/feature.md <<'EOF'
# Export to CSV

Add a `--csv` flag to the export command. It writes the same rows as the
default output, as CSV with a header line.
EOF
```

That's all you do. A few seconds later the daemon starts a run:

```sh
tome runs list
```

The run works like this:

1. tome starts an orchestrator agent in a tmux session and gives it the workflow, with `features/export-csv/feature.md` filled in.
2. The orchestrator reports each step, and spawns a worker agent named `investigate-export-csv`.
3. The worker reads the code the feature touches and reports what it found, without changing anything.
4. The orchestrator writes `features/export-csv/tasks.md` and publishes a `feature.planned` event.

## 4. Watch it

Each agent runs in its own tmux session. In another terminal:

```sh
tmux ls
tmux attach -t <session>
```

You can type into an agent's session at any time, to answer a question or steer it. Detach with `Ctrl-b d`.

To see a run's steps, history and logs:

```sh
tome runs show <id>
tome runs logs <id>
```

When the run succeeds, read `features/export-csv/tasks.md`. Change anything you disagree with; it's your plan now.

## 5. Keep going

Every feature file you add from now on gets the same treatment. Nothing subscribes to `feature.planned` yet, so for now that event goes nowhere. Add the [`implement-feature`](recipes.md#plan-a-feature-then-build-it) recipe and each planned feature is built too, one agent and one branch per task. To build the feature you just planned, publish the event again yourself:

```sh
tome publish feature.planned export-csv
```

To try a trigger without touching files, fire it by hand:

```sh
tome triggers fire plan-feature --path features/export-csv/feature.md
```

You can also start any workflow yourself with `tome run <name>`. It stays attached and prints progress until the run ends.

## Next

- [Recipes](recipes.md): implementing planned features, fixing failing tests, reviewing branches, and more.
- [Triggers and the message bus](guides/triggers.md): schedules, messages, and chaining workflows together.
- [Write a workflow](guides/write-a-workflow.md): every frontmatter option.
