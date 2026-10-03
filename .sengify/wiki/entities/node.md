# Node

## Definition

Another machine with tome installed, reached over SSH, whose [[daemon]] can be driven from this one. Each node owns its own runs, sessions, worktrees and run store. Nothing is shared between nodes.

## Attributes

- name: `[a-z0-9_-]`, unique; `local` is reserved for this machine _(source: [014](../features/014-remote-nodes/feature.md))_
- ssh: the SSH destination (anything `ssh` accepts) _(source: [014](../features/014-remote-nodes/feature.md))_
- tome: the tome binary on the node (default `tome`) _(source: [014](../features/014-remote-nodes/feature.md))_
- projects: where this machine's projects are on the node; defaults to the same home-relative path _(source: [014](../features/014-remote-nodes/feature.md))_
- configured in the global `~/.tome/config.yaml` only _(source: [014](../features/014-remote-nodes/feature.md))_

## Relationships

- [[daemon]]: each node runs its own daemon; `--on <node>` sends requests to it
- [[run]]: a run is owned by one node from start to finish; referenced as `<node>:<id>`
- [[session]]: remote tmux sessions can be viewed locally with `tome session view`
- [[event]]: a project's `bus.forward` rules deliver events to nodes

## Planned Features

- Remote workers: one run's workers on other nodes (the next phase after 014)

## Sources

- features/014-remote-nodes/feature.md
