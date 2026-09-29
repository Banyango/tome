# Docs site — Task Plan

Source feature: [feature.md](./feature.md)

## Tasks

### 013-1. Public repo, license and README

**Blocked by:** none

Create the public `banyango/tome` GitHub repo with an MIT `LICENSE` and a short `README` that links to the docs site.

### 013-2. Release workflow

**Blocked by:** 013-1

Pushing a `v*` tag builds `--features bundled` tarballs for aarch64 and x86_64 `apple-darwin` and `unknown-linux-gnu`, and attaches them with checksums to a GitHub Release. If any target fails, no partial release is published.

### 013-3. install.sh

**Blocked by:** 013-2

A `curl | sh` installer that detects OS and architecture, downloads the right tarball, verifies its checksum and installs `tome` to `~/.local/bin` (no sudo). `TOME_INSTALL_DIR` overrides the location and `TOME_VERSION` pins a release. It handles unsupported platforms (points to building from source), checksum mismatch (installs nothing), a directory missing from `PATH` (prints a hint), an existing install (overwrites, which is how upgrades work) and a failed download (error names the URL tried).

### 013-4. mdBook site, Pages deploy and llms.txt

**Blocked by:** 013-1

An mdBook skeleton with the navigation structure, and a CI workflow that builds from `main` on every push and deploys to GitHub Pages at `banyango.github.io/tome`. The build also publishes `llms.txt` (index) and `llms-full.txt` (all pages joined). CI runs `tome validate` on every example workflow under `docs/`; a failure stops the deploy and leaves the live site untouched.

### 013-5. Install and quickstart pages

**Blocked by:** 013-3, 013-4

Pages covering installing tome, the prerequisites (a multiplexer and an agent harness), starting the daemon, and a first run from start to finish with tmux and Claude Code, using an example workflow kept under `docs/`.

### 013-6. Concept pages

**Blocked by:** 013-4

Concept pages for workflow, run, step, daemon, backend, the primitives (worker, worktree, group, queue), triggers and the message bus, covering only features that exist today.

### 013-7. Guide: writing a workflow

**Blocked by:** 013-4

A guide that explains every frontmatter field with examples, kept as validated workflow files under `docs/`, so there is no separate format reference page. It points readers to `tome --help` for command and flag details.

### 013-8. Guides: workers/worktrees/fan-out and triggers/message bus

**Blocked by:** 013-7

A guide on workers, worktrees, fan-out and fan-in, and a guide on triggers and topics, each with validated example workflows.

### 013-9. Guides: sessions/backends and operating tome

**Blocked by:** 013-7

A guide on sessions and backends (harness config, cmux or tmux, layout and placement), and a guide on operating tome (daemon lifecycle, `tome runs`, `tome query`, `tome gc`, removing workflows, troubleshooting).
