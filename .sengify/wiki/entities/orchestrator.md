# Orchestrator

## Definition

The agent that carries out a [[run]]. The [[daemon]] launches it in its own visible [[session]], and it reads the [[workflow]]'s natural-English body and drives it live by calling tome commands.

## Attributes

- harness: `defaults.harness`, or `defaults.orchestrator_harness` if set (via the [[harness-adapter]])
- bootstrap: a built-in prompt (the tome command reference and its duties), the resolved workflow body, any extra orchestrator instructions from the workflow, and the env vars `TOME_RUN_ID` and `TOME_OUTPUT=json`
- duties: decide order, branching and looping; spawn workers; report [[step]] progress with `tome step start|done|fail`; end the run with `tome run finish --status succeeded|failed`
- exits without finishing: the run fails with reason `orchestrator_exited`
- placement: `tome run` placement flags, above the workflow's `orchestrator` block; its session can be re-placed with `tome session move <run>/orchestrator` _(source: [010](../features/010-session-placement/feature.md))_
- `from: caller`: opens as a tab in, or a split off, the cmux pane that ran `tome run`, in its workspace, without taking focus. It falls back with a warning off cmux or when the pane is gone. `tome session move <run>/orchestrator --from caller` moves it next to the pane running the move _(source: [011](../features/011-caller-placement/feature.md))_

## Relationships

- [[run]]: one orchestrator per run
- [[step]]: carries out steps and reports them
- [[action]]: performs actions using tome primitives
- [[session]]: runs in its own session, which the user can watch and type into
- [[daemon]]: launched by the daemon

## Sources

- features/002-running-a-workflow/feature.md
- features/010-session-placement/feature.md
- features/011-caller-placement/feature.md
