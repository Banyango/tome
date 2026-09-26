# Foundation — Task Plan

Source feature: [feature.md](./feature.md)

## Tasks

### 001-1. Daemon + CLI transport

**Blocked by:** none

Build the tome CLI and daemon in Rust. `tome daemon start/stop/status` controls the single per-user daemon, and the CLI talks to it with newline-delimited JSON-RPC over `~/.tome/tome.sock`. When the daemon isn't running, CLI commands fail with a hint to run `tome daemon start` or `tome daemon install` and don't start it. This task also sets the output contract every later command follows: human-readable by default, stable machine output with `--json` or `TOME_OUTPUT=json`, and meaningful exit codes. Covers use case 7.

### 001-2. Workflow loading & validation

**Blocked by:** 001-1

Load workflows written as markdown with yaml frontmatter (`name`, `description`, `triggers`, `params`, `defaults`, `concurrency`, `on_conflict`) and a natural-English body. Look them up in `~/.tome/workflows` and `.tome/workflows`, where a project workflow wins over a global one with the same `name`. Resolve `--param k=v` overrides against the declared params and defaults, and substitute `{{params.x}}` / `{{run.id}}` into the body (plain substitution, no logic). `tome validate` checks yaml syntax, unknown frontmatter keys and undefined placeholders, reports errors with line numbers, and exits with code `2` on failure. The same checks are available to run before any run starts. Covers use cases 1–4.

### 001-3. Run state store

**Blocked by:** 001-1, 001-2

The daemon is the only process that opens `~/.tome/tome.duckdb`, and it migrates the schema automatically on startup. The store records each run with its project path and the resolved workflow snapshot taken when the run started, so editing the file mid-run affects only new runs. It also records the step status and history the orchestrator reports, and indexes raw log files under `~/.tome/runs/<run-id>/<step>.log` (path, size, tail excerpt). Covers use case 5 and provides the storage 002 builds on.

### 001-4. Run inspection

**Blocked by:** 001-3

`tome runs list`, `tome runs show` and `tome runs logs` show current and past runs, their step history and their logs. `tome query "<sql>"` runs read-only SQL through the daemon. All of these support human and `--json` output. Covers use case 8.

### 001-5. OS service install

**Blocked by:** 001-1

`tome daemon install` registers the daemon as a launchd agent on macOS or a systemd `--user` unit on Linux, so it's always available. Covers use case 6.

### 001-6. Crash recovery

**Blocked by:** 001-3

When the daemon starts, any run still marked in progress is marked failed with reason `daemon_restart`, and its worktrees are kept. For each of those runs, recovery calls a "kill this run's sessions" hook and a "notify the user" hook. Both hooks do nothing until the backend/session and notification features fill them in. Covers use case 9.

### 001-7. Garbage collection

**Blocked by:** 001-3

`tome gc --older-than <age>` removes finished runs older than the given age, along with their log files and worktrees. Nothing is ever deleted automatically. Covers use case 10.
