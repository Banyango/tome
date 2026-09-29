---
name: summarize
description: Summarize a file into a short note.
params:
  file: {type: string, description: "The file to summarize"}
  words: {type: int, default: 100, description: "Length limit"}
  bullets: {type: bool, default: false}
---
Summarize `{{params.file}}` in at most {{params.words}} words.
Use bullets: {{params.bullets}}. This is run {{run.id}}.

## Read
Read the file.

## Summarize
Write the summary to `summary.md` and finish the run.
