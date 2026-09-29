---
name: one-at-a-time
description: Never run twice at once; extra runs wait their turn.
concurrency: 1
on_conflict: queue     # or `reject` to refuse a run when one is going
orchestrator: |
  Keep your messages to the user short. Ask before deleting anything.
---
Do the work.

## Work
Do it.
