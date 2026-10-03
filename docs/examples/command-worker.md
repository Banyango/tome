---
name: test-then-fix
description: Run the tests as a command worker, and start an agent to fix any failures.
mode: orchestrated
---
Run the test suite and fix what breaks.

## Test
Spawn a command worker named `tests` that runs `cargo test`, and wait for it.
If it succeeded, finish the run successfully.

## Fix
Spawn an agent worker named `fixer` on its own worktree, told to make the
failing tests pass and to commit its work. Give it the tail of the test
output from the `tests` worker's summary. Wait for it. If it failed, fail the
run.

## Verify
Merge the `fixer` branch, run the tests again as a command worker, and finish
the run with the result.
