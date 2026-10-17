# Daemon

The per-user, per-machine process listening on `~/.tome/tome.sock`, and the
CLI ↔ daemon protocol.

## Files

- `src/daemon.rs`: `Daemon`, `run_foreground` (what `tome daemon run` and service
  units start), per-connection serving, streaming responses (`Sink`). Routes a
  request to `Engine` or `api`.
- `src/rpc.rs`: newline-delimited JSON-RPC 2.0 over the Unix socket. `Request`,
  `Response`, `Client`, `call`.
  `src/rpc/ssh.rs` is the SSH transport used for nodes ([nodes.md](nodes.md)).
- `src/lifecycle.rs`: `tome daemon start|stop|status`.
- `src/service.rs`: `tome daemon install|uninstall` (launchd on macOS, systemd
  `--user` on Linux).

## Notes

- The daemon holds the only DuckDB connection (`Engine` → `Store`); RPC handlers
  run with it locked.
- Adding an RPC: implement it in the feature module, list it in that module's
  `METHODS`, and route it in `Engine::handles`/`dispatch` ([runs.md](runs.md)).
- Tests isolate state with `TOME_HOME`.

## Tests

- Unit: `cargo test --bin tome rpc::` (also `service::`)
- Integration: `daemon`, `service`, `handshake`, `concurrency`

## User docs

`docs/src/concepts/daemon.md`
