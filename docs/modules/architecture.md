# Architecture map

A map for agents and new contributors: where each feature lives today, so a
change can start from one feature doc instead of a crate-wide search. These
docs describe the **current flat `src/` layout**; update them as modules move.

tome is one binary crate. A CLI process parses a command (`src/main.rs`,
`dispatch`) and either does local work or sends a JSON-RPC request to the
daemon (`src/rpc.rs`). The daemon owns the only DuckDB connection, so every
read or write of run state goes through it.

## Features

| Feature | Doc | Owns |
|---|---|---|
| Workflows | [workflows.md](workflows.md) | Parsing/validating workflow files, `tome workflow`, `tome validate` |
| Triggers | [triggers.md](triggers.md) | Cron, file, topic and manual firing; arming; file watching |
| Runs | [runs.md](runs.md) | Run lifecycle, steps, orchestrator launch, resume, recovery, handshake, gc, inspection |
| Workers | [workers.md](workers.md) | `tome worker/group/worktree/queue`, worker monitor, queues |
| Sessions | [sessions.md](sessions.md) | Terminals (cmux/tmux/herdr), placement, harnesses, git worktrees |
| Bus | [bus.md](bus.md) | Project message bus: publish, deliveries, forwarding, lifecycle events |
| Nodes | [nodes.md](nodes.md) | Remote machines over SSH, node providers |
| Plugins | [plugins.md](plugins.md) | Plugin install/manage, hook host, `run.before_start` gate |
| Daemon | [daemon.md](daemon.md) | Daemon process, RPC protocol, service install, lifecycle |

## Shared code (not a feature)

- `src/main.rs`: clap `Command` enum, `dispatch`, node routing (`route`, `reach`).
- `src/output.rs`: `CliError`, `CliResult`, `Report`, `Mode`, `table`. The output
  contract every command follows (text by default, `--json` for stable JSON).
- Leaf helpers that depend on nothing else in the crate, so any feature may use
  them: `src/ids.rs` (`RunId` and friends, `run_sender`), `src/paths.rs`
  (everything under `~/.tome`, overridable with `TOME_HOME`; `same_path`),
  `src/role.rs` (`Mode`, the `orchestrator`/`agent` roles, `is_main`),
  `src/text.rs` (`is_identifier`, `shell_quote`), `src/params.rs` (`req_*`/`opt_*`
  readers for RPC params, `internal`).
- `src/config.rs`: global and project `config.yaml`.

## Known coupling to be aware of

These are why a change sometimes spreads wider than its feature:

- `output::{CliError, CliResult}` is used by almost every module, including
  domain code (`store`). Domain code returns presentation errors.
  `workflow.rs` and `placement.rs` are already clean: they return their own
  errors and `workflowcmd.rs` / `placementcmd.rs` convert them. Follow that
  pattern for the others.
- Module cycles. `store`, `config`, `placement` and `session` are now below the
  features (no imports back up). What is left is two clusters; list them with
  a graph of `crate::` imports before assuming a new edge is safe:
  - Run engine: `api`, `arming`, `bus`, `engine`, `handshake`, `orchestrator`,
    `resume`, `triggers`, `watch`, `workers`. Mostly because feature files
    `impl Engine` (the hub) and the hub calls back into them.
  - Remote and plugins: `node`, `rpc`, `lifecycle`, `service`, `provider`,
    `triggerscmd` (one feature, so its internal cycle is fine) plus the edges
    that tie it to others: `workflow` → `plugin`/`provider` (`Library` reads the
    plugin store, `provider_opts` parsing), `harness` ↔ `plugin` (plugin-provided
    harnesses), `plugin` → `node`/`workflow`.
  When adding code, import downwards: leaf helpers, then `store`/`config`/
  `placement`/`session`, then features.
- `engine.rs` is the daemon's hub: it dispatches RPCs for runs, workers,
  triggers and the bus (`Engine::dispatch`).
- `store.rs` holds the whole schema (all `CREATE TABLE`s and migrations) and the
  run/step/session types; `store/*.rs` hold the per-feature queries.

## Working conventions

- Tests: unit tests are inline (`#[cfg(test)]`); run one module with
  `cargo test --bin tome <module>::`. Integration tests are `tests/<name>.rs`
  (run one with `cargo test --test <name>`), driving the real binary and a
  private tmux server through `tests/common`. cmux tests skip when cmux is
  unreachable or `TOME_SKIP_CMUX` is set.
- Run targeted tests first, then `make test`.
- User-facing behavior or flag changes also update `docs/src` (the published
  mdBook). These module docs are for contributors and are not published.
