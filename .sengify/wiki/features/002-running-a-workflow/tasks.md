# Running a workflow — Task Plan

Source feature: [feature.md](./feature.md)

Guiding principle: an agent must be able to verify every task end-to-end without a human. The harness adapter is a command template, so tests replace the real agent with a scripted stub that drives tome commands. Sessions use tmux because it runs headless and can be scripted.

## Tasks

### 002-1. Run control plane

**Blocked by:** none

`tome run <wf> --detach` validates the workflow (exit `2` if the frontmatter is invalid or a placeholder is undefined), records a new run, substitutes `{{params}}` / `{{run}}` placeholders into the body, and returns the run id right away. The run-side commands `tome step start "<name>"`, `tome step done|fail`, `tome run finish --status succeeded|failed [--summary …]` and `tome run cancel <id>` find the run through `TOME_RUN_ID` (or an explicit id) and record their transitions as step records and run status. The results show up in `tome runs show`. An agent can test the whole control plane by calling these commands directly, with no orchestrator. Covers use cases 5 (detach) and 8.

### 002-2. Attached runs & event stream

**Blocked by:** 002-1

`tome run <wf>` without `--detach` blocks and streams step events from the daemon, one line per transition in human mode or NDJSON with `--json`/`TOME_OUTPUT=json`. Agent pane output isn't included. The command exits with the run's final status: `0` succeeded, `1` failed, `130` cancelled, `2` invalid workflow. Ctrl-C, or the caller dying, cancels the run. An agent can test this by starting an attached run in the background, driving it with step/finish calls, signalling it, and checking the stream and exit codes. Covers use cases 2, 3 and 4 (CLI side).

### 002-3. Harness adapter + tmux sessions

**Blocked by:** none

A configurable harness-adapter command template for launching any agent CLI, with a built-in `claude` (Claude Code) preset. Beside it, a tmux-backed session layer that can launch a command in a named, visible session with a given environment, kill a session, and detect when the session's process has exited. The user can attach to these sessions to watch and type. An agent can test this on a headless tmux server with a stub harness command. Provides the launch path for 002-4 and the use case 6 plumbing.

### 002-4. Orchestrator launch

**Blocked by:** 002-1, 002-3

When a run starts, the daemon launches its orchestrator through the harness adapter, using `defaults.harness` or `defaults.orchestrator_harness` if set, in a dedicated session named like "tome: <workflow> #<n>". The bootstrap is the built-in orchestrator prompt (the tome command reference and the orchestrator's duties), the resolved workflow body, any extra orchestrator instructions from the workflow, and the env vars `TOME_RUN_ID` and `TOME_OUTPUT=json`. Cancelling (attached Ctrl-C or `tome run cancel`) kills the run's sessions, keeps its worktrees, marks the run `cancelled` and sends no notification. This fills in the kill-sessions hook from crash recovery. If the orchestrator exits or its pane closes before `tome run finish`, the run is marked `failed` with reason `orchestrator_exited` and the notify hook is called. An agent can test the whole flow with a scripted stub orchestrator that reads its bootstrap and calls `tome step …` / `tome run finish`. An opt-in smoke test runs the real Claude Code preset. Covers use cases 1, 4, 6 and 7.

### 002-5. Run concurrency

**Blocked by:** 002-1

The workflow frontmatter can set `concurrency: N` with `on_conflict: queue | reject` to limit how many runs of that workflow are active at once. It's unlimited by default. A queued run waits and starts when a slot frees up. A rejected run fails fast with a clear error and a non-zero exit. This applies the same way to attached, detached and trigger-started runs. An agent can test it with detached runs held open by a stub or by leaving them unfinished. Covers use case 9.

### 002-6. `tome workflow new`

**Blocked by:** none

`tome workflow new <name> [-d <description>] [--global] [--force]` writes a starter workflow that passes validation: frontmatter with `name`, `description`, an example param and the optional keys commented out, and a body with example steps. It goes where `tome run <name>` will find it: the nearest project `.tome/workflows` (created in the working directory if there is none), or `~/.tome/workflows` with `--global`. It refuses (exit `2`) an invalid name, an existing file unless `--force`, and a second workflow with the same name in the same directory. Creating a project workflow that overrides a global one is allowed and reported. Works without the daemon. An agent can test it by creating a workflow, validating it, and starting a detached run of it by name.
