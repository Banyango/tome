# Introduction

Tome gives you the primitives you need to build agentic workflows and meets you where you're already building.

<video src="assets/tome-launch.mp4" controls muted playsinline style="width: 100%; border-radius: 8px;"></video>


You get powerful features 
- **Triggers** - Run a workflow on a file change/ a cron job/ +more
- **Mix and match Harnesses** - Create a workflow that uses Claude and Codex, or whatever you want.
- **Workers** - Spin off any number of agents to work on something. Orchestrate them together.
- **Queues** - Push work to a queue and have workers pull it off. 
- **Event Buses** - Broadcast an event to any workflows listening.
- **Named Steps** - Know exactly where you are in the workflow.
- **Hubs** - Link together multiple projects in tome and orchestrate them together.
- **Remote Nodes** - Send work to remote machines. 
- **Remote Workers** - Do work on a docker container or a remote box through ssh.

Using these you can build anything from a single repeatable workflow to a whole agentic software factory.

```Tome works best with cmux, tmux and herdr.```

Get your agent to write a workflow: with the [tome plugin](agents/skill.md).

## Quick start

Install tome and start its daemon:

```sh title=Terminal
curl -fsSL https://raw.githubusercontent.com/banyango/tome/main/install.sh | sh

tome start
```

Then follow the [quickstart](quickstart.md) to add a workflow and watch it fire, and [install the tome plugin](agents/skill.md) so your agent can write the next one.

## Features

- **Workflows** The base object of tome. Create workflows in a prompt and tome will track each step. Figure out where you failed, and where you could restart a workflow.
- **Triggers.** Runs can start when files change, on a cron schedule, when a message arrives, or when another workflow finishes.
- **Queues.** Create a queue and push events to it, have workers pull off the queue as they work.
- **Event Bus.** Use an event bus to push events for any number of workers to listen to.
- **Any agent.** Tome is totally agent agnostic, run an orchestrator in Claude and a worker in Pi. Anything you want.
- **Remote Nodes.** Use ssh to send events and run workflows on other tome instances. 


## Learn more

- [Create workflows with an agent](guides/create-with-an-agent.md) shows how to have your agent write, run and fix workflows.
- [Recipes](recipes.md) has workflows for common jobs. Ask your agent to adapt one to your project.
- The [guides](guides/write-a-workflow.md) cover every option, and [How it works](concepts/workflows.md) explains the pieces.
- [Remote nodes](guides/nodes.md) shows how to add a machine, run workflows on it and forward events to it.
- Run `tome --help`, or `tome <command> --help`, for every command and flag.
- For agents, the docs are also published as plain text: [`llms.txt`](https://banyango.github.io/tome/llms.txt) is an index, and [`llms-full.txt`](https://banyango.github.io/tome/llms-full.txt) is every page in one file.
