<p align="center">
  <img src="assets/tome-logo.png" alt="Tome pixel art logo with a glowing rune" width="200">
</p>

<h1 align="center">Tome</h1>

<p align="center">
  <em>All the primitives you need to build your software factories</em>
</p>

<p align="center">
  <img src="https://img.shields.io/github/stars/Banyango/tome?style=flat-square&color=111111&label=stars" alt="Stars">
  <img src="https://img.shields.io/github/v/release/Banyango/tome?style=flat-square&color=111111&label=release" alt="Release">
  <img src="https://img.shields.io/badge/license-MIT-111111?style=flat-square" alt="MIT license">
</p>


Connect your harnesses together. Build an army of agents. Tome gives you the tools to build it.


## Features

- **Triggers** start runs when files change, on a cron schedule, when a message arrives, or when another workflow finishes.
- **Workers** fan work out to as many agents as you like, each on its own git branch.
- **Queues** and an event bus chain workflows together, or let workers pull jobs off a queue.
- **Harness agnostic** Run an orchestrator in Claude Code, or Codex, or whatever.
- Every agent runs in a **tmux**, **cmux** or **herdr** panes, so you can attach, answer a question, or take over.
- **Remote nodes** send events and run workflows on other machines over ssh.
- State lives in DuckDB, so a run's steps, history and logs are always there when something fails.

You don't have to write workflows by hand. Install the [tome plugin](#installation) and your coding agent writes them for you.


## How It Works

You can have an agent write the workflow or write it yourself.

### Have an agent write the workflow

1. Install the plugin (see [Installation](#installation)).
2. Ask your agent for what you want, for example "write a tome workflow that runs the tests every night". The skill is also available as `/tome:tome`.
3. Review the workflow it writes in `.tome/workflows/`, then run `tome validate <name>`.

### Write the workflow yourself

1. Create a Markdown file in `.tome/workflows/`. The frontmatter says what starts it. The body is plain English, split into steps under `##` headings.
2. Run `tome validate <name>` to check it. It points to the line of any mistake.
3. Run `tome run <name>`, or let the trigger start it.
4. Watch the agents in their panes, or follow along with `tome runs show <id>`.

### Benefits

1. A workflow is one file you can read, review in a PR and copy to another project.
2. Each run records its steps, history and logs, so a failed run tells you where it stopped.
3. A failed or cancelled run can be resumed with `tome run resume <id>`, and the agent is told what the earlier attempt did.
4. Workers get their own git worktrees, so nothing is merged without you.

### Where this approach works best

1. You repeat the same agent workflow and want it written down once.
2. You want several agents working on a task in parallel, where you can see them.
3. You want something to start work for you, on a file change, a schedule or a message.
4. You use more than one agent harness and want one way to coordinate them.


## Commands

| Command                    | What it does                                        |
|----------------------------|-----------------------------------------------------|
| `tome start`        | Start the daemon  |
| `tome validate <name>`     | Check a workflow                                    |
| `tome run <name>`          | Start a run                                         |
| `tome run resume <id>`     | Resume a failed or cancelled run                    |
| `tome runs list`           | See what's running                                  |
| `tome runs show <id>`      | Steps, custom status, history and logs for a run    |
| `tome runs logs <id>`      | Print a run's logs                                  |
| `tome queue`, `tome publish`, `tome events` | Queues and the message bus         |

Run `tome --help`, or `tome <command> --help`, for every command and flag.

## Workflow

### 1. Write a workflow

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

The `name` is what you pass to `tome run`. The `triggers` say what starts a run. The body is read by the run's agent, not parsed, and its `##` headings are the steps reported in `tome runs show`. `mode` is `single` (the default, one agent does everything) or `orchestrated` (an orchestrator hands work to workers).

### 2. Check it

```sh
tome validate fix-tests
```

tome rejects unknown frontmatter keys and `{{placeholders}}` that name no param, and points to the line.

### 3. Run and inspect

```sh
tome runs list            # see what's running
tome runs show <id>       # steps, history and logs
```

From now on, every time you save a Rust file the tests run. If they fail, an agent fixes them on its own branch and leaves it for you to review. Nothing is merged without you.

More in the [quickstart](https://banyango.github.io/tome/quickstart.html) and the [recipes](https://banyango.github.io/tome/recipes.html), from reviewing a branch in parallel to a whole software factory.

## Installation

### tome

```sh
curl -fsSL https://raw.githubusercontent.com/banyango/tome/main/install.sh | sh
tome daemon start
```

This installs a self-contained binary for macOS or Linux (arm64 or x86_64) in `~/.local/bin`, with no `sudo`. You also need a terminal multiplexer ([tmux](https://github.com/tmux/tmux), cmux or herdr) and an agent harness such as [Claude Code](https://claude.com/claude-code). See the [install guide](https://banyango.github.io/tome/install.html) for options.

### Claude Code plugin

```sh
/plugin marketplace add banyango/tome
/plugin install tome@tome
```

The skill is available as `/tome:tome`. Run `/reload-plugins`, or start a new session, to load it.

### Codex

From the repository root:

```sh
codex plugin marketplace add .
```

Then enable it in `.codex/config.toml`:

```toml
[plugins."tome@tome"]
enabled = true
```

### OpenCode

Start OpenCode from this repository; its `opencode.json` loads the skill. To install it globally, copy `plugin/skills/tome` to `~/.config/opencode/skills/tome`.

### Other agents

Any agent that reads Agent Skills can use the skill. Download `SKILL.md` into a `tome/` folder in that agent's skills directory:

```sh
curl -fsSL https://raw.githubusercontent.com/banyango/tome/main/plugin/skills/tome/SKILL.md \
  -o <skills-dir>/tome/SKILL.md
```

For agents that don't support skills, the docs are also published as plain text: [`llms.txt`](https://banyango.github.io/tome/llms.txt) is an index and [`llms-full.txt`](https://banyango.github.io/tome/llms-full.txt) is every page in one file.

## Documentation

The docs live at **<https://banyango.github.io/tome>**. They are an mdBook in [`docs/`](docs); `make docs` serves them locally (needs mdbook 0.5).

## Workflow File Reference

```markdown
---
name: my-workflow                  # required — what you pass to `tome run`
description: One line, shown in listings.
mode: single | orchestrated        # default: single
triggers:                          # what starts a run
  - file: "src/**/*.rs"
concurrency: 1                     # max simultaneous runs
on_conflict: queue | reject        # default: queue
params:                            # inputs, filled in with --param key=value
  base: {default: main}
defaults:
  harness: claude
  model: opus
---

## Step name

What the step does, what counts as done, and what to do if it fails.
Set status to InProgress.
```

See [Write a workflow](https://banyango.github.io/tome/guides/write-a-workflow.html) for every field, including [custom statuses](https://banyango.github.io/tome/guides/write-a-workflow.html#custom-statuses).

## Building from source

You need a Rust toolchain.

```sh
git clone https://github.com/banyango/tome
cd tome
make build     # debug build
make test      # run the tests
make release   # release binary with bundled DuckDB (slow)
```

The release binary is `target/release/tome`. See [`AGENTS.md`](AGENTS.md) for the project layout, conventions and release process.

## License

[MIT](LICENSE).
