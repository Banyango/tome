# Nodes

Other machines with tome installed, reached over SSH, and providers that create
machines on demand.

## Files

- `src/node.rs`: `Node` (from `nodes:` in `~/.tome/config.yaml`), resolving
  `--on` / `TOME_NODE` / `node:ref` (`resolve`, `split_ref`), `set_target`/`target`.
- `src/nodecmd.rs`: `tome node add|rm|ls|check`, `runs ls --nodes`,
  `tome session view`, `tome run --view`.
- `src/rpc/ssh.rs`: SSH transport (`ssh ... tome rpc --stdio`).
  `src/rpc.rs` holds the protocol and `Client` (see [daemon.md](daemon.md));
  `classify_ssh` maps SSH failures to errors.
- `src/provider.rs`: node providers: plugins that make a machine for
  `tome run --on <provider>`; local record of runs on released machines.
- `src/providercmd.rs`: `tome run <workflow> --on <provider>`: create, run, release.
- `src/main.rs`: `route`/`reach` decide whether a command runs locally or is
  forwarded to a node's daemon.

## Notes

- `reach` classifies each command as `Node` (can go to a node's daemon with `--on`),
  `Local` (reads local files or manages this daemon) or `Agent` (only the run it's
  in). Unlisted commands default to `Node`: add new local-only or agent commands to
  `reach`, or `--on` will try to forward them.
- Provider plugins are manifest-declared; see [plugins.md](plugins.md).

## Tests

- Unit: `cargo test --bin tome node::` (also `provider::`, `nodecmd::`, `rpc::ssh`)
- Integration: `nodes`, `providers`

## User docs

`docs/src/guides/nodes.md`
