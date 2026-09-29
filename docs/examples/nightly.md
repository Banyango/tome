---
name: nightly
description: A weekday-morning check of the open branches.
triggers:
  - manual
  - cron: "0 9 * * 1-5"
    params: {base: main}
params:
  base: {type: string, default: main}
---
Started at {{trigger.time}} for the schedule {{trigger.scheduled}}.

## Check
List the branches that are ahead of {{params.base}} and say which have gone
stale.
