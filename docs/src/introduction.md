# Introduction

tome runs repeatable agentic workflows. You describe a process once, in a Markdown file in plain English, and say what starts it. Every time it fires, tome starts agents in tmux or cmux, where you can watch them work and step in.

```markdown title=.tome/workflows/plan-feature.md
{{#include ../examples/intro.md}}
```

Add a `features/<name>/feature.md` to your project, and an agent investigates the codebase and writes the plan. You don't run a command.

## Quick start

Install tome and start its daemon:

```sh title=Terminal
curl -fsSL https://raw.githubusercontent.com/banyango/tome/main/install.sh | sh
tome daemon start
```

Then follow the [quickstart](quickstart.md) to add a workflow and watch it fire.

## Features

- **Triggered.** Runs start when files change, on a cron schedule, when a message arrives, or when another workflow finishes.
- **Repeatable.** A workflow is one file. Commit it, copy it to the next project, change a line.
- **Parallel.** Fan work out to many agents, each on its own git worktree, then merge the results.
- **Visible.** Every agent runs in a terminal session you can attach to and type into.
- **Any agent.** Claude Code is built in. Any agent CLI can be added as a harness.
- **Recorded.** Each run's steps, history and logs are kept, and `tome runs` and `tome query` show them.

## When to use tome

tome fits processes you run again and again, where an agent's judgement is useful but the shape of the work stays the same:

- turning a feature spec into tasks, then into branches
- fixing failing tests as you work
- reviewing a branch with one agent per file
- scheduled checks, such as stale branches every morning
- chains, where one workflow's result starts the next

It's not the right tool for a one-off question, where you'd talk to the agent directly. Nor is it for a fully scripted pipeline with no decisions in it: use `make` or CI for that.

## Where it runs

tome runs on your machine, on macOS and Linux. Agents open in tmux or cmux, and run state lives in `~/.tome`. Nothing is hosted.

## Learn more

- [Recipes](recipes.md) has workflows you can copy for common jobs.
- The [guides](guides/write-a-workflow.md) cover every option, and [How it works](concepts/workflows.md) explains the pieces.
- Run `tome --help`, or `tome <command> --help`, for every command and flag.
- For agents, the docs are also published as plain text: [`llms.txt`](https://banyango.github.io/tome/llms.txt) is an index, and [`llms-full.txt`](https://banyango.github.io/tome/llms-full.txt) is every page in one file.
