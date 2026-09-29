# Install the tome skill

The tome skill teaches a coding agent how to write, validate, run and debug tome workflows. It is one file, `SKILL.md`, in the [Agent Skills](https://agentskills.io) format. The agent loads it when a task mentions tome or `.tome/workflows`.

The skill doesn't install tome itself. Do that first: [Install](../install.md). [Install tome (for agents)](install.md) installs tome and the skill together.

## Claude Code

For every project, install it in your home directory:

```sh
mkdir -p ~/.claude/skills/tome
curl -fsSL https://raw.githubusercontent.com/banyango/tome/main/skills/tome/SKILL.md \
  -o ~/.claude/skills/tome/SKILL.md
```

For one project, install it in the repository and commit it, so everyone working there gets it:

```sh
mkdir -p .claude/skills/tome
curl -fsSL https://raw.githubusercontent.com/banyango/tome/main/skills/tome/SKILL.md \
  -o .claude/skills/tome/SKILL.md
```

Start a new Claude Code session to load it. To check it's there, ask "what skills do you have?", or ask for something like "write a tome workflow that runs the tests every night".

To upgrade, run the same `curl` again.

## Other agents

Any agent that reads Agent Skills can use the same file. Put `SKILL.md` in a `tome/` folder in that agent's skills directory; its docs say where that is.

An agent that doesn't support skills can read the same material as plain text. Point it at [`llms-full.txt`](https://banyango.github.io/tome/llms-full.txt), or paste the file below into its instructions.

## Asking an agent to install it

Paste this into your agent:

```text
Install the tome skill for Claude Code: download
https://raw.githubusercontent.com/banyango/tome/main/skills/tome/SKILL.md
to ~/.claude/skills/tome/SKILL.md, creating the folder if needed. Then tell me
to start a new session.
```

## What's in it

The skill covers checking the install, the daemon, workflow files and their frontmatter, triggers and the message bus, running and inspecting runs, and fixing common failures. The full file:

````markdown title=skills/tome/SKILL.md
{{#include ../../../skills/tome/SKILL.md}}
````
