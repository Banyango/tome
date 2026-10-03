---
name: plan-feature
description: When a feature file is added, investigate the codebase and break the feature into tasks.
mode: orchestrated
triggers:
  - manual
  - file: "features/*/feature.md"
    on: [created]
    debounce: 5s
---
A feature was just written down. Work out how it fits this codebase and
break it into tasks.

New feature files (empty for a manual run):

{{trigger.paths}}

## Find features
Each path above looks like `features/<slug>/feature.md`. If there are no
paths, use every directory under `features/` that has a `feature.md` but no
`tasks.md`. Skip any feature that already has a `tasks.md`. If nothing is
left, finish the run successfully and say so.

## Investigate
Create a group named `investigate`. For each feature, spawn an agent worker
in it named `investigate-<slug>`. Tell it to read the feature file, then
find the code the feature touches: the modules and entry points, the tests
that cover them, and any similar feature that was built before. It must not
change any files. It reports as its summary the files that will change, the
patterns to follow, and any risks or open questions. Wait for the group.

## Break down
For each feature, write `features/<slug>/tasks.md` from the feature file and
its investigator's summary. Give each task a `### <N>. <title>` heading, a
`**Blocked by:**` line (task numbers, or `none`), and a short paragraph on
what changes, where, and how to tell it's done. Keep each task small enough
for one agent to implement and test as a single commit. List open questions
at the end.

## Hand off
For each feature that got a `tasks.md`, publish an event so the next
workflow can pick it up:

    tome publish feature.planned <slug>

## Finish
Finish the run with each feature and its number of tasks. If an
investigator failed, name its feature and fail the run.
