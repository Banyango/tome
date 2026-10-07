<h1 align="center">tome</h1>

<p align="center">
  Software Factory primitives for your workflows, written in Markdown.
  <br />
  Go from one agent working on your project to an army of them, coordinating.
</p>

<p align="center">
  <a href="#about">About</a> ·
  <a href="#install">Install</a> ·
  <a href="#a-workflow-in-30-seconds">Example</a> ·
  <a href="https://banyango.github.io/tome">Documentation</a> ·
  <a href="#building-from-source">Building</a>
</p>

<p align="center">
  <a href="https://github.com/banyango/tome/releases"><img alt="Release" src="https://img.shields.io/github/v/release/banyango/tome" /></a>
  <a href="LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-blue" /></a>
</p>

## About

tome is a Rust CLI and daemon that runs agent workflows for you. A workflow is
a Markdown file written in plain English, plus a few lines of frontmatter that
say what starts it. The daemon watches for that trigger, starts an
orchestrator agent, and the orchestrator spawns worker agents to do the work.

- **Triggers.** Start runs when files change, on a cron schedule, when a
  message arrives, or when another workflow finishes.
- **Workers and worktrees.** Fan work out to as many agents as you like, each
  on its own git branch.
- **Queues and an event bus.** Chain workflows together, or have workers pull
  jobs off a queue.
- **Any agent.** tome is harness agnostic. Run an orchestrator in Claude Code
  and a worker in something else.
- **Watch and steer.** Every agent runs in a tmux, cmux or herdr pane. Attach
  to it, answer a question, or take over.
- **Remote nodes.** Send events and run workflows on other machines over ssh.
- **Resumable.** State lives in DuckDB, so a run's steps, history and logs are
  always there when something fails.

You don't have to write workflows by hand. Install the
[tome plugin](https://banyango.github.io/tome/agents/skill.html) and your
coding agent writes them for you.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/banyango/tome/main/install.sh | sh
tome daemon start
```

This installs a self-contained binary for macOS or Linux (arm64 or x86_64) in
`~/.local/bin`, with no `sudo`. You also need a terminal multiplexer
([tmux](https://github.com/tmux/tmux), cmux or herdr) and an agent harness
such as [Claude Code](https://claude.com/claude-code). See the
[install guide](https://banyango.github.io/tome/install.html) for options.

## A workflow in 30 seconds

Save this as `.tome/workflows/fix-tests.md` in your project:

```markdown
---
name: fix-tests
description: Whenever Rust source changes, run the tests and fix any failures on a branch.
mode: orchestrated
triggers:
  - file: "src/**/*.rs"
    debounce: 10s
    while_running: queue
concurrency: 1
---
These files just changed:

{{trigger.paths}}

## Test
Spawn a command worker named `tests` that runs `cargo test`, and wait for it.
If it succeeded, finish the run successfully.

## Fix
Spawn an agent worker named `fixer` with its own worktree. Tell it to make the
failing tests pass without changing what the tests check, and to commit its
work. Wait for it. If it failed, fail the run and say why.
```

From now on, every time you save a Rust file the tests run. If they fail, an
agent fixes them on its own branch and leaves it for you to review. Nothing is
merged without you.

```sh
tome validate fix-tests   # check the workflow
tome runs list            # see what's running
tome runs show <id>       # steps, history and logs
```

More in the [quickstart](https://banyango.github.io/tome/quickstart.html) and
the [recipes](https://banyango.github.io/tome/recipes.html), from reviewing a
branch in parallel to a whole software factory.

## Documentation

The docs live at **<https://banyango.github.io/tome>**. Run `tome --help`, or
`tome <command> --help`, for every command and flag. For agents, the docs are
also published as plain text: [`llms.txt`](https://banyango.github.io/tome/llms.txt)
is an index and [`llms-full.txt`](https://banyango.github.io/tome/llms-full.txt)
is every page in one file.

The docs are an mdBook in [`docs/`](docs). `make docs` serves them locally
(needs mdbook 0.5).

## Building from source

You need a Rust toolchain.

```sh
git clone https://github.com/banyango/tome
cd tome
make build     # debug build
make test      # run the tests
make release   # release binary with bundled DuckDB (slow)
```

The release binary is `target/release/tome`. See [`AGENTS.md`](AGENTS.md) for
the project layout, conventions and release process.

## License

[MIT](LICENSE)
