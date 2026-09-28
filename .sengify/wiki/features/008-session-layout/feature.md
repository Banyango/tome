# Session layout

## Description

Today tome opens a new cmux workspace (or tmux session) for every [[orchestrator]] and every [[worker]], so a busy project fills the sidebar. A `layout:` setting chooses where new [[session]]s go.

### Layouts

| layout | cmux | tmux |
|---|---|---|
| `tab` (default) | a tab (surface) in one split pane of the project's tome workspace | a window in the `tome-<project>` session |
| `split` | its own split pane in the project's tome workspace | a pane in the `tome-<project>` session |
| `workspace` | its own workspace (today's behaviour) | its own session (today's behaviour) |

- **`tab`:** the split that holds the tabs is created on first use and reused after that. cmux removes it when its last tab closes, and it's created again the next time it's needed.
- **`split`:** new panes open to the right.

### The tome workspace

- **One per project:** a cmux workspace (or tmux session) per project, titled e.g. `tome: tome-cli` (tmux: `tome-tome-cli`). It's created on first use and reused after that.
- **Global workflows:** those that run outside a project use `tome: global`.
- **When it's empty:** the workspace stays open. A plain shell in the project directory keeps its place, so it doesn't move around in the sidebar.
- **Layouts that use it:** `tab` and `split` only.

### Configuration

- **Global setting:** `layout:` in `~/.tome/config.yaml`, next to `backend:`.
- **Workflow override:** a workflow can override it with `defaults.layout`, the same precedence as `backend`.

### Behaviour kept from today

- **Focus:** new sessions open unfocused.
- **Titles:** sessions keep their titles (`tome: <wf> #<id> / <name>`). Under `tab` they tell apart the sessions of runs going at the same time.
- **Closing:** finished sessions are closed unless they were spawned with `--keep-open`.

### Session handles

- tome tracks each session by its own pane or tab (tmux pane or window), not by the workspace it's in.
- These all target that pane or tab:
  - liveness checks;
  - kill;
  - nudges typed into the pane;
  - crash-recovery cleanup;
  - the ATTACH hint in `tome runs show`, which selects the workspace, then focuses the tab or pane.

### Out of scope

- The herdr [[backend]]
- Choosing the layout per spawn (e.g. `tome worker spawn --layout`)
- Custom split directions or sizes
- Putting sessions in the workspace you currently have focused

## Use Cases

1. As a user, I want all of a project's tome sessions in one workspace, so that my sidebar doesn't fill up with a workspace per agent.
2. As a user, I want the `tab` layout, so that the orchestrator and its workers are tabs I can flip between in one split.
3. As a user, I want the `split` layout, so that I can watch several agents side by side.
4. As a user, I want to keep the `workspace` layout, so that I can go back to one workspace per agent if I prefer.
5. As a workflow author, I want `defaults.layout` in a workflow, so that a workflow that fans out a lot can use tabs while others use splits.
6. As a tmux user, I want the same layouts mapped onto sessions, windows and panes, so that the setting means the same thing on both backends.
7. As a user, I want the tome workspace to stay in place between runs, so that I always know where to look.

## Constraints

- The default changes from `workspace` to `tab`.
- The layout is resolved in this order: workflow `defaults.layout`, then `layout:` in `~/.tome/config.yaml`, then `tab`.
- Each backend needs to create a pane or tab inside an existing workspace or session, and check, kill and type into it by its own handle. The session records gain that handle.
- Sessions still use the run's backend (feature 003). The layout only decides where inside that backend they go.

## Edge Cases

- **The user closes the tome workspace or the tab split by hand** → it's created again the next time it's needed.
- **The user closes a single tab or pane** → the existing handling applies: `orchestrator_exited` or `worker_exited`.
- **Several runs at once** → under `tab`, all their sessions share the one split, and their titles tell them apart.
- **`split` with many workers** → panes keep splitting, and the multiplexer decides how small they get. There's no cap.
- **Unknown `layout` value** → in a workflow, `tome validate` exits `2`. In `config.yaml`, the command errors with a hint, like an unknown `backend`.
- **The daemon restarts** → crash recovery kills the recorded sessions by their pane or tab handles. The tome workspace itself is left open.
- **`--keep-open`** → the finished session's tab or pane stays open, as today.
- **The project's tome workspace was renamed by the user** → tome finds it by the id it recorded, not by its title. If that id is gone, it creates a new workspace.

## Related Entities

- [[session]]
- [[backend]]
- [[orchestrator]]
- [[worker]]
- [[workflow]]
- [[daemon]]
