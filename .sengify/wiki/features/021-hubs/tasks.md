# Hubs: shared buses, queues, workflows and config — Task Plan

Source feature: [feature.md](./feature.md)

## Constraint: keep hub and global state movable to a remote owner

Hub and global state may later live on another machine. Build so that change is routing, not a rewrite:

- Every hub/global operation goes through one scope type (`Project | Hub(name) | Global`) and one façade (publish, list/show events, queue push/pull/peek/ack/close). No `"hub:<name>"` key strings outside the store layer.
- "Which daemon owns this scope" is a single lookup that answers "this machine" for now.
- Queue claims live only in the owning store. Queues are never forwarded or replicated (021-7 forwards bus events only).
- Hub workflows and config are read through one loader, so it can later fetch them instead of reading `~/.tome/hubs/<name>/`.
- Note the remote design (owner routing, remote attachments, timeouts, claim leases) in `docs/modules/bus.md`.

## Tasks

### 021-1. Hub registry and attachment commands

**Blocked by:** none

`tome hub create|rm|attach|detach|ls|show`, including `--global`. Hubs live under `~/.tome/hubs/<name>/` and attachments in the daemon's projects table, never in the repo. Hub names must be valid identifiers and can't be `global`. Attaching to a missing hub is an error with a hint. The new RPCs are routed through the daemon, and the commands go in `reach` so `--on` works. `rm` refuses while projects are attached; the pending-work checks come in 021-6. Introduce the scope type and the owner lookup here (always local).

### 021-2. Hub and global bus: publish and inspect

**Blocked by:** 021-1

Events are scoped to a bus (project, hub or global). `tome publish` and `tome events ...` take `--hub`/`--global` and refuse callers outside an attached project, with a hint to `tome hub attach`. The `tome.` prefix is reserved on every bus. Each event records the publishing project, and chain depth counts across scopes. Lifecycle events stay on the project bus. Publish and inspect go through the scope façade.

### 021-3. Hub and global queues

**Blocked by:** 021-1

`tome queue ...` takes `--hub`/`--global` with the same attachment check. Hub and global queues are shared by attached projects' agents, and claim, ack and the return of a claimed message when its worker ends work as for project queues. Queue operations go through the scope façade, and claims stay in the owning store.

### 021-4. Subscribing triggers (`hub:` / `global: true`)

**Blocked by:** 021-2

`on:` triggers take `hub: <name>` or `global: true`, and validation rejects missing hubs. Each subscribing workflow in each attached project gets its own delivery, and the run starts in that project. The publishing project is available as a trigger field. Global workflows still can't subscribe.

### 021-5. Hub workflows and config layering

**Blocked by:** 021-1

Workflow and config resolution walks a project's attached hubs as named layers, in the order project, hubs, global. Hub workflows apply to every attached project: relative globs resolve per project, cron fires once per project, and an `on:` without `hub:` listens on the project's own bus. When two attached hubs define the same name, `tome validate`, arming and running that name all report an error. `bus.forward` is allowed in hub and global config. Hub workflows and config load through one loader.

### 021-6. Safe detach and hub removal

**Blocked by:** 021-2, 021-3, 021-4

Detaching drops the project's pending deliveries for that hub and returns its claimed queue messages. `tome hub rm` refuses while projects are attached or work is pending, and names them. `--force` detaches every project and deletes the hub's deliveries, events, queues and files.

### 021-7. Forwarding hubs and global across nodes

**Blocked by:** 021-2, 021-5

A `bus.forward` rule in a hub's or the global config sends matching events to a node. The node publishes them on its hub of the same name, creating it on first use, or on its global bus. Only bus events are forwarded; queues never are. `--on <node>` combined with `--hub`/`--global` acts on that node's hub or global.
