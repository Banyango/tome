# Recipes

Workflows to copy into `.tome/workflows/` and adapt. Each is a real file under `docs/examples/` in the repo, and CI runs `tome validate` on each one. They're written for a Rust project, so change the commands to fit yours.

| Recipe | Starts when |
| --- | --- |
| [Plan a feature, then build it](#plan-a-feature-then-build-it) | a feature file is added, then when its plan is ready |
| [Fix failing tests](#fix-failing-tests) | a source file changes |
| [Review a branch in parallel](#review-a-branch-in-parallel) | you run it |
| [Implement, then review](#implement-then-review) | a message arrives, then when the first run succeeds |
| [Split work across branches](#split-work-across-branches) | you run it |
| [A weekday-morning check](#a-weekday-morning-check) | a schedule |
| [Keep one agent on call](#keep-one-agent-on-call) | messages arrive |
| [Tidy docs as you write](#tidy-docs-as-you-write) | a doc file changes |
| [A software factory](#a-software-factory) | a request, an alert, a schedule, or another workflow |

## Plan a feature, then build it

Two workflows chained through the message bus. You add `features/<slug>/feature.md`. `plan-feature` has an agent investigate the codebase, writes `tasks.md`, and publishes `feature.planned`:

```markdown title=.tome/workflows/plan-feature.md
{{#include ../examples/plan-feature.md}}
```

`implement-feature` subscribes to that event and implements each task with its own agent on its own branch. Tasks that don't block each other run in parallel, and a blocked task branches from the task it waits for:

```markdown title=.tome/workflows/implement-feature.md
{{#include ../examples/implement-feature.md}}
```

Nothing is merged, so you review the branches and merge them yourself. To re-run just the build for a feature, publish the event by hand:

```sh
tome publish feature.planned export-csv
```

## Fix failing tests

Every time you save a Rust file, run the tests. If they fail, an agent fixes them on its own branch and leaves the branch for you to review.

```markdown title=.tome/workflows/fix-tests.md
{{#include ../examples/fix-tests.md}}
```

The fixer works in a worktree under `.tome/`, which is never watched, so its edits don't start another run. `concurrency: 1` and `while_running: queue` mean saves made during a run are collected into one follow-up run.

## Review a branch in parallel

One reviewer agent per changed file, all at once, combined into one report.

```markdown title=.tome/workflows/fan-out.md
{{#include ../examples/fan-out.md}}
```

```sh
tome run fan-out --param base=main
```

## Implement, then review

Two workflows chained through the message bus. Publishing a request starts `implement`, and every implement run that succeeds starts `review`.

```markdown title=.tome/workflows/implement.md
{{#include ../examples/chain-implement.md}}
```

```markdown title=.tome/workflows/review.md
{{#include ../examples/chain-review.md}}
```

```sh
tome publish feature.requested "Add a --verbose flag to the export command"
```

Anything that can run a shell command can publish: a git hook, CI, a script, or another agent.

## Split work across branches

Give it a list of tasks. Each gets its own agent on its own branch, and the orchestrator merges them and runs the tests.

```markdown title=.tome/workflows/fan-in-worktrees.md
{{#include ../examples/fan-in-worktrees.md}}
```

```sh
tome run fan-in-worktrees --param tasks="$(cat tasks.txt)"
```

## A weekday-morning check

A cron trigger, with a `manual` entry to show you can also run it by hand.

```markdown title=.tome/workflows/nightly.md
{{#include ../examples/nightly.md}}
```

## Keep one agent on call

One long-lived run takes work items as they arrive, instead of starting a run for each.

```markdown title=.tome/workflows/watcher.md
{{#include ../examples/signal-running.md}}
```

```sh
tome publish work.new "Rename the config loader to settings"
```

## Tidy docs as you write

A file trigger that fixes only the files that changed, so it never triggers itself.

```markdown title=.tome/workflows/doc-check.md
{{#include ../examples/watch-files.md}}
```

## A software factory

Four workflows that pass work to each other on the message bus, so a change goes from request to production with a person involved only for risky changes. It's modelled on the ["agentic software factory"](https://newsletter.pragmaticengineer.com/) that The Pragmatic Engineer described inside OpenAI.

| Workflow | Starts on | Publishes |
| --- | --- | --- |
| `factory-build` | `factory.requested` | `factory.ship` or `factory.review-needed` |
| `factory-deploy` | `factory.ship` | `alert.outage` if the rollout regresses |
| `factory-perf` | every hour, or `alert.latency` | `factory.requested` for each fix |
| `factory-sevbot` | `alert.outage` or `incident.question` | `incident.proposed`, `incident.answer`, `factory.requested` |

`factory-build` builds the change, loops until CI and four specialist reviewers pass, and sorts it into low or high risk:

```markdown title=.tome/workflows/factory-build.md
{{#include ../examples/factory-build.md}}
```

A low-risk change goes straight to `factory-deploy`. A high-risk one waits for an engineer, who reviews the branch and ships it by hand:

```sh
tome publish factory.ship <branch>
```

`factory-deploy` merges and deploys it, then rolls it out behind a flag while it watches a dashboard it made for the change:

```markdown title=.tome/workflows/factory-deploy.md
{{#include ../examples/factory-deploy.md}}
```

`factory-perf` looks for latency regressions and sends each fix back to `factory-build`, which closes the loop:

```markdown title=.tome/workflows/factory-perf.md
{{#include ../examples/factory-perf.md}}
```

`factory-sevbot` keeps one run going for an incident. More alerts and people's questions reach it on its `events` queue:

```markdown title=.tome/workflows/factory-sevbot.md
{{#include ../examples/factory-sevbot.md}}
```

```sh
tome publish factory.requested "Add a --since flag to the export command"
tome publish incident.question "Is the EU region affected?"
tome publish incident.question "Resolved: rolled back the uploader deploy"
```

Some things to know before you use it:

- **Context comes from the harness.** Slack, Notion, metrics and deploy tools are whatever MCP servers and CLIs your agents have. The `./scripts/deploy` and `./scripts/flag` commands are placeholders.
- **Alerts need a bridge.** tome doesn't receive webhooks. Point your alerting at a small script, or a cron job, that runs `tome publish alert.outage "..."`.
- **Loops stop themselves.** A rollout that regresses goes deploy → sevbot → build → deploy, and each hop adds one to the event's depth. Events deeper than 8 aren't delivered, so the factory gets about two tries at fixing its own outage before a person has to step in. Each `factory-perf` run on the hourly schedule starts again at depth 0.
- **Runs aren't timed out.** The deploy and sevbot runs say in their bodies when to stop, because tome doesn't enforce `timeout`.

## Next

[Write a workflow](guides/write-a-workflow.md) covers every frontmatter option, and [Triggers and the message bus](guides/triggers.md) covers every trigger field.
