# Install tome (for agents)

This page is for a coding agent that has been asked to install tome. Do the steps in order, and check each one before moving on. If a check fails, stop and tell the user what failed and what you saw.

The plain-text version of this page is at <https://banyango.github.io/tome/md/agents/install.md>.

## 1. Check whether tome is already installed

```sh
tome --version
```

If this prints a version, tome is installed. Skip to step 4, unless the user asked for an upgrade.

## 2. Check the platform

```sh
uname -sm
```

tome supports macOS (`Darwin`) and Linux, on `arm64`/`aarch64` or `x86_64`. On anything else, stop and tell the user. Building from source is their call; see [Install](../install.md#build-from-source).

## 3. Run the installer

```sh
curl -fsSL https://raw.githubusercontent.com/banyango/tome/main/install.sh | sh
```

It needs no `sudo`. It downloads the latest release, checks its SHA-256 checksum and puts `tome` in `~/.local/bin`. If the checksum doesn't match, it installs nothing and exits with an error. Report that to the user; don't retry around it.

Only if the user asks for them:

- `TOME_VERSION=v0.1.0`: a specific release instead of the latest.
- `TOME_INSTALL_DIR=<dir>`: another directory instead of `~/.local/bin`.

Set them on `sh`, not `curl`:

```sh
curl -fsSL https://raw.githubusercontent.com/banyango/tome/main/install.sh | TOME_VERSION=v0.1.0 sh
```

## 4. Check it's on the PATH

```sh
command -v tome && tome --version
```

If `command -v` prints nothing, the install directory isn't on `PATH`. The installer printed the line to add. Tell the user; don't edit their shell profile unless they ask. To finish this session, call the binary by its full path, `~/.local/bin/tome`.

## 5. Check the prerequisites

```sh
command -v tmux || echo "no tmux"
command -v claude || echo "no claude"
command -v git || echo "no git"
```

- tome needs a terminal multiplexer: tmux, or cmux. If the user works in cmux, tmux is optional. Ask if you can't tell.
- The default agent harness is Claude Code (`claude`). Other agents can be configured; see [Sessions and backends](../guides/sessions.md).
- `git` is needed for workflows that give workers their own worktrees.

Tell the user what's missing. Installing tmux or an agent CLI changes their system, so ask first.

## 6. Start the daemon

```sh
tome daemon start
tome daemon status
```

`status` exits with code 3 if the daemon isn't running. If the user runs cmux, the daemon has to be started from a terminal inside cmux, or sessions won't open there.

To have it start at login, `tome daemon install` registers a launchd (macOS) or systemd `--user` (Linux) service. Only do that if the user asks.

## 7. Install the tome skill

The [tome skill](skill.md) teaches agents to write, run and debug tome workflows. Install it for every project:

```sh
mkdir -p ~/.claude/skills/tome
curl -fsSL https://raw.githubusercontent.com/banyango/tome/main/skills/tome/SKILL.md \
  -o ~/.claude/skills/tome/SKILL.md
```

If the user asked for it in this project only, use `.claude/skills/tome/` in the repository instead of `~/.claude/skills/tome/`. If you aren't Claude Code but you read Agent Skills, put `tome/SKILL.md` in your own skills directory. If you don't read skills, skip this step and say so in your report.

Check it:

```sh
head -3 ~/.claude/skills/tome/SKILL.md
```

The file starts with `---` and then `name: tome`. The skill loads in the next session, not this one.

## 8. Report back

Tell the user:

- the version from `tome --version`, and where the binary is,
- anything missing from step 5,
- whether the daemon is running,
- where the skill was installed, and that it loads in a new session.

Next, they can follow the [Quickstart](../quickstart.md).
