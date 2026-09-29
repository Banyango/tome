# Docs site

## Description

A public docs site at `banyango.github.io/tome`, built with mdBook. It teaches people how to install tome, run it, and use it, and agents can read it too. CI builds the site from `main` and deploys it to GitHub Pages on every push. Next to the HTML, the site publishes `llms.txt` (an index) and `llms-full.txt` (every page joined into one file) so agents can fetch plain text.

The docs cover only features that exist in the code today. They include cmux and tmux, but not herdr or the workflow authoring skill. Each of those gets documented by its own feature when it lands.

This feature also adds the distribution pieces the docs rely on:

- Create the public GitHub repo `banyango/tome`, with an MIT `LICENSE` and a short `README` that links to the site.
- Add a release workflow. It runs when a `v*` tag is pushed, builds `--features bundled` binaries for macOS and Linux on both arm64 and x86_64, and attaches them with checksums to a GitHub Release.
- Add an `install.sh` script that users run with `curl | sh`. It detects the OS and architecture, downloads the right tarball, checks its checksum, and installs `tome`.

Content in v1:

- **Install and quickstart:** installing tome, the prerequisites (a multiplexer and an agent harness), starting the [[daemon]], and a first run from start to finish with tmux and Claude Code.
- **Concepts:** [[workflow]], [[run]], [[step]], [[daemon]], [[backend]], the primitives ([[worker]], [[worktree]], [[group]], [[queue]]), [[trigger]]s, and the message bus.
- **Guides:**
  - Write a workflow. This guide explains every frontmatter field with examples, so there is no separate format reference page.
  - Workers, worktrees and fan-out.
  - Triggers and the message bus.
  - Sessions and backends.
  - Operating tome.

There is no CLI reference in v1. The docs point readers to `tome --help` for command and flag details.

## Use Cases

1. As a new user, I want to install tome with one command, so that I can start without a Rust toolchain.
2. As a new user, I want a quickstart that takes me from install to a finished first run using tmux and Claude Code, so that I can see tome work end to end.
3. As a user, I want concept pages for workflow, run, step, daemon, backend, primitives, triggers and the message bus, so that I understand how the pieces fit together.
4. As a workflow author, I want a guide that explains every frontmatter field with examples, so that I can write workflows without reading the source.
5. As a user, I want guides for:
   - workers, worktrees, fan-out and fan-in
   - triggers and topics
   - sessions and backends (harness config, cmux or tmux, layout and placement)
   - operating tome (daemon lifecycle, `tome runs`, `tome query`, `tome gc`, removing workflows, troubleshooting)

   so that I can use each capability.
6. As an agent, I want to fetch `llms.txt` and `llms-full.txt`, so that I can load the docs as plain text.
7. As a maintainer, I want to push a `v*` tag and get binaries for all four targets attached to a GitHub Release, so that the install script has something to download.
8. As a maintainer, I want every push to `main` to rebuild and redeploy the site, so that the docs stay current.
9. As a maintainer, I want CI to run `tome validate` on every example workflow under `docs/`, so that examples that no longer work fail the build.

## Constraints

- Build the site with mdBook and host it on GitHub Pages from the public repo `banyango/tome`.
- Publish one unversioned site built from `main`. Versioned docs are a follow-up.
- Document only what is built today. Leave out planned features such as herdr and the authoring skill.
- Keep example workflows as files under `docs/` so that CI can run `tome validate` on them.
- Build release binaries with `--features bundled` so they are self-contained. Ship four targets: aarch64 and x86_64 `apple-darwin`, and aarch64 and x86_64 `unknown-linux-gnu`.
- `install.sh`:
  - installs to `~/.local/bin` by default, and `TOME_INSTALL_DIR` overrides the location
  - `TOME_VERSION` pins a release
  - needs no sudo
- License the repo under MIT.

## Edge Cases

- **Unsupported OS or architecture:** `install.sh` exits with a clear message and points to building from source.
- **Checksum mismatch:** `install.sh` stops and installs nothing.
- **Install directory isn't on `PATH`:** `install.sh` installs anyway and prints a hint about adding the directory to `PATH`.
- **tome is already installed:** `install.sh` overwrites it, which is how upgrades work.
- **No release exists yet, or the download fails:** `install.sh` exits with an error that names the URL it tried.
- **An example workflow fails `tome validate`:** the docs CI fails and nothing deploys.
- **The Pages deploy fails:** the site that is already live stays up.
- **A release build fails on one target:** no partial release is published.

## Related Entities

- [[workflow]]
- [[run]]
- [[step]]
- [[daemon]]
- [[backend]]
- [[harness-adapter]]
- [[session]]
- [[worker]]
- [[worktree]]
- [[group]]
- [[queue]]
- [[trigger]]
