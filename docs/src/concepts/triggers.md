# Triggers and the message bus

## Triggers

A **trigger** is a rule in a workflow's frontmatter that makes the daemon react to something. The reaction is to start a new run, or to signal runs of that workflow that are already going.

There are four kinds:

| Kind | Fires when |
| --- | --- |
| `manual` | never on its own. It documents that you start the workflow by hand; `tome run` always works. |
| `file` | files matching a glob are created or modified. Changes are batched into one event. |
| `cron` | a five-field cron time arrives, in local time. Times missed while the daemon was down are skipped. |
| `on` | an event is published on a topic that matches a pattern. |

`to:` says what a trigger does: `new` (the default) starts a run, `running` signals runs that are going, and `running-or-new` signals if any run is going and starts one otherwise. A trigger can also set `params:` for the run it starts. Event details are available in the body as `{{trigger.*}}` placeholders.

tome has no loop guard. A run that changes files its own trigger watches starts another run. Use `ignore:`, `on: [created]`, or `while_running: mute` to prevent that.

## The message bus

Each project has a **message bus**. Anything can publish an **event** to a **topic**, and any workflow can subscribe to a topic with an `on:` trigger.

- A topic is dot-separated words, like `review.requested`. The `tome.` prefix is reserved.
- A subscription is a pattern. `*` matches one segment and a trailing `**` matches one or more.
- An event has text as its payload, up to about 1 MiB, and a sender: you, a run, a worker, or tome itself.
- Each subscribing workflow gets its own durable **delivery** of every matching event. A delivery is `pending`, `claimed`, `done`, `failed` or `dropped`.

Events publish with `tome publish <topic> <text>`.

tome publishes events too: `tome.run.<workflow>.started`, `.succeeded`, `.failed` and `.cancelled`, plus `.blocked` on herdr. Subscribing to these lets one workflow chain onto another.

A run that was started by an event carries its depth, one more than the event that started it. Events deeper than 8 aren't delivered, which stops two workflows from triggering each other forever.

Event subscriptions work in project workflows only.

See [Triggers and the message bus](../guides/triggers.md) for examples.
