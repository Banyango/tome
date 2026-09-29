# Install

## One command

```sh
curl -fsSL https://raw.githubusercontent.com/banyango/tome/main/install.sh | sh
```

The script:

- detects your OS and CPU (macOS or Linux, on arm64 or x86_64),
- downloads the matching build from the latest [GitHub release](https://github.com/banyango/tome/releases),
- checks its SHA-256 checksum, and installs nothing if it doesn't match,
- puts `tome` in `~/.local/bin`. It needs no `sudo`.

If `~/.local/bin` isn't on your `PATH`, the script says so and shows the line to add.

| Variable | Effect |
| --- | --- |
| `TOME_INSTALL_DIR` | Install here instead of `~/.local/bin`. |
| `TOME_VERSION` | Install a specific release, like `v0.1.0`, instead of the latest. |

```sh
curl -fsSL https://raw.githubusercontent.com/banyango/tome/main/install.sh | TOME_VERSION=v0.1.0 sh
```

To upgrade, run the script again. It overwrites the old binary.

Release builds are self-contained: DuckDB is compiled in, so there is nothing else to install.

## Build from source

You need a Rust toolchain.

```sh
git clone https://github.com/banyango/tome
cd tome
cargo build --release --features bundled
```

The binary is `target/release/tome`. `--features bundled` compiles DuckDB into it, which takes a few minutes. Without the feature, the build links a prebuilt DuckDB library that sits next to the binary.

## Check it works

```sh
tome --version
tome --help
```

## Prerequisites

To run a workflow you need two more things:

- **A terminal multiplexer**: [tmux](https://github.com/tmux/tmux), or cmux if you use it. tome starts each agent in its own multiplexer session so you can watch it.
- **An agent harness**: a command-line coding agent. [Claude Code](https://claude.com/claude-code) works out of the box. Other agents can be added in config; see [Sessions and backends](guides/sessions.md).

`git` is also needed for workflows that give workers their own worktrees.

Next: [Quickstart](quickstart.md).
