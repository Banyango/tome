# Rust Architecture Alignment

## Description

Align the existing Rust implementation with `AGENTS.md` by addressing six architecture issues found in the codebase review. This feature improves internal boundaries, type safety, overload handling, and daemon shutdown while preserving existing CLI, RPC, workflow, and storage compatibility wherever possible.

### Scope

1. **Typed application inputs.** Decode and validate RPC requests at the transport boundary, then invoke run creation and execution operations with typed inputs. Application operations should not interpret raw RPC JSON fields. Initial evidence: `src/engine.rs` (`Engine::start`) and `src/api.rs` (`create_run`). Arbitrary user payloads may remain JSON where JSON is their actual domain representation.
2. **Enums for built-in states and events.** Replace string representations of closed sets, including step statuses, step history events, and message-bus delivery states, with enums and exhaustive handling. Convert to existing strings at storage and output boundaries. Initial evidence: `src/store.rs` (`Step`, `StepHistory`) and `src/store/events.rs` (`state`, `Delivery`).
3. **Distinct identifier types.** Introduce newtypes for run, event, and delivery IDs and carry them through internal operations and relationships, preventing accidental interchange. Preserve numeric representations in the database and public protocol. Initial evidence: `src/store.rs` (`Run`) and `src/store/events.rs` (`BusEvent`, `Delivery`).
4. **Protected validated invariants.** Keep invariant-bearing workflow fields private and expose validated construction, accessors, and controlled updates. Remove or replace construction paths that bypass validation, including the empty `Pattern::default()` that its parser rejects. Initial evidence: `src/workflow.rs` (`Workflow`, `Frontmatter`) and `src/topic.rs` (`Pattern`).
5. **Bounded event queues.** Bound run-event streaming and OS filesystem-event queues, with explicit overload behavior. Slow stream consumers must not cause unlimited memory growth or block unrelated operations. Filesystem-event overflow must trigger reconciliation rather than silently lose changes. Initial evidence: `src/engine.rs` (`Engine::watch`) and `src/watch.rs` (`Os::start`).
6. **Coordinated daemon shutdown.** Own background thread handles and cancellation signals. Stop accepting work, cancel background loops and connection handling, release blocked waits, and join background work before closing the store and exiting. Define a bounded shutdown policy for stalled I/O. Initial evidence: `src/daemon.rs` (`run_foreground`, `Daemon::shutdown`).

These are findings from static inspection, not confirmed runtime defects. Implementation should verify each affected path before changing it.

## Use Cases

1. As a maintainer, I want transport parsing separated from application operations, so that domain behavior can be tested with deterministic typed inputs.
2. As a maintainer, I want built-in states and events represented by enums, so that invalid values are rejected and state handling is exhaustive.
3. As a maintainer, I want distinct identifier types, so that the compiler catches mistaken relationships between runs, events, and deliveries.
4. As a maintainer, I want validated values to retain their invariants after construction, so that later operations can rely on them without repeating validation.
5. As a user, I want slow event consumers and filesystem bursts handled within bounded memory, so that the daemon remains responsive and changes are reconciled.
6. As a user, I want daemon shutdown to coordinate background work and database closure, so that threads do not race against a closed store or remain stuck waiting.

## Constraints

- Follow the architecture guidance in `AGENTS.md`; prefer concrete functions and structs, adding abstractions only for a concrete purpose.
- Keep filesystem, network, process, and terminal I/O at the edges, with deterministic rules and transformations behind typed interfaces.
- Preserve existing CLI flags, RPC field names, serialized state names, workflow syntax, and numeric IDs unless a necessary behavior change is explicitly documented.
- Keep user-defined custom step statuses from feature 018 free-form and separate from built-in state enums.
- Keep DuckDB as the authoritative store; this feature does not require a database replacement, incremental database, or crate split.
- Shared locks are allowed where ownership and mutation rules are clear; this feature does not mandate replacing every lock with an actor.
- Specify and test queue capacities, overflow recovery, cancellation, and shutdown ordering during implementation.
- Test domain invariants with deterministic inputs and integration behavior at RPC, storage, event-stream, filesystem-watch, and shutdown boundaries. Update `docs/src` for any user-facing behavior changes.
- New product features, unrelated refactors, and implementation of features 017 or 018 are out of scope.

## Edge Cases

- Malformed RPC inputs → reject at the transport boundary before application side effects.
- Unknown built-in state in stored data → return an explicit decoding error rather than constructing an invalid domain state.
- An identifier for the wrong entity is passed internally → fail at compile time; missing entities supplied through public numeric IDs retain existing error behavior.
- A caller attempts an invalid workflow update or empty topic-pattern construction → reject through validated APIs.
- A run-event consumer falls behind → use an explicit bounded overload policy with a recoverable disconnect or replay path; do not silently drop lifecycle transitions.
- Filesystem events exceed queue capacity → mark the affected watch for reconciliation and rescan to recover its current state.
- Shutdown occurs while clients are streaming or threads await events → cancellation releases waits and permits coordinated termination.
- Background I/O does not respond to cancellation → apply the documented bounded shutdown policy rather than waiting indefinitely.
- Shutdown overlaps with run creation or event publication → stop admitting work and coordinate in-flight mutations before database closure.

## Related Entities

- [[daemon]]
- [[workflow]]
- [[run]]
- [[step]]
- [[event]]
- [[trigger]]
- [[session]]
