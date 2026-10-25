# Plugins

Folders in `~/.tome/plugins/<name>/` with a `plugin.yaml` manifest and their own
executables, installed only by the user. Plugins can add commands, hooks and node
providers.

## Files

All under `src/features/plugins/`:

- `mod.rs`: module root and docs.
- `manifest.rs`: `Manifest`, `Hook`, `Version`/`VersionReq`, compatibility
  checks, name-claim rules. **Pure**: no file, process or config I/O.
- `store.rs`: what is installed (`installed`, `active`, `Installed`,
  `InstallState`), data/log paths, claims that collide with built-in commands.
- `host.rs`: the daemon's persistent plugin child processes: spawn,
  JSON-line protocol, backoff (`plugin_request`, `Log`, `check_start`).
- `gate.rs`: reply to the blocking `run.before_start` hook (`Verdict`,
  `Change`, `Gate`): allow/reject plus env/arg changes.
- `hooks.rs`: the daemon's calls to hooks about runs
  (`run.before_start`, `run.started`, `run.finished`).
- `harnesses.rs`: harness lookup (`candidates`, `resolve`, `all`) that adds the
  plugins' harnesses to the user's ones in `src/harness.rs`; a name defined twice
  is a clash.
- `library.rs`: `discover(cwd)`, the workflow `Library` plus the enabled
  plugins' workflows. `Library::discover_user` (in `workflow.rs`) is the
  user-only version and never reads the plugin store.
- `cmd.rs`: `tome plugin install|update|rm|enable|disable|ls|check`,
  `tome notify`; asks the daemon to reload after changes.

## Notes

- Plugin command names are checked against built-ins (`main.rs::builtin_commands`,
  `store::user_claims`/`collisions`).
- The host is called from `engine` around run start/finish.
- `workflow.rs` and `harness.rs` do not import plugins; code that needs plugin
  workflows or harnesses calls `library::discover` / `harnesses::resolve`.
- Still coupled to nodes: `store.rs`/`cmd.rs`/`hooks.rs` use the nodes feature,
  and `nodes::resolve_workflow_rpc` uses `library::discover`. This is a
  feature-to-feature cycle (providers are plugins; plugins list nodes).

## Tests

- Unit: `cargo test --bin tome plugins::` (also `harness::`, `provider_opts::`)
- Integration: `cargo test --test plugins`

## User docs

`docs/src/guides/plugins.md`
