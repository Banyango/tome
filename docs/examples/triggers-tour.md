---
name: triggers-tour
description: One of each kind of trigger.
params:
  reason: {type: string, default: manual}
triggers:
  - manual
  - file: "src/**/*.rs"
    on: [created, modified]
    debounce: 5s
    ignore: ["target/**"]
    while_running: queue
    params: {reason: files}
  - cron: "0 9 * * 1-5"
    params: {reason: schedule}
  - on: review.requested
    params: {reason: message}
---
Started because of: {{params.reason}}.

## Work
Do it.
