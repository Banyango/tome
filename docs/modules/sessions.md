# Sessions

The visible terminals agents run in, where they open, and how an agent CLI is
launched in them.

## Files

- `src/session.rs`: `Kind` (cmux/tmux/herdr), `Layout`, `Backend`, `Target`;
  focus/caller detection, notify, liveness, `send_line`, `kill`.
- `src/session/herdr.rs`: herdr socket adapter. `session/workspace.rs`: per-project
  workspaces (`<project>-orchestrator`). `session/split.rs`: split direction/size/
  `from`. `session/moves.rs`: `tome session move`.
- `src/placement.rs`: pure placement rules: `Settings` (layout, split, from ...),
  `Spec`, built-in presets, and how each setting resolves from CLI flag →
  workflow → config levels (`resolve`, `moved`). `parse_settings` / `parse_value`
  read it from YAML or RPC params. Returns its own errors (`FlagError`,
  `PlacementError`); no `output` or `config` imports.
- `src/placementcmd.rs`: the CLI side: loads presets and the config `layout`
  (`presets`, `resolve`, `moved`, `check_flag_preset`, `check_presets`,
  `undefined_presets`), `tome layout presets`, and `From` conversions of
  placement errors to `CliError` (hints included).
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
