# Hubs

Named, machine-local groups of projects (`~/.tome/hubs/<name>/`) and the
global scope's shared bus and queues. A project reaches a hub or global only
once it's attached.

## Files

- `src/scope.rs` (leaf): `HubName`, `Shared` (`Hub(name) | Global`), `Scope`
  (`Project(path) | Shared`), and `owner` (which daemon owns a scope; always
  this machine for now). Only the store turns a scope into its stored key
  (`hub:<name>`, `global:`), which goes in the same `project_path` columns a
  project's events and queues use.
- `src/features/hubs/mod.rs`: daemon side. Hub directories (`dir`, `exists`,
  `all`), `require_attached`, and the `hub.*` RPCs (`METHODS`), dispatched
  from `Engine::dispatch_hubs`.
- `src/features/hubs/cmd.rs`: CLI client, `tome hub ...`.
- `src/store/attachments.rs`: the `attachments` table (project path, scope key),
  and `mirror_attachments`, which rewrites `~/.tome/attachments.json`.
- `src/layers.rs` (leaf): the one loader for hub workflows and config. `hubs(root)`
  reads the mirror and gives the attached hubs' directories, so the CLI (no
  daemon) and the daemon (no RPC to itself) see the same layers. A remote hub
  would be fetched here.

## Notes

- A hub exists while its directory does; there's no hubs table.
- The attachments table is the truth. The mirror is rewritten on every
  attach/detach and whenever the store opens.
- Layers: `workflow::Library.hubs` adds `Scope::Hub` entries (checked like project
  ones), `arming::scan` arms them once per attached project (the project's own name
  wins, and two hubs with one name are a broken trigger), and `config::Config::load`
  inserts `Scope::Hub` files between global and project.
- Keep hub and global operations behind `Scope`/`Shared`: don't build
  `hub:<name>` strings outside the store. See the remote notes in
  [bus.md](bus.md).

## Tests

- Unit: `cargo test --bin tome scope::` (also `store::attachments`)
- Integration: `cargo test --test hubs`

## User docs

`docs/src/guides/hubs.md`
