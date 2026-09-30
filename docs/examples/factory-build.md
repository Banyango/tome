---
name: factory-build
description: Build a requested change, loop until CI and the reviewers pass, then ship it or ask an engineer.
params:
  base: {type: string, default: main, description: "Branch the change starts from"}
  max_attempts: {type: int, default: 3, description: "Fix rounds before giving up"}
triggers:
  - manual
  - on: factory.requested
concurrency: 1
---
Build this change:

{{trigger.payload}}

## Implement
Spawn an agent worker named `builder` on its own worktree, branching from
{{params.base}}. Give it the request above. Tell it to use whatever context
it needs (the code and its docs, and any Slack, Notion or metrics tools its
harness has), to commit its work, and to report the branch and what it
changed as its summary. Wait for it. If it failed, fail the run. Its branch
is now the change's branch.

## CI
Spawn a command worker named `ci-<n>` that runs `cargo build && cargo test`
in the change branch's worktree, and wait for it. If it failed, spawn an
agent worker `fix-<n>` on a new worktree branching from the change's branch,
with the tail of the output from the CI worker's summary. Tell it to commit a
fix. Its branch becomes the change's branch; run CI again. Stop after
{{params.max_attempts}} rounds and fail the run with the last output.

## Review
Create a group named `reviews`. Spawn four agent workers in it, each told to
review the change's branch against {{params.base}} from one angle only:
`review-data` (schemas, migrations, queries), `review-infra` (config,
deploys, resource use), `review-cloud` (cloud services and permissions) and
`review-security` (auth, secrets, input handling). Each must not change any
files. It reports its findings as its summary, each marked `blocking` or
`minor`, and ends with a risk of `low` or `high`. Wait for the group.

If any reviewer has a blocking finding, send those findings to a new fix
worker the same way, go back to CI, and review again. This counts
towards the same {{params.max_attempts}} rounds.

## Classify
The change is low-risk only if every reviewer said `low` and nothing is
blocking. Write the reviews and the verdict to `.tome/factory/<branch>.md`.

## Hand off
If the change is low-risk, ship it:

    tome publish factory.ship <branch>

Otherwise ask for an engineer and don't ship:

    tome publish factory.review-needed "<branch>: <why it is high-risk>"

## Finish
Finish the run successfully with the branch, the verdict and where the
review file is.
