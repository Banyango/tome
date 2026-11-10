# Bulb Backend

## Description

Bulb (`../getbulb`), the UI primitive application for agentic workflow GUIs, becomes tome's fourth terminal [[backend]] alongside cmux, tmux and herdr. Workers run in Bulb sessions. Tome talks to the Bulb daemon directly over its socket protocol rather than shelling out to the `bulb` CLI. Placement maps onto Bulb's `open` relations, tome notifications become Bulb notifications, and Bulb's agent lifecycle status `waiting` becomes a tome blocked signal (as with herdr). Local only.

## Use Cases

1. As a human working in Bulb, I want workflow workers to open as Bulb terminals near my session, so that I can watch and step into runs inside my Bulb workspace.
2. As an agent running in a Bulb terminal, I want tome to auto-detect Bulb, so that workers I spawn land in Bulb without configuration.
3. As a human, I want to set `backend: bulb` in config, so that tome uses Bulb even when started outside a Bulb terminal.
4. As a human, I want tome to start the Bulb daemon headless when it isn't running, so that runs proceed and I can open Bulb later to see them.
5. As a human, I want tome notifications delivered as Bulb notifications, so that I see them where I work.
6. As a human, I want a Bulb agent reporting `waiting` to raise a tome notification and a `tome.run.<workflow>.blocked` event, so that I know a worker needs me.

## Constraints

- Transport is the Bulb daemon socket protocol (`open`, `session.input`, `session.read`, `session.close`, `session.list`, `notify`); no dependency on `bulb` being on PATH, but tome is coupled to that protocol.
- Placement: tab → `open` relation `tab`; split → `child`/`sibling` with `near session:<caller>`. Bulb's active arrangement decides final geometry.
- Local only: remote nodes and remote workers (014, 023) keep using existing backends.
- Selection: auto-detect when the caller runs inside a Bulb terminal (Bulb's environment); otherwise only via explicit `backend: bulb` config.
- Bulb `idle` status is not a step-completion signal; tome's handshake and completion rules still apply.

## Edge Cases

- Bulb daemon not running → start it headless for the project and continue.
- User closes a worker's Bulb tab mid-step → treated as process exit: workflow failure rules apply, default notification, resumable via 017.
- Daemon unreachable or protocol mismatch → step fails with a clear error naming Bulb.
- Bulb selected for a remote worker → rejected with an error saying the Bulb backend is local only.
- Bulb harness hooks not installed → no agent status and no blocked events; everything else works.

## Related Entities

- [[backend]]
- [[session]]
- [[worker]]
- [[notification]]
- [[event]]
- [[daemon]]
