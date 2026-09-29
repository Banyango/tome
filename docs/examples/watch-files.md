---
name: doc-check
description: Check the docs whenever a Markdown file under docs/ changes.
triggers:
  - file: "docs/**/*.md"
    on: [created, modified]
    debounce: 5s
    ignore: ["docs/drafts/**"]
    while_running: queue
---
These files changed:

{{trigger.paths}}

## Check
Read each changed file and fix broken links and typos. Do not touch any file
that isn't on the list, so this run doesn't start another one.
