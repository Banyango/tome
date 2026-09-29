# Quickstart

This walks through a first run from start to finish with tmux and Claude Code. You need both installed, and tome [installed](install.md).

## 1. Start the daemon

The daemon starts runs and keeps track of them. Start it in the background:

```sh
tome daemon start
tome daemon status
```

`status` prints whether it is running. It exits with code 3 if it isn't. To have it start at login, run `tome daemon install` once. See [Operating tome](guides/operating.md).

## 2. Make a workflow

tome looks for workflows in `.tome/workflows` in your project, and in `~/.tome/workflows` for ones you want everywhere. Work in a git repository, and create a starter file:

```sh
cd my-project
tome workflow new hello --description "A first run"
```

That writes `.tome/workflows/hello.md`. Replace its contents with this example:

```markdown
{{#include ../examples/hello.md}}
```

The frontmatter sets the name, a parameter, and defaults: tmux as the backend and Claude Code as the harness. The body is plain English, split into steps under `##` headings. `{{params.who}}` and `{{run.id}}` are filled in when the run starts.

Check it:

```sh
tome validate hello
```

## 3. Run it

```sh
tome run hello --param who=Ada
```

The command stays attached and prints progress as the run moves. In the background:

1. tome starts an orchestrator agent in a tmux session and hands it the workflow.
2. The orchestrator reports each step with `tome step start`, and spawns a worker named `greeter`.
3. The worker writes `greeting.txt`, then calls `tome worker done`.
4. The orchestrator checks the file and finishes the run.

## 4. Watch it

Each agent runs in its own tmux session. In another terminal:

```sh
tmux ls
tmux attach -t <session>
```

You can type into an agent's session at any time, for example to answer a question or steer it. Detach with `Ctrl-b d`.

To see the run's state:

```sh
tome runs list
tome runs show <id>
tome runs logs <id>
```

Press `Ctrl-C` in the terminal where `tome run` is attached to cancel the run. Add `--detach` to start a run and get its id back right away instead.

## 5. Look at the result

When the run ends, its status is `succeeded` or `failed`, with a summary. `greeting.txt` is in your project directory.

## Next

- Learn the pieces: [Workflows, runs and steps](concepts/workflows.md).
- Write your own: [Write a workflow](guides/write-a-workflow.md).
