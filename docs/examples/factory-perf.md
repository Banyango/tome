---
name: factory-perf
description: Look for latency regressions, drop duplicates, and request fixes for the real ones.
mode: orchestrated
triggers:
  - manual
  - cron: "0 * * * *"
  - on: alert.latency
concurrency: 1
---
Look for latency regressions in production. An alert that started this run,
if any:

{{trigger.payload}}

## Detect
Compare the last hour's latency for each service with the same hour last
week. List every endpoint whose p95 is more than 20% slower. Add the alert
above if it isn't on the list already.

## Dedupe
Drop anything already in `.tome/factory/perf-open.md`, and merge
regressions that share a cause, such as endpoints slowed by one deploy. If
nothing is left, finish the run successfully and say so.

## Investigate
Create a group named `perf`. For each regression, spawn an agent worker in
it named `perf-<n>`, told to find the cause from the metrics, traces and
recent commits, without changing any files, and to report the cause and a
proposed fix as its summary. Wait for the group.

## Request fixes
For each regression with a proposed fix, add it to
`.tome/factory/perf-open.md` and hand it to the factory:

    tome publish factory.requested "<the regression, its cause and the proposed fix>"

## Finish
Finish the run with each regression and whether a fix was requested.
