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
tome hub rm release            # refused while any project is attached
```

All `tome hub` commands take `--on <node>` to act on another machine's hubs.
