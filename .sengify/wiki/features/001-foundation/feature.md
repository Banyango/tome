# Foundation

## Description

This feature defines how tome stores [[workflow]]s, runs its [[daemon]] and records [[run]] state.

### Workflow format

- **File:** a markdown file with yaml frontmatter for workflow-level settings and a natural-English body describing the [[step]]s.
- **Frontmatter:**
  - `name`: identity. A project workflow overrides a global one with the same `name`.
  - `description`
  - `triggers`: see [[trigger]]
  - `params`: typed inputs with defaults, overridable per run (`tome run <wf> --param k=v`)
  - `defaults`: workflow-wide [[backend]], harness ([[harness-adapter]]), `orchestrator_harness`, timeout and `on_failure`
  - `concurrency` / `on_conflict`: limit on simultaneous runs of this workflow (see 002)
- **Steps:** written in natural English in the body. `## Headings` are guidance for the orchestrator agent, not structured definitions. The orchestrator reads the body and decides order, branching, looping and parallel work at runtime (see [002 - Running a workflow](../002-running-a-workflow/feature.md)).

  ```markdown
  ## Implement
  Create a worktree off {{params.base}} and ...

  ## Review
  Once implementation is done, start a reviewer agent in its own pane to check the diff.
  If it asks for changes, go back to Implement; otherwise finish.
  ```

- **Placeholders:** `{{params.x}}` and `{{run.id}}`, substituted before the orchestrator sees the body. Substitution only, with no conditionals or loops.
- **Lookup:** workflows load from `~/.tome/workflows` (global) and `.tome/workflows` (project), and project overrides global.
- **Validation:** `tome validate` runs the checks on demand, and the same checks run before every run starts. They cover the frontmatter (yaml syntax and known keys) and undefined `{{placeholders}}`, and report errors with line numbers. The English body isn't validated. An invalid workflow never starts.
- **Mid-run edits:** the resolved workflow (after override lookup and param substitution) is saved with the run when it starts. Editing the file affects only new runs.

### Daemon

- There is one daemon per user per machine.
- It runs as an OS service. `tome daemon install` registers a launchd agent on macOS or a systemd `--user` unit on Linux. `tome daemon start/stop/status` are also available.
- The CLI talks to it with newline-delimited JSON-RPC over the Unix socket `~/.tome/tome.sock`.
- If the daemon isn't running, CLI commands fail with a hint. They do not start it.
- Output is human-readable by default. `--json` (or `TOME_OUTPUT=json`, which the harness adapter can set) gives stable machine output. Exit codes are meaningful.

### Run state storage

- The database is `~/.tome/tome.duckdb`, and only the daemon opens it.
- It stores runs (with project path and workflow snapshot), the step status and history the orchestrator reports through `tome step`, and a log index (path, size, tail excerpt).
- Raw logs are files at `~/.tome/runs/<run-id>/<step>.log`.
- **Inspection:** `tome runs list/show/logs`, and `tome query "<sql>"` for read-only SQL passed through the daemon.
- **Retention:** nothing is deleted automatically. `tome gc --older-than <age>` removes finished runs along with their log files and worktrees.

## Use Cases

1. As a human author, I want to write a workflow as a markdown file with yaml frontmatter and steps in natural English, so that it reads naturally and an orchestrator agent can carry it out.
2. As a human author, I want to keep workflows in `~/.tome/workflows` and override them per project, so that I can reuse a workflow across projects and change it slightly.
3. As an author or agent, I want to pass `--param` values at run time, so that one workflow covers small variations without copying the file.
4. As an author or agent, I want `tome validate` to report errors with line numbers, so that I catch broken workflows before running them.
5. As a user, I want each run to use the workflow as it was when the run started, so that editing a file doesn't break runs already in progress.
6. As a user, I want `tome daemon install` to register tome as a launchd or systemd service, so that the daemon is always available.
7. As an agent, I want `--json` output and meaningful exit codes, so that I can parse tome's responses reliably.
8. As a user or agent, I want `tome runs list/show/logs` and `tome query`, so that I can inspect current and past runs.
9. As a user, I want a daemon crash to fail in-progress runs cleanly while keeping their worktrees, so that no agent keeps working unsupervised and no work is lost.
10. As a user, I want `tome gc` to prune old runs on demand, so that disk usage stays under my control.

## Constraints

- Written in Rust. Supports macOS and Linux; Windows is out of scope for v1.
- DuckDB allows only one read-write process, so every database access goes through the daemon.
- Agents are the main CLI callers, so machine output (`--json`) and exit codes are a stable contract.
- Templating is plain substitution, with no logic.

## Edge Cases

- **Daemon not running when a CLI command is called:** the command fails with a hint to run `tome daemon start` or `tome daemon install`.
- **Daemon crashes or restarts during runs:** all in-progress runs are marked failed with reason `daemon_restart`, their [[session]]s (the orchestrator's and the workers') are killed, their [[worktree]]s are kept, and the user gets a [[notification]].
- **Invalid workflow** (bad frontmatter yaml, unknown frontmatter key, undefined `{{placeholder}}`): the run is refused with exit code `2`, the errors are reported with line numbers, and no orchestrator is launched.
- **Workflow file edited mid-run:** the run keeps using its saved copy, and the edit affects only new runs.
- **Global and project workflows share a `name`:** the project workflow wins.
- **DuckDB schema changes between tome versions:** the daemon migrates the schema automatically on startup.

## Related Entities

- [[workflow]]
- [[step]]
- [[run]]
- [[daemon]]
- [[trigger]]
- [[backend]]
- [[harness-adapter]]
- [[session]]
- [[worktree]]
- [[notification]]
