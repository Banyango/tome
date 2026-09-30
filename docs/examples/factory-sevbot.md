---
name: factory-sevbot
description: Investigate an outage, propose mitigations, and answer questions until it is resolved.
triggers:
  - manual
  - on: alert.outage
    to: running-or-new
  - on: incident.question
    to: running-or-new
concurrency: 1
---
There is an incident. The first report:

{{trigger.payload}}

Keep a log in `.tome/factory/incident-{{run.id}}.md`, and add to it as you go.

## Investigate
Create a group named `investigate`. Spawn agent workers in it: one reads the
metrics and alerts, one reads the logs, and one reads the deploys and
commits from the last day. None of them changes anything. Each reports what
it found, with times, as its summary. Wait for the group.

## Mitigate
From the summaries, write down the most likely cause and a ranked list of
mitigations, such as turning off a flag or rolling back a deploy. Don't
apply any of them. Publish the top one so an engineer can act on it:

    tome publish incident.proposed "<cause>: <mitigation>"

## Answer
Pull from your `events-$TOME_RUN_ID` queue with `tome queue pull events-$TOME_RUN_ID --wait 15m`. An
event on `alert.outage` is more evidence: add it to the log and look again
at the cause. An event on `incident.question` is a question: answer it in the
log and publish the answer on `incident.answer`. Ack each one. Keep going
until an event says the incident is resolved, or the queue has been empty
for an hour.

## Finish
Write a timeline and a summary at the top of the log. If a fix is needed,
publish it:

    tome publish factory.requested "<the fix, and a link to the log>"

Finish the run with the path to the log.
