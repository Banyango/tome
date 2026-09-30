---
name: factory-deploy
description: Roll out a shipped branch behind a feature flag and watch it until it is fully live.
triggers:
  - on: factory.ship
concurrency: 1
---
Deploy this branch: {{trigger.payload}}

Deploys use `./scripts/deploy` and feature flags use `./scripts/flag`. Change
these to your own tools.

## Merge
Merge the branch into main and push. If the merge conflicts, fail the run:
the change needs to go back through `factory-build`.

## Deploy
Run `./scripts/deploy main` as a command worker and wait for it. Fail the
run if it fails.

## Dashboard
Make a dashboard for the metrics this change could move: its endpoints'
latency and error rate, and anything the review file in `.tome/factory/`
calls out. Record where it is.

## Roll out
Turn the change's flag on for 1%, then 10%, 50% and 100%. At each stage,
wait 10 minutes and check the dashboard against the hour before. If latency
or errors get worse, turn the flag off, then publish what you saw:

    tome publish alert.outage "<branch>: <what regressed, and the dashboard>"

and fail the run.

## Finish
Finish the run successfully with the branch, the dashboard and the final
flag state.
