# Step

## Definition

A named unit of work in a [[workflow]], written in natural English under a `## Heading` in the workflow body. The [[orchestrator]] reads it and carries it out, for example by launching an agent, running a command, fanning out, or waiting for approval.

## Attributes

- name: the name the orchestrator reports with `tome step start "<name>"`
- status: reported by the orchestrator with `tome step start` / `tome step done|fail`
- completion conditions (any of):
  - the agent explicitly calls tome to report that it's done
  - the launched process exits
  - a timeout expires
  - a human approves
- failure conditions: user-specified per workflow; by default, notify the user
- inputs/outputs: files on disk and/or a git [[worktree]]

## Relationships

- [[workflow]]: steps belong to a workflow
- [[run]]: step status is recorded as part of run state
- [[orchestrator]]: carries out the step and reports its progress
- [[action]]: the orchestrator uses actions to carry out a step
- [[session]]: a step may run inside a multiplexer session
- [[worktree]]: a step may get its own worktree for isolated changes
- [[group]]: fan-out steps are tracked as a group

## Planned Features

- Step completion and failure handling _(source: .sengify/sources/feature-set-2026-09-26.md)_

## Sources

- intent.md
- .sengify/sources/interview-2026-09-26.md
- .sengify/sources/feature-set-2026-09-26.md
- features/002-running-a-workflow/feature.md
