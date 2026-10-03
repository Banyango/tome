---
name: fan-out
description: Review every file changed on a branch in parallel, then combine the reviews.
mode: orchestrated
params:
  base: {type: string, default: main, description: "Compare the current branch to this one"}
concurrency: 1
---
Review the files changed since {{params.base}}, one worker per file.

## Split
List the files that changed between {{params.base}} and HEAD. If there are
none, finish the run successfully and say so.

## Review
Create a group named `reviews`. For each changed file, spawn an agent worker
in that group, named `review-<n>`, told to review only that file and to
report its findings as its summary. Then wait for the group.

## Combine
Read every member's summary from the group's status. Write one report to
`review.md`, most serious findings first. If any reviewer failed, list it in
the report and carry on. Finish the run successfully with the path to the
report.
