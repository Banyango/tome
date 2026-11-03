# Hubs

A **hub** is a named group of projects on one machine. Projects attached to
a hub can share a message bus, queues, workflows and config without making
them global. Hubs sit between the global scope (`~/.tome`) and a project
(`.tome/`): project first, then its hubs, then global.

## Create and attach

```sh
tome hub create release        # makes ~/.tome/hubs/release/
cd ~/src/api && tome hub attach release
cd ~/src/web && tome hub attach release
tome hub attach --global       # opt this project into the global bus and queues
```

- A hub name is an identifier (a letter or `_`, then letters, digits, `_` or
  `-`) and can't be `global`.
- A project can attach to any number of hubs, and to global.
- Attaching to a hub that doesn't exist is an error; create it first.
- Attachments are recorded in this machine's daemon, never in the repo, so a
  teammate without the hub is unaffected.

## Hub and global buses

An attached project can publish to and inspect a hub's bus, or global's:

```sh
tome publish --hub release build.done "api 1.4.0"
tome publish --global deploy.started "..."
tome events ls --hub release
tome events show build.done --hub release   # SENDER shows the publishing project
tome events retry <event> --hub release     # retry and remove take it too
```

- A project that isn't attached is refused, with a hint to `tome hub attach`.
- `tome.` topics are reserved on every bus, and tome's own run events stay on
  the project bus.
- A chain's depth counts across buses: an event a run publishes to a hub is
  one deeper than the event that started the run.
- `--on` can't publish to another node's hub yet.

A workflow subscribes to a hub's bus with `hub:` (or global's with
`global: true`) on its `on:` trigger:

```yaml
triggers:
  - on: build.*
    hub: release
```

- Each subscribing workflow in each attached project gets its own delivery,
  and its run starts in that project. `{{trigger.publisher}}` is the project
  that published the event.
- A trigger naming a hub that doesn't exist fails `tome validate`.
- A project's own bus and a hub's are separate: a trigger without `hub:`
  never gets hub events, and one with it never gets the project's.
- Global workflows (`~/.tome/workflows/`) still can't subscribe.

## Hub workflows and config

Workflows in `~/.tome/hubs/<name>/workflows/` apply to every attached
project, as if each project had a copy:

- Names resolve project first, then the attached hubs, then global, so a
  project's own workflow wins over a hub's of the same name.
- Relative file globs resolve in each project, and a cron trigger fires once
  per project.
- An `on:` without `hub:` listens on each project's own bus.
- When two attached hubs define the same name, `tome validate`, `tome
  triggers ls` and `tome run` all report it as an error.

`~/.tome/hubs/<name>/config.yaml` takes the same keys as a project's
`.tome/config.yaml` (`bus.forward` included), layered between the global
config and the project's: a later file wins per key. `nodes` and `herdr`
stay in the global config.

## Hub and global queues

`tome queue` takes `--hub`/`--global` too, with the same attachment check. A
hub's queues are shared by the agents of every attached project:

```sh
tome queue push jobs "build web" --hub release   # from ~/src/api
tome queue pull jobs --hub release               # from ~/src/web
tome queue ls --global
```

Claims, acks and closing work as for project queues, and a claimed message
goes back on the queue when the worker that claimed it ends. Queues are never
copied to another node.

## Inspect

```sh
tome hub ls                    # every hub, and global, with attached project counts
tome hub show release          # a hub's path, attached projects and workflows
tome hub show --global
tome hub show                  # what the current project is attached to
```

## Detach and remove

```sh
tome hub detach release
tome hub detach --global
tome hub rm release            # refused while projects are attached or work is pending
tome hub rm release --force    # detach everyone and delete the hub's events, queues and files
```

Detaching drops the project's pending deliveries of that hub's events and
returns any messages it had pulled from the hub's queues but not acked, so
another project can pull them. `tome hub rm` names the attached projects and
the pending deliveries and queued messages that stop it.

All `tome hub` commands take `--on <node>` to act on another machine's hubs.
