# Bulb Backend — Task Plan

Source feature: [feature.md](./feature.md)

## Tasks

### 024-1. Run on Bulb end-to-end

**Blocked by:** none

Make `bulb` a valid backend everywhere one is named: `defaults.backend`, `TOME_BACKEND`, `backend:` in config and `tome validate`. Auto-detect Bulb when the caller runs inside a Bulb terminal (Bulb's environment); otherwise use it only when config names it explicitly.

Tome talks to the Bulb daemon directly over its socket protocol (`open`, `session.input`, `session.read`, `session.close`, `session.list`), with no dependency on `bulb` being on PATH. If the daemon isn't running, tome starts it headless for the project and continues. Sessions open as tabs. Liveness, kill and nudges go through the protocol. A worker tab the user closes mid-step counts as process exit: workflow failure rules and the default notification apply, and the run stays resumable (017). An unreachable daemon or a protocol mismatch fails the step with a clear error naming Bulb.

Covers use cases 2, 3 and 4.

### 024-2. Placement on Bulb

**Blocked by:** 024-1

Map placement (008, 010, 011) onto Bulb's `open` relations: a tab becomes relation `tab`, and a split becomes `child`/`sibling` with `near session:<anchor>`. `from: caller` records the calling Bulb session from Bulb's environment and opens near it. Bulb's active arrangement decides the final geometry. Where Bulb can't honour a setting, tome falls back with a warning on the session, as 010 and 011 do.

Covers use case 1.

### 024-3. Bulb notifications

**Blocked by:** 024-1

On Bulb runs, tome notifications are sent through Bulb's `notify` with the same title and body, including `backend_unavailable` for triggered runs. `TOME_NOTIFY=off` still turns them off.

Covers use case 5.

### 024-4. Blocked-agent tracking from Bulb status

**Blocked by:** 024-1, 024-3

Watch the Bulb agent lifecycle status of the orchestrator and each worker. When it becomes `waiting`, the session is marked blocked, a notification is sent (with the grace period and rate limit from 015-6) and one `tome.run.<workflow>.blocked` event is published. The mark clears when the agent leaves `waiting` or the session ends. Bulb's `idle` status is never treated as step completion; tome's handshake and completion rules still apply. Without Bulb's harness hooks there is no agent status and no blocked events, and everything else still works.

Covers use case 6.

### 024-5. Local-only enforcement and docs

**Blocked by:** 024-1

Selecting Bulb for a remote worker or node session (014, 023) is rejected with an error saying the Bulb backend is local only. Remote nodes and remote workers keep using the existing backends. Add Bulb to the docs' backend table and to the sessions module docs.
