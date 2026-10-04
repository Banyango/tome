# Introduction

Tome orchestrates agent harnesses. Go from having a single harness working on your project to an army of harnesses all coordinating together!

You don't have to write it yourself: with the [tome plugin](agents/skill.md), your coding agent writes it for you.

Tome gives you powerful primitives triggers, workers, worktrees, queues, remote nodes (ssh) and an event bus. Put them together and you can build anything from a single test fixer to a whole software factory.

Every agent runs in a tmux or cmux session, so you can watch what it's doing and step in.

## Quick start

Install tome and start its daemon:

```sh title=Terminal
curl -fsSL https://raw.githubusercontent.com/banyango/tome/main/install.sh | sh
tome daemon start
```

Then follow the [quickstart](quickstart.md) to add a workflow and watch it fire, and [install the tome plugin](agents/skill.md) so your agent can write the next one.

## Features

- **Triggers.** Runs can start when files change, on a cron schedule, when a message arrives, or when another workflow finishes.
- **Workflows** The base object of tome. Create workflows in a prompt and tome will track each step. Figure out where you failed, and where you could restart a workflow.
- **Queues.** Create a queue and push events to it, have workers pull off the queue as they work.
- **Event Bus.** Use an event bus to push events for any number of workers to listen to.
- **Any agent.** Tome is totally agent agnostic, run an orchestrator in Claude and a worker in Pi. Anything you want.
- **Remote Nodes.** Use ssh to send events and run workflows on other tome instances. 


## Learn more

- [Create workflows with an agent](guides/create-with-an-agent.md) shows how to have your agent write, run and fix workflows.
- [Remote nodes](guides/nodes.md) shows how to add a machine, run workflows on it and forward events to it.
- [Recipes](recipes.md) has workflows for common jobs. Ask your agent to adapt one to your project.
- The [guides](guides/write-a-workflow.md) cover every option, and [How it works](concepts/workflows.md) explains the pieces.
- Run `tome --help`, or `tome <command> --help`, for every command and flag.
- For agents, the docs are also published as plain text: [`llms.txt`](https://banyango.github.io/tome/llms.txt) is an index, and [`llms-full.txt`](https://banyango.github.io/tome/llms-full.txt) is every page in one file.
