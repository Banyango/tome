# tome

Rust CLI and daemon for repeatable agentic workflows (Markdown workflows started by file changes, cron, messages or other workflows). State lives in DuckDB; workers run in cmux, tmux or herdr panes. Docs are an mdBook in `docs/`; agent plugins live in `plugin/`.

## Commands

- `make build` / `make test` / `make fmt`: debug build, `cargo test`, `cargo fmt`.
- `make release`: release binary with bundled DuckDB (slow).
- `make docs`: serve the docs (needs mdbook 0.5). Update `docs/src` when user-facing behavior or flags change.
- Integration tests are in `tests/`; shared helpers in `tests/common`.

## Releasing

1. Pick the version (minor for features, patch for fixes) and bump it everywhere:
   - `Cargo.toml` and `Cargo.lock` (`cargo update -p tome --offline`)
   - `plugin/.claude-plugin/plugin.json`
   - `plugin/.codex-plugin/plugin.json`
   - `plugin/.cursor-plugin/plugin.json`
   - `plugin/.kimi-plugin/plugin.json`
   - `.claude-plugin/marketplace.json` and `.agents/plugins/marketplace.json` have no version; leave them.
2. Commit as `Release vX.Y.Z`, tag `vX.Y.Z`, and push `main` and the tag.
3. The tag push runs `.github/workflows/release.yml` (builds four targets and publishes the GitHub release). Do not move or delete a pushed tag without explicit approval.

# Rust architecture guidelines

Apply these patterns when adding or changing Rust code. Scale the architecture to the problem; introduce abstractions when they have a concrete purpose.

## Dependency boundaries

- Keep domain rules and transformations deterministic. Perform filesystem, network, process, and terminal I/O at the edges.
- Keep entry points thin: parse inputs, invoke application operations, and render results.
- Let application operations coordinate domain logic and external capabilities.
- Organize modules around responsibilities and dependency boundaries. Extract crates when enforcing a boundary or supporting reuse makes it worthwhile.
- Keep transport formats and environment details out of domain types. Convert them at the boundary.

## Traits and composition

- Prefer concrete structs and functions by default.
- Introduce small traits for capabilities that need interchangeable implementations or define a reusable protocol.
- Keep trait contracts focused on the behavior callers need.
- Use composable wrappers for policies such as tracing, timeouts, and concurrency limits when a common interface makes composition useful.
- Prefer straightforward functions over objects created solely to perform one action.

## Ownership and concurrency

- Decide who owns mutable state before choosing synchronization.
- For resources requiring coordinated access, consider one owning task that receives explicit commands and sends replies.
- Use bounded channels when queues need backpressure. Define shutdown and cancellation behavior for background work.
- Use shared locks when they fit the access pattern, and keep ownership and mutation rules clear.

## Types and invariants

- Use enums for mutually exclusive states and exhaustive handling of closed sets of commands or events.
- Use newtypes to distinguish identifiers and values with different meanings.
- Validate constrained values in constructors and keep invariant-bearing fields private.
- Express function preconditions in parameter types whenever practical.
- Use ordinary enums for state transitions unless typestate provides a clear benefit.
- Prefer borrowed views such as `&str`, `&[T]`, and `&Path` when callers do not need to transfer ownership.

## Internal representations and public APIs

- Give complex internals a small, understandable façade.
- For graphs and interconnected entities, consider a central owning store and typed IDs for relationships.
- Keep storage details and internal computation machinery behind the API boundary.
- Avoid creating public compatibility commitments for implementation details.

## Incremental computation

- Consider dependency-tracked computation when repeated work and invalidation are substantial problems.
- Keep authoritative inputs distinct from derived results and make dependencies explicit.
- Introduce an incremental database only when its benefits justify the added concepts.

## Validation

- Test domain behavior and invariants with deterministic inputs.
- Concentrate integration tests on meaningful I/O and API boundaries.
- Prefer behavior-oriented fixtures that allow internal APIs to evolve.
- Run checks appropriate to the change; documentation-only changes do not require compiling the application.

## Reference implementations

- [rust-analyzer architecture](https://rust-analyzer.github.io/book/contributing/architecture.html): dependency boundaries, typed IDs, façades, and deterministic core logic.
- [rust-analyzer style](https://rust-analyzer.github.io/book/contributing/style.html): type-level preconditions, local invariants, and function-oriented APIs.
- [Serde Serializer](https://docs.rs/serde/latest/serde/trait.Serializer.html): focused extension traits.
- [Tower Service design](https://tokio.rs/blog/2021-05-14-inventing-the-service-trait): common interfaces and composable middleware.
- [Tokio channels](https://tokio.rs/tokio/tutorial/channels): task-owned resources and command/reply channels.
- [Salsa algorithm](https://salsa-rs.github.io/salsa/reference/algorithm.html): dependency tracking and incremental recomputation.
