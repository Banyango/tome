# Install the tome plugin

The tome plugin for Claude Code teaches your agent how to write, validate, run and debug tome workflows. It holds one skill, `tome`, in the [Agent Skills](https://agentskills.io) format. The agent loads it when a task mentions tome or `.tome/workflows`, or you can call it with `/tome:tome`.

The plugin doesn't install tome itself. Do that first: [Install](../install.md). [Install tome (for agents)](install.md) installs tome and the plugin together.

## Claude Code

In a Claude Code session, add the tome marketplace and install the plugin:

```text
/plugin marketplace add banyango/tome
/plugin install tome@tome
```

Or from your shell:

```sh
claude plugin marketplace add banyango/tome
claude plugin install tome@tome
```

Run `/reload-plugins`, or start a new session, to load it. To check it's there, run `claude plugin details tome`, or ask for something like "write a tome workflow that runs the tests every night".

To install it for everyone working in one repository, run this there once and commit the `.claude/settings.json` it writes:

```sh
claude plugin marketplace add banyango/tome --scope project
claude plugin install tome@tome --scope project
```

## Updates

The plugin follows the tome repository, so each change to the skill is a new version. To update:

```text
/plugin marketplace update tome
```

or `claude plugin update tome@tome` from your shell, then start a new session. To update on its own, turn on auto-update: run `/plugin`, go to **Marketplaces**, select `tome`, and select **Enable auto-update**.

Once it's installed, see [Create workflows with an agent](../guides/create-with-an-agent.md) for how to use it.

## Other agents

Any agent that reads Agent Skills can use the skill without the plugin. Download `SKILL.md` into a `tome/` folder in that agent's skills directory; its docs say where that is:

```sh
curl -fsSL https://raw.githubusercontent.com/banyango/tome/main/plugin/skills/tome/SKILL.md \
  -o <skills-dir>/tome/SKILL.md
```

Run the same `curl` again to upgrade.

## Codex

Codex can use the plugin from the repository's marketplace. Add it from the repository root:

```sh
codex plugin marketplace add .
```

Enable the plugin for this project in `.codex/config.toml`:

```toml
[plugins."tome@tome"]
enabled = true
```

Restart Codex to load it. The repository marketplace is available to the project, and the project config enables tome.

## OpenCode

OpenCode reads the existing tome skill through the repository's `opencode.json`; start OpenCode from this repository and load `tome` with its `skill` tool. To install it globally instead, copy `plugin/skills/tome` to `~/.config/opencode/skills/tome`.

An agent that doesn't support skills can read the same material as plain text. Point it at [`llms-full.txt`](https://banyango.github.io/tome/llms-full.txt), or paste the file below into its instructions.

## Asking an agent to install it

Paste this into Claude Code:

```text
Install the tome plugin: run `claude plugin marketplace add banyango/tome`
and `claude plugin install tome@tome`, then tell me to run /reload-plugins.
```

## What's in it

The skill covers checking the install, the daemon, workflow files and their frontmatter, triggers and the message bus, running and inspecting runs, and fixing common failures. The full file:

````markdown title=plugin/skills/tome/SKILL.md
{{#include ../../../plugin/skills/tome/SKILL.md}}
````
