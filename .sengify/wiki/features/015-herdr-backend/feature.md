# herdr backend

## Description

tome opens [[session]]s on two [[backend]]s, tmux and cmux. The original plan named a third, herdr (<https://herdr.dev/docs/>), and 010 left it out of scope. This feature adds herdr as a third backend. Every layout, placement setting, session move and notification that works on tmux and cmux should mean the same thing on herdr.

herdr suits tome well:

- **Runs in the background.** It keeps a server running even when no client window is attached, so the daemon can open panes without a window open. cmux can't do this, and tmux needs its own server.
- **Socket API.** herdr is controlled through newline-delimited JSON over a local socket, the same wire format as tome's own RPC. The `herdr` CLI is also available.
- **Stable pane ids.** Panes have stable ids (`w1:p1`), and a new pane can start a command directly from an argv, with its own `env` and with `focus: false`.
- **Agent status.** herdr knows when an agent in a pane is `idle`, `working` or `blocked` (waiting on an approval or a question). tome can use this to tell you when a worker is stuck.

### Choosing herdr

- `herdr` becomes a valid value everywhere a backend is named: `defaults.backend`, `TOME_BACKEND`, and `backend:` in the project or global config.
- **Default backend order:** workflow, then `TOME_BACKEND`, then config, as today. When none of them sets one, the order is:
  1. cmux, if the daemon runs inside cmux;
  2. otherwise herdr, if the daemon runs inside herdr or `tome run` was called from a herdr pane (`HERDR_ENV=1`);
  3. otherwise tmux.
  
  Triggered runs have no caller, so herdr users who want triggered runs on herdr set `backend: herdr` in config.
- **Which herdr server:** the first match wins:
  1. `herdr.session: <name>` in config;
  2. `HERDR_SOCKET_PATH`;
  3. `HERDR_SESSION` in the daemon's environment;
  4. herdr's default session socket (`~/.config/herdr/herdr.sock`).

  A daemon started at login (launchd or systemd) can reach the default socket, so herdr doesn't have cmux's rule that the daemon must start inside the app.
- **No server:** if no herdr server is reachable when a run needs one, the run fails to start with the reason `backend_unavailable`. The hint is to start herdr (`herdr`) or set `herdr.session`. tome doesn't start a herdr server itself.

### Layouts and placement

Placement settings (008, 010) map onto herdr's workspaces, tabs and panes. tome-owned workspaces get herdr labels, and tome finds them again by label:

| setting | herdr |
|---|---|
| `workspace: project` | the workspace labelled `<project>-orchestrator`, with the project as its `cwd`. Created on first use and reused after that. |
| `workspace: focused` | the workspace focused when the run starts, recorded on the run as on other backends |
| `workspace: <name>` | the workspace labelled `<project>-<name>` |
| `workspace: own` | a new workspace for each session |
| `layout: tab` | a new herdr tab in the target workspace. herdr's tabs belong to the workspace, as tmux windows belong to a session, so `from` is ignored for tabs, as on tmux. |
| `layout: split` | `pane.split` from the `from` anchor pane, in `split.direction` |
| `split.size` | a percentage becomes herdr's `ratio`. A size in cells is converted using the anchor pane's size. If tome can't read that size, the split is still made, the size is skipped, and a warning is recorded (010's rule). |
| `split.direction` | `right` and `down` are native. If herdr's API has no `left` or `up`, tome splits `right` or `down` and swaps the two panes, which looks the same. |
| `from: orchestrator / first / last` | the session's own pane id, as on cmux |

- **Focus:** every workspace, tab and pane tome creates opens with `focus: false`.
- **Labels:** each pane tome creates is renamed to its session name, e.g. `run-12/orchestrator` or `run-12/lint`. That name shows in herdr's agent list, and recovery uses it to find leftover panes.
- **`from: caller` (011):** now works on herdr too. The caller is the herdr pane that ran `tome run`, read from its `HERDR_PANE_ID` and `HERDR_WORKSPACE_ID`, and recorded on the run. It's still orchestrator only, with the same fallbacks and warnings as on cmux.
- **`tome session move` (010):** works on herdr through herdr's move, swap and resize methods. A move herdr can't make fails, and the session stays where it was, as with 010's other failed moves. The agent isn't restarted.

### Running sessions

- **Launching:** herdr starts the session's launcher script directly from an argv. It isn't typed into a shell. The launcher sets the agent's environment, captures output to the step log and `exec`s the agent, as on other backends. The session ends when the agent does.
- **Handle:** the session's handle (008) is its herdr pane id. It's recorded with the herdr socket path, so liveness checks, kills and nudges always go to the right server.
- **Liveness:** a session is alive while its pane exists and is still running the launcher's process. tome checks both through herdr's API, and it may subscribe to `pane.exited` and `pane.closed` so that it notices an exit straight away instead of at the next poll.
- **Kill:** `pane.close`. A tome workspace stays open after its last session closes, as on cmux.
- **Nudges** (signals and `tome worker` messages typed into an orchestrator, feature 004) use `pane.send_text` followed by an `enter` key.
- **Notifications** that tome sends as cmux notifications today go to herdr's `notification.show` on herdr runs, with the same title and body. `TOME_NOTIFY=off` turns them off.

