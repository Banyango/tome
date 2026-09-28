# Session layout — Task Plan

Source feature: [feature.md](./feature.md)

> All tasks are done: implemented as 008-1 to 008-5 directly on main (e35f2f4, ac41290, 754a335, 0d709a9, 9b9ed69).

## What changed

- **Setting:** `layout: tab | split | workspace` resolves from the workflow's `defaults.layout`, then `TOME_LAYOUT`, then `layout:` in `~/.tome/config.yaml`, then `tab`. `TOME_LAYOUT` isn't in the feature spec: it's an env override that matches `TOME_BACKEND`, and the daemon passes it through to agents. An unknown value in a workflow makes `tome validate` exit `2`. An unknown value in the config errors with a hint.
- **Session records:** sessions gain `pane` (tmux pane id `%N`, cmux surface id) and `layout` columns (store migrations 6 and 7). Under `tab`/`split`, `handle` is the tome workspace's id (cmux workspace id, tmux session id `$N`). Liveness checks, kill, nudges and crash recovery all go by `pane`. Records from before this feature have no layout and are treated as `workspace`.
- **ATTACH hint:**
  - cmux: `cmux select-workspace --workspace W && cmux focus-panel --panel S --workspace W`
  - tmux: `tmux select-window -t %N \; select-pane -t %N \; attach -t '$M'`
- **Tome workspace:** its id is recorded in `~/.tome/workspaces.json`, one entry per backend, tmux server and project. On cmux, the entry also records the pane that holds the `tab` layout's tabs. On tmux, the session is also tagged with the `@tome-workspace` option. That way a recycled `$N` id isn't mistaken for it, and `kill_prefix` never kills it, even when the project is named like `tome-7-x`. Within the daemon, a lock keeps launches that happen at the same time from each creating a workspace.
- **`tab`:**
  - tmux: `new-window -d` in the tome session.
  - cmux: `new-surface` in the recorded tabs pane. When that pane is missing, `new-split right` off the rightmost pane first, and its pane is recorded.
- **`split`:**
  - tmux: `split-window -d -h -f` on the first window, then `select-layout even-horizontal`, with the pane title set to the session title.
  - cmux: `new-split right` off the rightmost pane (cmux lists panes left to right).
- **cmux launch:** new tabs and splits can't be given a command. So tome renames the tab to the session's title, then types ` exec sh <script>` into the new tab and presses Enter. The leading space keeps it out of shell history, and `exec` makes the tab close when the agent exits.
- **Launcher script:** it now `cd`s into the session's directory first. That's needed because a new cmux tab starts in the workspace's directory, not the worker's worktree.
- **Tests:** `tests/common` sets `TOME_LAYOUT=workspace` by default, so older tests keep per-session names. `tests/layout.rs` (tmux) and `tests/cmux.rs` (tab and split) unset it. The cmux test env's drop now also closes the tome workspaces recorded in `workspaces.json`.

## Potential follow-ups

- **Kept-open sessions after a run finishes:** finishing a run doesn't close its sessions. The orchestrator exits by itself, and cancel or crash recovery kill the rest. So a `--keep-open` worker's tab or pane stays until the user closes it. Under `tab`/`split` these pile up in the shared workspace. Consider closing them when the run finishes, or giving the user a command to clear them.
- **Typed launch on cmux:** the launch depends on typing into a freshly opened shell. Timing was fine in testing, but a slow shell startup (for example a heavy zshrc, or a prompt that asks something) could swallow or garble the typed command. The session would then sit there until the start handshake (007) fails it. Consider switching to a real command argument if cmux adds one for `new-surface`/`new-split`.
- **tmux pane limit under `split`:** tmux refuses a new split when the window runs out of room ("no space for new pane"). The launch then fails with an error rather than the multiplexer shrinking panes further. The spec says there's no cap, so consider falling back to a new window, or documenting the limit.
- **Commit split:** the `split` code landed in the 008-4 commit because it shares functions with `tab`. The 008-5 commit adds only its tests. That's worth knowing when reading history or bisecting.
- **Stale records in `workspaces.json`:** entries are replaced when a workspace is recreated, but never removed. For example, entries for deleted projects or old tmux sockets from tests stay. The file is small, but `tome gc` could prune them.
- **Lock is in-process only:** the lock only covers launches inside one daemon. Two tome homes that share a project on the same cmux each get their own workspace, which is intended. But nothing stops two processes under one tome home from racing to create it.
- **Flaky unit test:** `session::tests::dead_pane_with_remain_on_exit_is_not_alive` failed once in a full `cargo test` and passed when rerun. It looks timing-sensitive under load; consider giving it a longer wait or making it more robust.

## Tasks

### 008-1. Layout setting and resolution

**Blocked by:** none

A `layout:` setting (`tab`, `split` or `workspace`) resolves in this order: the workflow's `defaults.layout`, then `layout:` in `~/.tome/config.yaml`, then `tab`. That's the same precedence as `backend`. An unknown value in a workflow makes `tome validate` exit `2`. An unknown value in `config.yaml` errors with a hint, like an unknown `backend`. Each run records its resolved layout. Under `workspace`, cmux and tmux behave as they do today: one workspace or session per orchestrator and worker.

### 008-2. Sessions tracked by their own pane/tab handle

**Blocked by:** none

Session records gain the backend's handle for the session's own pane or tab (a cmux surface, a tmux pane or window), not the workspace or session it sits in. These all target that handle: liveness checks, kill, nudges typed into the pane, and crash-recovery cleanup. The ATTACH hint in `tome runs show` selects the containing workspace or session, then focuses the tab or pane. If the user closes a single tab or pane, the existing `orchestrator_exited` / `worker_exited` handling applies. Behaviour under the `workspace` layout doesn't change.

### 008-3. The project's tome workspace

**Blocked by:** 008-2

Each project gets one tome workspace: a cmux workspace or tmux session named `<project>-orchestrator`. Workflows that run outside a project use `global-orchestrator`. The workspace is created on first use and found again by the id tome recorded, not by its title, so a rename by the user doesn't break it. If that id is gone (for example, the user closed the workspace), a new one is created. When no sessions are left, it stays open with a plain shell in the project directory, so it keeps its place in the sidebar. Crash recovery after a daemon restart kills the recorded sessions but leaves the tome workspace open.

### 008-4. `tab` layout (new default)

**Blocked by:** 008-1, 008-3

Under `tab`, the orchestrator and its workers open unfocused as tabs in one split pane of the project's tome workspace. On tmux, they open as windows in `<project>-orchestrator`. The split is created on first use and reused after that. If cmux removes it when its last tab closes, or the user closes it by hand, it's created again the next time it's needed. When several runs are active at once, their sessions share the one split, and their titles (`tome: <wf> #<id> / <name>`) tell them apart. Finished sessions close unless they were spawned with `--keep-open`.

### 008-5. `split` layout

**Blocked by:** 008-1, 008-3

Under `split`, each orchestrator and worker session opens unfocused in its own split pane of the project's tome workspace. New panes split to the right. On tmux, each session is a pane in `<project>-orchestrator`. There's no cap on the number of panes; the multiplexer decides how small they get. Finished sessions close unless they were spawned with `--keep-open`.
