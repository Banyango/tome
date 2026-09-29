# tome

tome runs agentic workflows. You describe a process as a Markdown file: what the steps are, in plain English. tome starts an orchestrator agent that reads the file and carries it out, spawning worker agents in your terminal multiplexer as it goes. You can watch every agent work, type into any of them, and repeat the process later or in another project.

## Why

Repeating an agent workflow usually means a pile of prompts and notes that you re-paste by hand. Tome keeps the workflow in one file that you can version, copy to another project and change a little. Because each agent runs in a visible terminal session, you can see what is happening and step in when something needs you.

## What is in the box

- **Workflows**: Markdown files with a little YAML frontmatter. The body is plain English. tome never parses it; the orchestrator agent reads it.
- **A daemon** that starts runs, watches for triggers and keeps track of every run.
- **Workers, worktrees, groups and queues**: the pieces an orchestrator uses to split work across agents and collect the results.
- **Triggers**: start a workflow when a file changes, on a schedule, or when a message arrives.
- **Two multiplexers**: cmux and tmux. Any agent CLI can be a harness; Claude Code is built in.

tome is written in Rust and keeps run history in an embedded DuckDB database under `~/.tome`.

## Where to go next

1. [Install tome](install.md) and [do a first run](quickstart.md).
2. Read the [concepts](concepts/workflows.md) to see how the pieces fit together.
3. Follow the [guides](guides/write-a-workflow.md) when you want to build something.

There is no CLI reference in these docs. Run `tome --help`, or `tome <command> --help`, for every command and flag.

## For agents

The docs are also published as plain text: [`llms.txt`](https://banyango.github.io/tome/llms.txt) is an index, and [`llms-full.txt`](https://banyango.github.io/tome/llms-full.txt) is every page in one file.

## What these docs cover

Only what tome does today. Planned pieces, such as the herdr backend and a skill for writing workflows, will be documented when they land.