### Blocked agents

When a session's backend is herdr, tome watches herdr's agent status for the orchestrator and each worker:

- **When an agent becomes `blocked`:** the session records `blocked` with the time. `tome runs show` shows it on the session, and `tome worker status` reports it. tome then sends a notification, e.g. "run 12: worker lint is waiting for input", after a short grace period of 5 seconds, so that brief prompts don't cause noise.
- **When the agent leaves `blocked`:** the mark is cleared.
- **Lifecycle event:** a `tome.run.<workflow>.blocked` lifecycle event (feature 009) is published once each time a session becomes blocked. Its payload is the run id, the session and the time. This lets a workflow react, for example by notifying a chat.
- **Step state:** blocked isn't a step state, and it doesn't fail or pause anything. It's only information.
- **Agents herdr doesn't recognise** never become blocked. tome's behaviour for them is the same as on tmux.

### Recovery

- On daemon start, sessions of runs failed with `daemon_restart` (feature 002) are found by their pane label and closed, as on other backends.
- If the herdr server itself restarted, a pane may come back from herdr's snapshot restore as a plain shell, or its agent may be resumed by herdr's native agent restore. Either way it isn't tome's launcher process any more, so tome treats the session as gone, and the step fails through the existing session-exit path. tome doesn't close the restored pane, so the user can look at it.

### Testing

- Integration tests run against a herdr session of their own (`HERDR_SESSION=tome-test-<pid>`), as tmux tests use `TOME_TMUX_SOCKET`. They're skipped when `herdr` isn't installed.

### Out of scope

- herdr's own worktree commands. tome keeps managing worktrees itself (feature 003).
- herdr's remote machines (`herdr machine add`). Remote runs come from 014. On a node, herdr behaves as on any machine. Viewing a node's herdr session from this machine (014's `tome session view`) isn't supported yet, and is refused with a hint to add the node to herdr with `herdr machine add`.
- herdr plugins (for example, a tome panel inside herdr).
- Starting a herdr server automatically.
- Using herdr's agent status for anything except information, such as auto-approving or failing steps.

## Use Cases

1. As a herdr user, I want to set `backend: herdr`, so that my runs' orchestrators and workers open in the multiplexer I already use.
2. As a user, I want `tome run` from a herdr pane to pick herdr by itself, so that it works without any setup.
3. As a user, I want the same `layout`, `workspace`, `split` and `from` settings to work on herdr as on tmux and cmux, so that a workflow's layout doesn't depend on the backend.
4. As a user, I want `from: caller` to open the orchestrator next to my herdr pane, so that I can start runs from a chat agent in herdr and watch them beside it.
5. As a user, I want a notification when a worker is blocked on an approval or a question, so that a run doesn't sit waiting without my knowing.
6. As an author, I want a `tome.run.<workflow>.blocked` event, so that a workflow can react to stuck agents.
7. As a user, I want tome's daemon to reach herdr even when started at login, so that triggered runs open in herdr without my starting the daemon from a herdr pane.
8. As a user, I want `tome session move` to work on herdr, so that I can rearrange running agents without restarting them.

## Constraints

- herdr is reached through its socket API. Running the `herdr` CLI is acceptable for operations the API doesn't cover. The exact parameters each method takes should be checked against `herdr api schema --json` while implementing.
- Every placement setting keeps its meaning across backends. Where herdr can't do something, tome falls back with a warning on the session, as 010 does for tmux and cmux. It never errors unless the pane can't be created.
- tmux and cmux behave exactly as before.
- The blocked-agent feature only adds information and notifications. It never changes run or step state.
- `tome validate` accepts `herdr` as a backend, and the docs' backend table lists it.

## Edge Cases

- **No herdr server is reachable when a run starts** → the run fails with `backend_unavailable` and a hint. For a triggered run, this is also notified through the existing channel.
- **The herdr server stops during a run** → every session on it is gone. The steps fail through the session-exit path, and the orchestrator's exit fails the run with `orchestrator_exited`, as with a crashed tmux server.
- **The user closes the `<project>-orchestrator` workspace or relabels it** → it's created again the next time it's needed.
- **Two workspaces have the same tome label** (for example, the user made one) → tome uses the first one herdr lists and records a warning.
- **`split.direction: left/up` and the swap fails** → the split stays where herdr put it (right or down), with a warning.
- **Split size in cells and the pane's size can't be read** → the size is skipped, with a warning.
- **`from: caller` but the caller wasn't in a herdr pane, or the run's backend isn't herdr** → the fallback and warning from 011, with the warning naming herdr or cmux as appropriate.
- **An agent is blocked and the worker is killed or finishes** → the mark is cleared, and nothing more is sent.
- **An agent keeps going blocked and unblocked** → at most one notification per session per minute. Every change is still recorded.
- **herdr's restore brings back a tome pane after a herdr restart** → treated as gone (see Recovery). The pane is left open.
- **`TOME_BACKEND=herdr` but herdr isn't installed** → the run fails with `backend_unavailable` and a hint to install herdr.

## Related Entities

- [[backend]]
- [[session]]
- [[notification]]
- [[event]]
- [[run]]
- [[worker]]
- [[orchestrator]]
