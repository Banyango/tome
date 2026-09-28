# Session layout — Task Plan

Source feature: [feature.md](./feature.md)

## Tasks

### 008-1. Layout setting and resolution

**Blocked by:** none

A `layout:` setting (`tab`, `split` or `workspace`) resolves in this order: the workflow's `defaults.layout`, then `layout:` in `~/.tome/config.yaml`, then `tab`. That's the same precedence as `backend`. An unknown value in a workflow makes `tome validate` exit `2`. An unknown value in `config.yaml` errors with a hint, like an unknown `backend`. Each run records its resolved layout. Under `workspace`, cmux and tmux behave as they do today: one workspace or session per orchestrator and worker.

### 008-2. Sessions tracked by their own pane/tab handle

**Blocked by:** none

Session records gain the backend's handle for the session's own pane or tab (a cmux surface, a tmux pane or window), not the workspace or session it sits in. These all target that handle: liveness checks, kill, nudges typed into the pane, and crash-recovery cleanup. The ATTACH hint in `tome runs show` selects the containing workspace or session, then focuses the tab or pane. If the user closes a single tab or pane, the existing `orchestrator_exited` / `worker_exited` handling applies. Behaviour under the `workspace` layout doesn't change.

### 008-3. The project's tome workspace

**Blocked by:** 008-2

Each project gets one tome workspace: a cmux workspace titled `tome: <project>`, or a tmux session `tome-<project>`. Workflows that run outside a project use `tome: global`. The workspace is created on first use and found again by the id tome recorded, not by its title, so a rename by the user doesn't break it. If that id is gone (for example, the user closed the workspace), a new one is created. When no sessions are left, it stays open with a plain shell in the project directory, so it keeps its place in the sidebar. Crash recovery after a daemon restart kills the recorded sessions but leaves the tome workspace open.

### 008-4. `tab` layout (new default)

**Blocked by:** 008-1, 008-3

Under `tab`, the orchestrator and its workers open unfocused as tabs in one split pane of the project's tome workspace. On tmux, they open as windows in `tome-<project>`. The split is created on first use and reused after that. If cmux removes it when its last tab closes, or the user closes it by hand, it's created again the next time it's needed. When several runs are active at once, their sessions share the one split, and their titles (`tome: <wf> #<id> / <name>`) tell them apart. Finished sessions close unless they were spawned with `--keep-open`.

### 008-5. `split` layout

**Blocked by:** 008-1, 008-3

Under `split`, each orchestrator and worker session opens unfocused in its own split pane of the project's tome workspace. New panes split to the right. On tmux, each session is a pane in `tome-<project>`. There's no cap on the number of panes; the multiplexer decides how small they get. Finished sessions close unless they were spawned with `--keep-open`.
