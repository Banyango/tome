# Introduction

Tome orchestrates agent harnesses, such as Claude Code, so they work together. You describe a workflow once, in plain English, and tome runs it the same way every time it fires. You don't have to write it yourself: with the [tome plugin](agents/skill.md), your coding agent writes it for you.

Its primitives are triggers, workers, worktrees, queues and an event bus. Put them together and you can build anything from a single test fixer to a whole software factory.

Every agent runs in a tmux or cmux session, so you can watch what it's doing and step in.

## Quick start

Install tome and start its daemon:

```sh title=Terminal
curl -fsSL https://raw.githubusercontent.com/banyango/tome/main/install.sh | sh
tome daemon start
```

Then follow the [quickstart](quickstart.md) to add a workflow and watch it fire, and [install the tome plugin](agents/skill.md) so your agent can write the next one.

## Features

- **Triggered.** Runs start when files change, on a cron schedule, when a message arrives, or when another workflow finishes.
- **Repeatable.** A workflow is one file. Commit it, copy it to the next project, change a line.
- **Parallel.** Fan work out to many agents, each on its own git worktree, then merge the results.
- **Visible.** Every agent runs in a terminal session you can attach to and type into.
- **Any agent.** Claude Code is built in. Any agent CLI can be added as a harness.
- **Recorded.** Each run's steps, history and logs are kept, and `tome runs` and `tome query` show them.

## When to use tome

- turning a feature spec into tasks, then into branches
- fixing failing tests as you work
- reviewing a branch with one agent per file
- scheduled checks, such as stale branches every morning
- chains, where one workflow's result starts the next


## Where it runs

tome runs on your machine, on macOS and Linux. Agents open in tmux or cmux, and run state lives in `~/.tome`.

## Learn more

- [Create workflows with an agent](guides/create-with-an-agent.md) shows how to have your agent write, run and fix workflows.
- [Recipes](recipes.md) has workflows for common jobs. Ask your agent to adapt one to your project.
- The [guides](guides/write-a-workflow.md) cover every option, and [How it works](concepts/workflows.md) explains the pieces.
- Run `tome --help`, or `tome <command> --help`, for every command and flag.
- For agents, the docs are also published as plain text: [`llms.txt`](https://banyango.github.io/tome/llms.txt) is an index, and [`llms-full.txt`](https://banyango.github.io/tome/llms-full.txt) is every page in one file.
