# Single-agent runs

## Description

Today every [[run]] starts an [[orchestrator]], even when the workflow never delegates anything. That's an extra agent and an extra [[session]] for each run. This feature adds a frontmatter setting, `mode`, which says how a [[workflow]] is run:

```yaml
mode: single        # single (default) | orchestrated
```

- **`single` (the default):** tome starts one agent, in a new **`agent`** role, gives it the whole workflow, and the agent carries it out by itself. There's no orchestrator and there are no [[worker]]s.
- **`orchestrated`:** today's behaviour, unchanged. An orchestrator reads the workflow and can spawn workers, [[group]]s and [[worktree]]s.

The mode is set in the frontmatter only. There's no `tome run` flag and no default in config, because a workflow written for one agent can't safely become orchestrated, and the reverse would break delegation.

### The single agent

- **What it gets:** the whole resolved workflow body and a built-in prompt of its own, which tells it to do the work itself in its session. Placeholders (`{{params.*}}`, `{{run.id}}`, `{{trigger.*}}`) are filled in as today.
- **Steps:** it reports [[step]]s with `tome step start|done|fail`, as an orchestrator does. A failed step doesn't end the run. The agent decides whether to carry on, retry or stop.
- **Ending the run:** it must call `tome run finish --status succeeded|failed`. If it exits without finishing, the run fails with the reason `agent_exited`.
- **Commands it can use:**
  - `tome ready`, `tome step …`, `tome run finish` and `tome runs show`;
  - the project's [[queue]]s;
  - `tome publish` and `tome events`.
- **Commands it can't use:** `tome worker …`, `tome group …` and `tome worktree create` are refused with exit `2` and the hint "set `mode: orchestrated` in the workflow's frontmatter".
- **Handshake:** it calls `tome ready` first, with the same nudge and failure rules as an orchestrator (feature 007).
- **Signals:** triggers and events with `to: running` (features 004, 009) go onto the run's `events` queue and nudge the agent's session, as they nudge an orchestrator today.

### The `agent` role

- **Session:** `<run>/agent`. `tome session move <run>/agent` moves it, as for other sessions (feature 010).
- **Harness:** `defaults.harness`, because this agent does the real work. `defaults.orchestrator_harness` isn't used.
- **Placement:** an `agent` block in `defaults.layout`, in the config, or in a preset, plus the levels shared by all roles (`defaults.layout`, `TOME_LAYOUT`, project and global config, the built-in default) and `tome run`'s placement flags. It doesn't fall back to the `orchestrator` block. `from: caller` (feature 011) is allowed for it.

### Authoring and validation

- The authoring skill sets `mode: orchestrated` whenever the workflow it writes delegates work, such as spawning workers, fanning out or fanning in, and leaves it out otherwise.
- `tome validate` and the workflow load:
  - reject an unknown `mode` value (exit `2`);
  - warn when the body of a `single` workflow describes spawning workers, groups, worktrees or fan-out/fan-in;
  - warn when a `single` workflow sets something only orchestrated runs use: `defaults.orchestrator_harness`, or an `orchestrator` or `workers` placement block or rule.
- The bundled examples and docs that delegate get `mode: orchestrated`: fan-out, fan-in-worktrees, the factory examples, command-worker / test-then-fix, the chain examples, and any others whose body spawns workers.

### Visibility

- `tome runs show` displays the run's mode. The run's sessions are listed as `agent` or `orchestrator` plus workers.
- The attached `tome run` stream names the role that started.

### Out of scope

- A shared orchestrator across runs.
- The daemon deciding step order or feeding steps one at a time.
- Choosing the mode automatically from the body.
- Switching a run from `single` to `orchestrated` while it runs (escalation).
- Hidden or background sessions.

## Use Cases

1. As a workflow author, I want a workflow without delegation to run as one agent by default, so that simple runs don't cost an extra orchestrator agent and session.
2. As an author, I want to set `mode: orchestrated` for workflows that fan out or coordinate workers, so that they keep today's behaviour.
3. As an author, I want `tome validate` to warn when a `single` workflow describes spawning workers, so that I find out before a run is refused halfway through.
4. As a run's agent, I want a clear refusal and hint when I try to spawn a worker, so that the failure explains the fix.
5. As a user, I want the single agent placed through its own `agent` block, with `from: caller` allowed, so that I can control where it opens separately from orchestrators.
6. As a user, I want `tome runs show` to display the mode, and `<run>/agent` to work with `tome session move`, so that single runs are as easy to inspect and manage as orchestrated ones.
7. As a skill user, I want the authoring skill to choose the mode for me, so that I don't need to know about it to write a working workflow.

## Constraints

- `mode: orchestrated` behaves exactly as runs do today.
- The mode is resolved when the run is created and saved in its snapshot. Editing the workflow doesn't change a run that's already going or queued.
- The mode is set in the frontmatter only.
- Agents in both modes use the same step and run commands. The modes differ only in which commands are allowed and in the built-in prompt.

## Edge Cases

- **An existing workflow delegates but doesn't set `mode`** → it now runs as `single`. Its first `tome worker spawn` is refused with exit `2` and the hint. `tome validate` warns about it ahead of time.
- **Runs created before this feature**, including queued ones, have no stored mode → they're treated as `orchestrated`.
- **The single agent exits without `tome run finish`** → the run fails with `agent_exited`, and its trigger deliveries settle as `failed` (feature 009).
- **The single agent never calls `tome ready`** → the handshake nudge and failure, as for an orchestrator (feature 007).
- **A `single` workflow sets `orchestrator_harness`, or an `orchestrator` or `workers` placement block** → `tome validate` warns that they're ignored.
- **`mode` has an unknown value** → validation error, exit `2`. The workflow can't be run.
- **The config sets `orchestrator: {from: caller}` but no `agent` block** → single runs ignore it and use the shared levels.
- **`tome session move <run>/orchestrator` on a single run, or `<run>/agent` on an orchestrated run** → not found, exit `4`, with a hint naming the run's actual role.

## Related Entities

- [[run]]
- [[orchestrator]]
- [[worker]]
- [[workflow]]
- [[step]]
- [[session]]
