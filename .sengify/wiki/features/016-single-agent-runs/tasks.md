# Single-agent runs — Task Plan

Source feature: [feature.md](./feature.md)

The feature's behaviour is already on main (see the 2026-10-02 21:00 entry in `log.md`). What's left is making the test suite trustworthy and covering the parts of the feature that no test exercises yet.

## Tasks

### 016-1. Keep tests apart from the caller's tome environment

**Blocked by:** none

The integration tests inherit `TOME_*` variables (`TOME_RUN_ID`, `TOME_WORKER_ID`, `TOME_HOME`, `TOME_OUTPUT`) from the shell that runs them. When the suite is run from inside a tome session, such as an agent or worker that tome started, about 28 tests across the bus, cmux, handshake, layout and workers suites fail for reasons unrelated to the code. For example, `tome worker spawn` is refused because the test thinks it's a worker of the outer run. The shared test environment should clear inherited `TOME_*` variables before setting its own, so the suite gives the same result inside or outside a tome session. The multiplexer variables that the cmux tests rely on stay as they are.

### 016-2. Test the rest of single-agent runs

**Blocked by:** 016-1

Add end-to-end tests for the single-run behaviour that's implemented but untested:

- **Placement:** the `agent` block from the workflow, config and presets places the agent's session. `from: caller` works for it. A config that has only an `orchestrator` block doesn't affect single runs.
- **Session management:** `tome session move <run>/agent` moves a single run's session. `<run>/orchestrator` on a single run, and `<run>/agent` on an orchestrated run, are not found (exit `4`) with a hint naming the run's actual role.
- **Handshake:** an agent that never calls `tome ready` is nudged, then fails, as an orchestrator does.
- **Signals:** triggers and events with `to: running` go onto the run's `events` queue and nudge the agent's session.
- **Exit:** when the agent exits without `tome run finish`, the run fails with `agent_exited` and its trigger deliveries settle as `failed`.
- **Visibility:** the attached `tome run` stream names `agent` as the role that started, and `tome runs show` lists the `agent` session.
