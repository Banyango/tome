# Plugins

Folders in `~/.tome/plugins/<name>/` with a `plugin.yaml` manifest and their own
executables, installed only by the user. Plugins can add commands, hooks and node
providers.

## Files

- `src/plugin.rs`: module root and docs.
- `src/plugin/manifest.rs`: `Manifest`, `Hook`, `Version`/`VersionReq`,
  compatibility checks, name-claim rules. **Pure**: no file, process or config I/O.
- `src/plugin/store.rs`: what is installed (`installed`, `active`, `Installed`,
  `InstallState`), data/log paths, claims that collide with built-in commands.
- `src/plugin/host.rs`: the daemon's persistent plugin child processes: spawn,
  JSON-line protocol, backoff (`plugin_request`, `Log`, `check_start`).
- `src/plugin/gate.rs`: reply to the blocking `run.before_start` hook (`Verdict`,
  `Change`, `Gate`): allow/reject plus env/arg changes.
- `src/pluginhooks.rs`: the daemon's calls to hooks about runs
  (`run.before_start`, `run.started`, `run.finished`).
- `src/plugincmd.rs`: `tome plugin install|update|rm|enable|disable|ls|check`,
  `tome notify`; asks the daemon to reload after changes.

## Notes

- Plugin command names are checked against built-ins (`main.rs::builtin_commands`,
  `plugin::store::user_claims`/`collisions`).
- The host is called from `engine` around run start/finish.

## Tests

- Unit: `cargo test --bin tome plugin::` (also `plugincmd::`)
- Integration: `cargo test --test plugins`

## User docs

`docs/src/guides/plugins.md`
