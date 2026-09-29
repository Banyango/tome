# Triggers and the message bus

Triggers start a workflow without you running `tome run`, or hand work to a run that is already going. They are listed under `triggers:` in the frontmatter, and the [daemon](../concepts/daemon.md) has to be running to act on them. The examples on this page are files under `docs/examples/` that CI validates.

```yaml
triggers:
  - manual                       # documentation only: `tome run` always works
  - file: "docs/**/*.md"         # files changed
  - cron: "0 9 * * 1-5"          # a schedule
  - on: review.requested         # a message on the bus
```

Every trigger can also take:

| Field | Meaning |
| --- | --- |
| `to` | `new` (the default) starts a run. `running` signals runs that are going. `running-or-new` signals them if any exist, and starts one if not. |
| `params` | Values for the workflow's `params`. A required param must be filled here, or `tome validate` fails. |

## File triggers

```markdown title=.tome/workflows/doc-check.md
{{#include ../../examples/watch-files.md}}
```

| Field | Meaning |
| --- | --- |
| `file` | A glob. In a project workflow it is relative to the project root. In a global workflow, use an absolute path or one starting with `~/`. |
| `on` | `created`, `modified`, or both (the default). |
| `debounce` | Changes within this window, counted from the last change, become one event. The default is 2s. |
| `ignore` | Globs to skip. `.git/`, `.tome/` and gitignored paths are never watched. |
| `while_running` | What to do with changes that arrive while a run of this workflow is active: `queue` (the default) waits and starts one more run afterwards, merging later batches into it. `parallel` starts a run for each batch, subject to `concurrency`. `mute` drops them. |

In the body, `{{trigger.paths}}` lists the changed files, and `{{trigger.event}}` is the kind of event.

tome has no loop guard. A run that edits files that its own trigger watches makes another run. Avoid that with `ignore:`, with `on: [created]`, or with `while_running: mute`, or tell the workflow not to touch those files, as the example does.

## Cron triggers

```markdown title=.tome/workflows/nightly.md
{{#include ../../examples/nightly.md}}
```

A cron expression has five fields and uses local time. Times that pass while the daemon is stopped are skipped, not made up. In the body, `{{trigger.scheduled}}` is the time that was scheduled and `{{trigger.time}}` is when it fired.

## The message bus

Every project has a message bus. Anything can publish to a topic, and any project workflow can subscribe to it.

```sh
tome publish review.requested "PR 42: add retry to the uploader"
tome publish review.requested - < details.txt      # payload from stdin
tome publish review.requested "..." --dry-run      # show which workflows would get it
```

A topic is words separated by dots. A subscription is a pattern: `*` stands for one word, and `**` at the end stands for one or more. `review.*` matches `review.requested` but not `review.requested.urgent`, while `review.**` matches both.

Every workflow that subscribes to a matching topic gets its own **delivery** of the event, and it is durable. If the daemon is down when you publish, the deliveries wait, and they are picked up when the daemon starts. What a delivery does depends on `to:`:

- `new` starts one run per event. `concurrency` limits how many run at once, and the rest queue.
- `running` gives the event to the runs that are going, on their `events` queue, and nudges the orchestrator to look. If no run is going, the delivery is marked done.
- `running-or-new` does one or the other.

The event reaches a new run in the body as `{{trigger.topic}}`, `{{trigger.payload}}`, `{{trigger.event_id}}` and `{{trigger.sender}}`.

This example receives requests one at a time. `concurrency: 1` makes extra ones wait:

```markdown title=.tome/workflows/implement.md
{{#include ../../examples/chain-implement.md}}
```

### Keeping a run alive

Set `to: running-or-new` and the run gets later events on its `events` queue instead of starting more runs:

```markdown title=.tome/workflows/watcher.md
{{#include ../../examples/signal-running.md}}
```

### Chaining workflows

tome publishes an event when a run changes state: `tome.run.<workflow>.started`, `.succeeded`, `.failed` and `.cancelled`, with a JSON payload. Subscribe to one to make a workflow follow another:

```markdown title=.tome/workflows/review.md
{{#include ../../examples/chain-review.md}}
```

With `implement` above, publishing `feature.requested` starts an implement run, and every successful one starts a review.

Chains can loop by accident, so each event records its depth: 0 when you published it, and one more than the event that started the run that published it. Events deeper than 8 are refused. `tome validate` warns if a workflow subscribes to its own lifecycle events.

Subscriptions work in project workflows only, because a global workflow has no project bus. The `tome.` prefix is reserved for tome's events.

## Testing a trigger

```sh
tome triggers ls                          # armed triggers and when each last fired
tome triggers fire my-workflow --dry-run  # what would happen, with params resolved
tome triggers fire my-workflow            # fire the first trigger that isn't `manual`
tome triggers fire my-workflow --index 2  # a specific trigger (0-based)
tome triggers fire watcher --payload "hello"   # a topic trigger, with a test event
tome triggers fire doc-check --path docs/a.md  # a file trigger, with a changed path
tome triggers disable                     # pause a project's triggers
tome triggers enable
```

`--payload` sends a test event to that one workflow only. It is recorded with the sender `test`.

## Looking at the bus

```sh
tome events ls                     # topics, and each subscriber's backlog
tome events show <topic>           # events that aren't settled yet
tome events retry <event>          # hand a failed delivery out again
tome events remove <event>         # drop a pending or failed delivery
```

A delivery is `pending`, `claimed`, `done`, `failed` or `dropped`. `tome gc` clears events once all their deliveries are settled.
