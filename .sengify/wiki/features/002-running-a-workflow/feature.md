# Running a workflow

## Description

A [[run]] is driven by an **orchestrator agent**. The [[daemon]] launches it in its own visible [[session]], and it reads the [[workflow]]'s natural-English body and carries it out live by calling tome commands. The daemon doesn't compile the steps, so it records what the orchestrator reports and enforces run-level rules.

### Workflow shape

- **Frontmatter:** stays yaml: `name`, `description`, `triggers`, `params`, `defaults`, plus `concurrency` and `on_conflict`. The daemon needs these before any agent starts.
- **Body:** plain English. `## Headings` are guidance for the orchestrator, with no per-step yaml.

  ```markdown
  ---
  name: review-loop
  triggers: [manual]
  params:
    base: {default: main}
  defaults:
    harness: claude
  ---

  ## Implement
  Create a worktree off {{params.base}} and ...

  ## Review
  Once implementation is done, start a reviewer agent in its own pane to check the diff.
  If it asks for changes, go back to Implement; otherwise finish.
  ```

- **Placeholders:** `{{placeholders}}` (`params`, `run`) are substituted before the orchestrator sees the body.
- **Validation:** `tome validate` checks the frontmatter and placeholders only.

### Starting a run

- `tome run <wf> [--param k=v]` blocks and streams step events, one line per transition, or NDJSON with `--json`. Agent pane output isn't included; use `tome runs logs` for that.
- **Exit codes:** `0` succeeded, `1` failed, `130` cancelled, `2` invalid workflow.
- **Ctrl-C** or the caller dying cancels the run.
- `tome run --detach` returns the run id right away. Runs started by a [[trigger]] are always detached. A detached run ends only by finishing, by `tome run cancel <id>`, or by failing.
- **Concurrency:** unlimited by default. The frontmatter can set `concurrency: N` with `on_conflict: queue | reject`.

### Orchestrator

- It's launched through the [[harness-adapter]] in a dedicated [[session]] (e.g. "tome: review-loop #42"). The user can watch it and type into it.
- **Harness:** uses `defaults.harness` unless `defaults.orchestrator_harness` is set.
- **Bootstrap:** a built-in orchestrator prompt (the tome command reference and its duties), the resolved workflow body, any extra orchestrator instructions from the workflow, and the env vars `TOME_RUN_ID` and `TOME_OUTPUT=json`.
- **Control flow:** it decides order, branching and looping, and spawns workers through tome primitives ([[session]], [[worktree]], [[group]], [[queue]]).
- **Progress reports:** `tome step start "<name>"` and `tome step done|fail`. These calls are the [[step]] records in DuckDB and the streamed events.
- **Ending the run:** `tome run finish --status succeeded|failed [--summary …]`.

### Cancellation

Ctrl-C on an attached run or `tome run cancel <id>` kills the run's sessions (the orchestrator's and the workers'), keeps its worktrees, and marks the run `cancelled`. No failure [[notification]] is sent.

### Out of scope

- How workers signal completion, and the details of failure handling (feature: step completion and failure handling)
- The primitives: backends, queues, worktrees, groups
- Trigger matching

## Use Cases

1. As a human author, I want to write workflow steps in natural English, so that workflows are easy to write and change without learning a schema.
2. As a user, I want `tome run <wf>` to stream step progress and exit with the run's status, so that I can follow along and script around it.
3. As an agent, I want `tome run --json` to emit NDJSON events and a status exit code, so that I can launch a workflow and react to how it ends.
4. As a user, I want Ctrl-C to cancel the run, killing its panes but keeping its worktrees, so that stopping is quick and no work is lost.
5. As a user, I want `--detach`, and detached runs from triggers, so that long or automatic workflows don't need a terminal open.
6. As a user, I want to watch and type into the orchestrator's pane, so that I can see its reasoning and redirect it.
7. As an orchestrator agent, I want a built-in prompt with the tome command reference, so that any harness can drive a workflow.
8. As a user, I want the orchestrator's `tome step` reports recorded, so that `tome runs show` and the stream show meaningful progress.
9. As an author, I want `concurrency` and `on_conflict` settings, so that trigger-heavy workflows don't pile up runs.

## Constraints

- The workflow body is natural English and is never parsed as structured data. The frontmatter remains yaml.
- Run behavior can vary between runs, because an agent interprets the workflow.
- The daemon enforces no run timeout and no loop limit (a deliberate choice).
- The orchestrator must work through the generic [[harness-adapter]], so it can't depend on harness-specific features such as skills.

## Edge Cases

- **Orchestrator exits or its pane is closed before `tome run finish`:** the run is marked `failed` with reason `orchestrator_exited`, and the user is notified (the default failure handling).
- **Concurrency limit reached:** the new run is queued or rejected, depending on `on_conflict`.
- **Invalid frontmatter or an undefined placeholder:** exit code `2`, and no orchestrator is launched.
- **Caller of an attached run dies:** the run is cancelled.
- **Orchestrator loops forever:** nothing stops it automatically; the user runs `tome run cancel <id>`.

## Related Entities

- [[workflow]]
- [[run]]
- [[step]]
- [[daemon]]
- [[session]]
- [[harness-adapter]]
- [[trigger]]
- [[worktree]]
- [[group]]
- [[queue]]
- [[notification]]
