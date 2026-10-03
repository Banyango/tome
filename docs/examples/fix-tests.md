---
name: fix-tests
description: Whenever Rust source changes, run the tests and fix any failures on a branch.
mode: orchestrated
triggers:
  - manual
  - file: "src/**/*.rs"
    debounce: 10s
    while_running: queue
concurrency: 1
---
These files just changed:

{{trigger.paths}}

## Test
Spawn a command worker named `tests` that runs `cargo test`, and wait for it.
If it succeeded, finish the run successfully.

## Fix
Spawn an agent worker named `fixer` with its own worktree. Tell it to make the
failing tests pass without changing what the tests check, and to commit its
work. Give it the tail of the test output from the `tests` worker's summary.
Wait for it. If it failed, fail the run and say why.

## Report
Don't merge the branch. Finish the run with the `fixer` branch name and a
one-line description of the fix as the summary, so it can be reviewed.
