---
name: hello
description: A first run. One worker writes a greeting to a file.
params:
  who: {type: string, default: world, description: "Who to greet"}
defaults:
  backend: tmux
  harness: claude
  timeout: 10m
---
This is a first workflow. It starts one worker and checks what it wrote.
Run {{run.id}} greets {{params.who}}.

## Greet
Spawn one agent worker named `greeter`. Tell it to write a one-line
greeting for {{params.who}} to `greeting.txt` in the current directory, then
report done with the greeting as its summary.

## Check
Wait for `greeter` to finish. If it failed, fail the run and say why.
Otherwise read `greeting.txt`, confirm it isn't empty, and finish the run
successfully with the greeting as the summary.
