# Workflow

## Definition

A reusable, user-authored definition of the steps an agentic process should take. Users specify workflows through a skill, and tome runs them.

## Attributes

- format: markdown with yaml frontmatter (`name`, `description`, `triggers`, `params`, `defaults`, `concurrency`, `on_conflict`) and a natural-English body
- location: `~/.tome/workflows` (global library) or `.tome/workflows` (project); a project workflow overrides a global one with the same name
- steps: described in natural English under `## Headings`; the [[orchestrator]] decides order, branching and looping at runtime
- triggers: the [[trigger]]s that start the workflow or advance its steps
- failure conditions: user-specified; by default, notify the user
- capabilities: run agents, open multiplexer windows, run CLI commands, notify the user, etc.

## Relationships

- [[step]]: a workflow is made of steps
- [[run]]: each execution of a workflow is a run
- [[trigger]]: triggers start the workflow or advance it
- [[orchestrator]]: carries out the workflow body during a run
- [[notification]]: the default response to a failure

## Planned Features

- Workflow definition format: the frontmatter layout is still open _(source: .sengify/sources/feature-set-2026-09-26.md)_
- Running a workflow via `tome run` _(source: .sengify/sources/feature-set-2026-09-26.md)_
- Workflow authoring skill _(source: .sengify/sources/feature-set-2026-09-26.md)_

## Sources

- intent.md
- .sengify/sources/interview-2026-09-26.md
- .sengify/sources/feature-set-2026-09-26.md
- features/002-running-a-workflow/feature.md
