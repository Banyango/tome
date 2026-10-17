# Sessions

The visible terminals agents run in, where they open, and how an agent CLI is
launched in them.

## Files

- `src/session.rs`: `Kind` (cmux/tmux/herdr), `Layout`, `Backend`, `Target`;
  focus/caller detection, notify, liveness, `send_line`, `kill`.
- `src/session/herdr.rs`: herdr socket adapter. `session/workspace.rs`: per-project
  workspaces (`<project>-orchestrator`). `session/split.rs`: split direction/size/
  `from`. `session/moves.rs`: `tome session move`.
- `src/placement.rs`: placement `Settings` (layout, split, from ...), presets, and
  how each setting resolves from CLI flag → workflow → config levels.
  `parse_settings` / `parse_value` read it from YAML or RPC params.
- `src/harness.rs`: harness adapter: command templates for launching an agent CLI;
  built-in presets plus user-defined (`config.yaml`). `resolve`.
- `src/harnesscmd.rs`: `tome harness ls|validate`.

## Notes

- `placement` and `session` import each other (placement needs `Layout`/`Kind`,
  session needs placement settings). Keep new types out of the cycle where possible.
- Backends are `Backend::{Tmux, Cmux, Herdr}`; `TOME_BACKEND` selects one, unset
  means auto. Workspaces are cmux workspaces or tmux sessions
  (`session/workspace.rs`); cmux and herdr sessions open as tabs.

## Tests

- Unit: `cargo test --bin tome placement::` (also `session::`, `harness::`)
- Integration: `layout` (tmux), `cmux` (needs cmux; skip with `TOME_SKIP_CMUX`)

## User docs

`docs/src/concepts/backends.md`, `docs/src/guides/sessions.md`
