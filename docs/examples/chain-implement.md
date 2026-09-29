---
name: implement
description: Implement one requested feature at a time.
triggers:
  - on: feature.requested
concurrency: 1
---
Implement this feature:

{{trigger.payload}}

## Implement
Make the change on a new branch and commit it.

## Finish
Finish the run successfully, with the branch name as the summary.
