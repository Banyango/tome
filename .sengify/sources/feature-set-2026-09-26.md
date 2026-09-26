# Initial Feature Set — 2026-09-26

The starting list of features to define with `/sengify:feature`, drawn from the wiki. They're in dependency order, so each builds on the ones above it.

## Foundation

1. **Workflow definition format**: the markdown + frontmatter layout for a workflow, with its steps, triggers and failure conditions, plus loading from `~/.tome/workflows` and `.tome/workflows` with project overriding global. Open gap: the frontmatter layout.
2. **Daemon lifecycle and CLI↔daemon protocol**: start, stop and status for the daemon, how CLI commands reach it, and what happens when it isn't running.
3. **Run state storage in DuckDB**: the schema for runs, step status, history and logs, and how you look at past and current runs.

## Execution core

4. **Running a workflow**: `tome run`, starting a run, advancing through steps, and branch/loop.
5. **Step completion and failure handling**: the four ways a step completes (agent calls tome, process exits, timeout, human approves), user-defined failure conditions, and the default of notifying the user.
6. **Harness adapter**: the command template for launching any CLI agent. Open gap: the template format.

## Primitives

7. **Terminal backends (cmux, tmux, herdr)**: one session interface over three multiplexers. All three are in v1 scope.
8. **Queues**: create, push and pull, and whether a message is delivered once or can be seen again. Open gap: delivery.
9. **Worktree management**: allocating git worktrees per step, cleaning them up, and merging results back for fan-in. Open gap: lifecycle.
10. **Groups, fan-out and fan-in**: spreading work across agents, tracking the group, and consolidating results.

## Triggers and I/O

11. **Triggers**: file created or edited, group complete, and message received from a tome queue, an external webhook, or matched agent output. Could be split into three: file triggers, message triggers, and matching agent output.
12. **Notifications and human approval**: delivery through the multiplexer, macOS and webhook/push, plus the approve step.
13. **Send message action**: pushing to a queue or into a running agent's pane.

## Authoring

14. **Workflow authoring skill**: the skill humans use to write and edit workflows.

## Suggested order

Start with #1 and #4, because nearly every other feature depends on the workflow format and the run model.
