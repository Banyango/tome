# Hubs: shared buses, queues, workflows and config

## Description

Today tome has two scopes: global (`~/.tome`) and project (`.tome/`). The message bus (feature 009) and queues (feature 003) belong to one project, so projects can't coordinate. Global workflows can't subscribe to topics at all.

This feature adds a **hub**: a named, machine-local group of projects that sits between global and project (project, then hubs, then global). A hub holds a **bus**, **queues**, **workflows** and **config**, kept under `~/.tome/hubs/<name>/`. The **global** scope also gets a bus and queues. A project reaches a hub or global only by **attaching** to it.

### Hubs and attaching

- A hub is created by name (`tome hub create <name>`) and removed with `tome hub rm <name>`. A hub can't be named `global`. Projects can live anywhere on disk.
- A project can attach to **many hubs** and to **global**: `tome hub attach <name>`, `tome hub attach --global`, `tome hub detach ...`, `tome hub ls|show`. Attaching to a hub that doesn't exist is an error with a hint.
- The attachment is recorded in the daemon's registry (the `projects` table), per machine. Nothing is written into the repo, so a teammate without the hub is unaffected.
- Attaching to global is opt-in for the **global bus and queues**. Global workflows and config keep applying to every project, as they do now.

### Bus and queues

- **Addressing:** topic and queue names stay plain. Commands take `--hub <name>` or `--global`: `tome publish --hub release review.requested "…"`, `tome queue push --hub release builds "…"`, and the same flags on `tome events ...` and `tome queue ls|...`. With neither flag, the project's own bus and queues are used, unchanged.
- **Who can use them:** only a caller in (or running from) a project attached to that hub or to global. From anywhere else, tome refuses with a hint to run `tome hub attach`.
- **Subscribing:** a trigger takes `hub: <name>` or `global: true`:

```yaml
triggers:
  - on: review.requested
    hub: release        # or `global: true`; neither = the project's own bus
```

  Only `on:` triggers of attached projects' workflows can subscribe. Each subscribing workflow in each attached project gets its own delivery (fan-out), as on the project bus, and the run starts in that project. The event records the publishing project, and a subscriber sees it as a trigger field.
- **Patterns, deliveries, ordering, payload limits and the `to:` rules** are as on the project bus. Each bus (a project's, each hub's, global) is its own namespace of topics.
- **Reserved prefix:** `tome.` is reserved on every bus. Publishing to it is refused.
- **Lifecycle events:** `tome.run.<workflow>.*` stays on the project bus only. To chain across projects, a workflow publishes a hub or global event itself.
- **Depth:** an event chain's depth counts across scopes, with the same limit of 8.
- **Queues:** a hub or global queue is shared by the attached projects' agents. Claim, ack and the return of a claimed message when its worker ends work as for project queues.

### Hub workflows and config

- A hub's `workflows/` and `config.yaml` apply to every attached project as if copied into it: relative file globs resolve against that project's root, cron fires one run per attached project, and an `on:` without `hub:` listens on that project's own bus.
- Precedence is project, then hub, then global; a nearer layer shadows a wider one of the same name.
- If two hubs a project is attached to define the same workflow name or the same named config entry, that is an **error**, not a precedence rule. `tome validate` and arming report it, and so does running that name, until one hub is detached or the project defines its own. Hub workflows can't be started by name from an unattached project.
- `bus.forward` rules are allowed in a hub's config and in the global config, besides the project config. `herdr` and `nodes` stay global-only.

### Nodes

- Hubs and global forward across nodes. A `bus.forward` rule in a hub's or the global config sends matching events to a node, which publishes them on its own hub of the same name (created on first use, as projects are today) or on its global bus.
- `--on <node>` with `--hub` or `--global` acts on that node's hub or global.

### Detaching and removal

- `tome hub detach` drops the project's pending deliveries for that hub. Queue messages its workers had claimed go back on the queue.
- `tome hub rm <name>` refuses while any project is attached or any delivery or queue message is pending, naming them. `--force` detaches every project, drops pending deliveries and deletes the hub's events, queues and files.

## Use Cases

1. As a user, I want to create a hub and attach several projects to it, so that those projects can coordinate without being global.
2. As a project workflow author, I want to subscribe a workflow with `hub: release`, so that it starts when another project in the hub publishes.
3. As an agent in a project, I want to publish to a hub or to global, so that other attached projects react.
4. As an orchestrator, I want to push work onto a hub queue that agents in other attached projects pull from, so that work is shared across projects.
5. As a user, I want a project attached to several hubs, so that one project can belong to several groups.
6. As a user, I want to put shared workflows and config in a hub, so that attached projects get them without copying, and a project can override them.
7. As a user, I want to attach a project to global, so that it can use the global bus and queues while unattached projects stay isolated.
8. As a user, I want to inspect and repair hub and global events and queues (`tome events ls --hub`, `tome queue ls --global`), so that I can debug cross-project flows.
9. As a user, I want to detach a project or remove a hub safely, so that pending work isn't lost silently.
10. As a user with several machines, I want hub and global events to forward to a node, so that the groups span nodes.

## Constraints

- Hubs are named and machine-local; their data lives under `~/.tome/hubs/<name>/` and in the daemon's DuckDB store. Attachments live in the daemon registry, never in the repo.
- The project bus, project queues and project workflows keep working unchanged. Global workflows and config still apply to every project without attaching.
- Global workflows still have no topic triggers. Only attached projects' workflows subscribe to hub and global buses.
- A hub name can't be `global` and must be a valid identifier.
- The daemon keeps the only DuckDB connection; new RPCs are listed in their module's `METHODS` and routed in `Engine::dispatch`. New commands go in `main.rs` `reach` so `--on` routing is right.
- Workflow and config resolution must walk a project's attached hubs, so a hub layer carries a name; one `Layer` value can't stand for all hubs.

## Edge Cases

- Caller not attached to the hub or global → publish, push and subscribe are refused with a hint to `tome hub attach`.
- Attach, `--hub` or `hub:` names a hub that doesn't exist → error naming the hub.
- Two attached hubs define the same workflow name or config entry → error until one is detached or the project overrides it.
- A project detaches while deliveries are pending → they are dropped; claimed queue messages return to the queue.
- `tome hub rm` with projects attached or work pending → refused, listing them; `--force` removes everything.
- Event chain deeper than 8 across scopes → refused and recorded, as today.
- Publishing a `tome.` topic on a hub or global → refused with exit `2`.
- A hub topic with no subscribers → the event is still recorded and shown by `tome events`.
- Forwarded event for a hub that doesn't exist on the target node → the hub is created there on first use.
- `--hub` or `--global` without a project and outside a run → refused with "not inside an attached project".

## Related Entities

- [[queue]]
- [[event]]
- [[trigger]]
- [[workflow]]
- [[node]]
- [[daemon]]
- [[run]]
