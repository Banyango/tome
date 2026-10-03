---
name: plan-feature
mode: orchestrated
triggers:
  - file: "features/*/feature.md"
    on: [created]
---
A feature was just written down: {{trigger.paths}}

## Investigate
Spawn an agent worker to read the code this feature touches. It reports
the files that will change and the patterns to follow.

## Plan
Write `tasks.md` next to the feature: small tasks, each saying what it's
blocked by.

## Hand off
Run `tome publish feature.planned <slug>`, so `implement-feature` builds it.
