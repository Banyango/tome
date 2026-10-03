---
name: defaults-tour
description: Every key under `defaults`.
mode: orchestrated              # orchestrator_* keys apply to orchestrated runs
defaults:
  backend: tmux                 # tmux or cmux
  harness: claude               # agent CLI for workers
  orchestrator_harness: claude  # agent CLI for the orchestrator
  timeout: 30m
  on_failure: notify
  start_timeout: 2m             # or `off`
  layout: split                 # where sessions open; see the sessions guide
---
Do the work described in the steps below.

## Work
Do it.
