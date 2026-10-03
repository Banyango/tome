---
name: review-layout
description: Reviewers open as splits next to the orchestrator; everything else below.
mode: orchestrated
defaults:
  backend: tmux
  harness: claude
  layout:
    preset: split
    split: { size: 30% }
    orchestrator: { layout: tab }
    workers:
      - match: "review-*"
        layout: split
        split: { direction: right }
        from: orchestrator
      - match: "*"
        layout: split
        split: { direction: down }
---
Review the change with one worker per file.

## Review
Spawn workers named `review-<n>`, one per changed file, and wait for them.
