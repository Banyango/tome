---
name: review
description: Review the change that a successful implement run made.
triggers:
  - on: tome.run.implement.succeeded
---
Review the change made by this run. The event says which run it was and
gives its summary:

{{trigger.payload}}

## Review
Read the branch named in the summary and write your findings to `review.md`.
