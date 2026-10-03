# herdr backend — Task Plan

Source feature: [feature.md](./feature.md)

## Tasks

### 015-1. Run on herdr end-to-end

**Blocked by:** none

Make `herdr` a valid backend everywhere one is named: `defaults.backend`, `TOME_BACKEND`, `backend:` in project or global config, `tome validate`, and the docs' backend table. When nothing names a backend, pick cmux if the daemon is inside cmux, otherwise herdr if the daemon is inside herdr or `tome run` came from a herdr pane (`HERDR_ENV=1`), otherwise tmux.

Find the herdr server in this order: `herdr.session`, then `HERDR_SOCKET_PATH`, then `HERDR_SESSION`, then the default socket, so a daemon started at login can reach it. If no server is reachable, or herdr isn't installed, the run fails with `backend_unavailable` and a hint.

Sessions open as tabs in the `<project>-orchestrator` workspace, which tome finds by label and creates on first use. Each session is started from the launcher argv, without taking focus, and its pane is labelled with the session name. The handle is the pane id, recorded with the socket path. Liveness means the pane exists and is still running the launcher. Kill uses `pane.close`. Nudges use `pane.send_text` followed by enter.

On daemon start, leftover panes from `daemon_restart` runs are found by label and closed. Panes restored by a herdr restart count as gone and are left open. If the herdr server stops during a run, its sessions fail through the session-exit path. Viewing a node's herdr session (014) is refused with a `herdr machine add` hint. Integration tests run against their own `HERDR_SESSION=tome-test-<pid>` and are skipped when herdr isn't installed.

Covers use cases 1, 2 and 7.

### 015-2. Full placement on herdr

**Blocked by:** 015-1

Every placement setting from 008 and 010 works on herdr:

- `workspace`: project, focused (recorded on the run), named (`<project>-<name>`) and own.
- `layout`: tab, and split through `pane.split` from the `from` anchor.
- `split.direction`: right and down are native; left and up split right or down and then swap the panes.
- `split.size`: a percentage becomes herdr's ratio; a size in cells is converted using the anchor pane's size.
- `from`: orchestrator, first and last anchor on the session's own pane id.

Everything opens with `focus: false`. Where herdr can't do something, tome falls back with a warning on the session, following 010: duplicate tome labels use the first workspace herdr lists, a failed swap leaves the split where herdr put it, and a cell size that can't be read is skipped. A workspace the user closed or relabelled is created again when next needed. tmux and cmux behave exactly as before.

Covers use case 3.

### 015-3. `from: caller` on herdr

**Blocked by:** 015-2

Record the calling herdr pane and workspace on the run, from `HERDR_PANE_ID` and `HERDR_WORKSPACE_ID`, and open the orchestrator next to that pane. It stays orchestrator only. 011's fallbacks and warnings apply when the caller wasn't in a herdr pane or the run's backend isn't herdr, with the warning naming herdr or cmux as appropriate.

Covers use case 4.

### 015-4. `tome session move` on herdr

**Blocked by:** 015-2

Re-place a live herdr session with herdr's move, swap and resize methods, without restarting the agent. A move herdr can't make fails, and the session stays where it was, as with 010's other failed moves.

Covers use case 8.

### 015-5. herdr notifications

**Blocked by:** 015-1

On herdr runs, notifications that go to cmux today go to herdr's `notification.show` instead, with the same title and body. This includes `backend_unavailable` for triggered runs. `TOME_NOTIFY=off` still turns them off.

### 015-6. Blocked-agent tracking

**Blocked by:** 015-1, 015-5

On herdr, watch the agent status of the orchestrator and each worker.

- **Becomes `blocked`:** the session records `blocked` with the time, shown by `tome runs show` and reported by `tome worker status`.
- **Notification:** e.g. "run 12: worker lint is waiting for input", sent after a 5-second grace period and at most once per session per minute. Every change is still recorded.
- **Lifecycle event:** each time a session becomes blocked, publish one `tome.run.<workflow>.blocked` event whose payload is the run id, the session and the time.
- **Clearing:** the mark clears when the agent leaves `blocked`, or when the session is killed or finishes; nothing more is sent after that.

Agents herdr doesn't recognise never become blocked. Run and step state never change.

Covers use cases 5 and 6.
