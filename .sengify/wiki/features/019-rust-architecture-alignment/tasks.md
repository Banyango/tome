# Rust Architecture Alignment — Task Plan

Source feature: [feature.md](./feature.md)

## Tasks

### 019-1. Typed run-creation inputs at the RPC boundary

**Blocked by:** none

RPC requests for run creation and execution are decoded and validated at the transport edge, and application operations take typed inputs instead of raw JSON fields. Malformed requests are rejected before any application side effects, and arbitrary user payloads may stay JSON. Covers use case 1.

### 019-2. Built-in state and event enums

**Blocked by:** none

Step statuses, step-history events and message-bus delivery states become enums with exhaustive handling, converted to the existing strings at storage and output boundaries. An unknown built-in state in stored data yields an explicit decoding error. Custom step statuses from feature 018 stay free-form and separate. Covers use case 2.

### 019-3. Distinct run, event and delivery IDs

**Blocked by:** 019-1, 019-2

Newtype identifiers for runs, events and deliveries are carried through the store, engine and bus relationships, so mixing them up fails at compile time. Numeric representations in the database and public protocol are unchanged, and missing entities supplied via public numeric IDs keep their existing error behavior. Covers use case 3.

### 019-4. Validated workflow and topic-pattern invariants

**Blocked by:** none

Invariant-bearing workflow fields become private, with validated construction, accessors and controlled updates. Construction paths that bypass validation, including the empty `Pattern::default()` that its parser rejects, are removed or replaced, and invalid updates or empty patterns are rejected through the validated APIs. Covers use case 4.

### 019-5. Bounded run-event streams and filesystem-event queues

**Blocked by:** 019-1

Run-event streaming and OS filesystem-event queues get specified capacities and explicit overload behavior. A slow stream consumer gets a recoverable disconnect or replay path without unbounded memory growth, blocking unrelated operations or silently dropping lifecycle transitions. Filesystem-queue overflow marks the affected watch for reconciliation and rescans it to recover current state. Covers use case 5.

### 019-6. Coordinated daemon shutdown

**Blocked by:** 019-1, 019-5

The daemon owns its background thread handles and cancellation signals. On shutdown it stops admitting work, cancels background loops and connection handling, releases blocked waits, and joins background work before closing the store, coordinating with in-flight run creation and event publication. A documented bounded shutdown policy covers I/O that doesn't respond to cancellation. Covers use case 6.
