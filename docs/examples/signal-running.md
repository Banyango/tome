---
name: watcher
description: Keep one run going and hand it new work as messages arrive.
triggers:
  - manual
  - on: work.new
    to: running-or-new
concurrency: 1
---
Handle work items as they come in. This run is meant to stay alive.

Start with:

{{trigger.payload}}

## Loop
Do the item. Then pull the next message from your `events` queue with
`tome queue pull events --wait 5m`, ack it, and do that item. When the queue
is empty after the wait, finish the run successfully.
