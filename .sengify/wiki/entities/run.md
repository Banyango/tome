# Run

## Definition

One execution of a [[workflow]]. Run state is the only thing tome keeps in its database.

## Attributes

- storage: DuckDB. _⚠ `intent.md` says SQLite; the interview replaced it with DuckDB. See the flag in [[overview]]. Resolved 2026-09-26: `intent.md` now says DuckDB._
- contents: runs, [[step]] status, history, logs

## Relationships

- [[workflow]]: a run executes one workflow
- [[step]]: a run tracks the status of each step
- [[daemon]]: the daemon creates runs and moves them forward

## Planned Features

- Run state storage in DuckDB: schema for runs, step status, history and logs, plus ways to look at past and current runs _(source: .sengify/sources/feature-set-2026-09-26.md)_
- Running a workflow via `tome run`: starting a run, advancing steps, branch/loop _(source: .sengify/sources/feature-set-2026-09-26.md)_

## Sources

- intent.md
- .sengify/sources/interview-2026-09-26.md
- .sengify/sources/feature-set-2026-09-26.md
