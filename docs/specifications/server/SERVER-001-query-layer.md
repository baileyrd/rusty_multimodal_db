# SERVER-001 — Network server/query layer: `Request`/`Response` protocol in front of `ProductionStore`/`GenericProductionStore`

- Version: 0.3.0 (third validation domain, `Employee` — see "Change history")
- Status: Accepted
- Owners: baileyrd
- Depends on: `STORAGE-011` (`ProductionStore`), `STORAGE-012` (`GenericProductionStore`)
- Supersedes: none

## Purpose and scope

`docs/decisions/ADR-0010-server-query-layer-proposal.md` (Accepted) and `docs/design/SERVER-QUERY-LAYER-DESIGN.md` (Accepted) proposed a thin network server/query layer in front of this crate's existing `ProductionStore`/`GenericProductionStore` — a `Request`/`Response` wire protocol over length-prefixed `bincode` framing, thread-per-connection, reusing whichever `RwLock` the wrapped store already manages. This spec covers the real implementation: `src/server/**`, the `server` Cargo feature, three domain adapters (`Dog`, `Order`/`Customer`, `Employee`), a minimal server binary, and the tests that verify it end to end over a real socket.

## Non-goals

- Not authentication, authorization, or transport encryption — named as explicit, blocking gaps by both the design and ADR-0010; this spec does not authorize deploying a server built from it beyond a trusted, localhost/development network.
- Not transaction semantics — every request is single-shot, same as every existing store method.
- Not a query language beyond fixed, server-assigned integer field tags — no parser, no planner, no schema-description sub-protocol.
- Not an async runtime (`tokio`) — thread-per-connection was the chosen concurrency model (ADR-0010's "Considered options"); this spec doesn't revisit that choice.
- Not a change to `ProductionStore`, `GenericProductionStore`, or any existing store/durability/concurrency code — this is a new, additive translation layer. **One exception, v0.3.0**: validating the `Employee` domain adapter surfaced a real, load-bearing gap directly in `crate::generic` (`Reversed` never forwarded `Neighbors`) — see FR-012. That fix completes already-accepted `STORAGE-012` capability, not a new storage-layer decision, and is the only `src/generic/**` change this spec's implementation has ever required.
- Not a benchmark suite (`benches/server.rs`) — this round's acceptance criteria are correctness (a real client/server round trip, a concurrent-client stress test), not throughput measurement. Named as a real, unscoped follow-up, not silently assumed unnecessary — see "Open questions."
- Not `CreatedAt`/`DiscountCents` field access for `Order` — `OrderProductionStack` (the durable stack this spec wraps) only carries `Status`/`Amount`; those two fields are in-memory-only in `crate::generic` and were never part of the durable production stack to begin with (see `src/server/order.rs`'s own module docs).

## Context and terminology

- **`server` Cargo feature**: off by default, distinct from `research` — this is new, real, additive capability (a network-listening binary surface), not a benchmarked-alternative or historical-spike module. Validating it against `Order`/`Customer` or `Employee` additionally needs `research` (since `order_customer` and `employee_impl` are both research-gated reference material); the `Dog` side needs `server` alone.
- **`ConnectionStore`**: the one trait `dispatch`/`serve` are generic over (`src/server/mod.rs`) — a thin per-domain adapter, implemented by `server::dog::DogConnectionStore` (wraps `ProductionStore`), `server::order::OrderConnectionStore` (wraps `GenericProductionStore<OrderProductionStack>`, behind `research`), and `server::employee::EmployeeConnectionStore` (wraps `GenericProductionStore<EmployeeProductionStack>`, behind `research`).
- **`FieldRef`**: a `u16` field tag, fixed per domain adapter at compile time (not runtime-negotiated) — `Dog`'s `FIELD_BREED`/`FIELD_AGE`, `Order`'s `FIELD_AMOUNT`/`FIELD_STATUS`/`FIELD_CREATED_AT`/`FIELD_DISCOUNT`, `Employee`'s `FIELD_NAME`/`FIELD_DEPARTMENT`/`FIELD_SALARY`.
- **`ScanValue`**: the wire value type. Extends the accepted design's original `U32`/`I64`/`Bool` with `Str(String)` — a necessary completion for `Dog::breed`'s `GetById` response, not a reopened decision; see `src/server/protocol.rs`'s own doc comment for the full list of implementation-time completions beyond the design's proposed shape.
- **`ParentLookup`**: a three-way enum (`Parent(id)`/`NoParent`/`ChildNotFound`) preserving the not-found/no-parent distinction this project's own PR #21 (`docs/PROJECT-STATUS.md`'s `Parent::parent` fix) restored in-process — deliberately not collapsed into a two-way `Option` at the wire boundary.

## Requirements

- `SERVER-001-FR-001`: **`ConnectionStore` trait and `dispatch`** (`src/server/mod.rs`) — one trait covering `get`/`filter_eq`/`scan_field`/`update_field`/`parent`/`children`/`neighbors`; `dispatch(store, request) -> Response` translates one `Request` into one `Response` with no I/O of its own, independently testable.
- `SERVER-001-FR-002`: **Length-prefixed `bincode` framing** (`src/server/framing.rs`) — a 4-byte little-endian length prefix, then a `bincode`-encoded payload, over any `Read`/`Write`. A length prefix exceeding `MAX_FRAME_BYTES` (16 MiB) is rejected *before* any payload buffer is allocated.
- `SERVER-001-FR-003`: **Thread-per-connection serving** (`src/server::serve`) — accepts connections on a `TcpListener` and spawns one OS thread per connection, each holding only `&S: ConnectionStore`; all coordination is whatever locking the wrapped store already does internally (no new lock introduced at this layer). `TCP_NODELAY` is set on every accepted connection — see FR-006.
- `SERVER-001-FR-004`: **`Dog` domain adapter** (`src/server/dog.rs`) — wraps `S: DogStore + ConcurrentStore` (in practice `ProductionStore`). `get`/`scan_field`/`update_field`/`neighbors` map onto `DogStore`/`ConcurrentStore`'s existing methods; `filter_eq`/`parent`/`children` report `ErrorCode::Unsupported` (no equality-index or directed relation exists for `Dog` in-process either).
- `SERVER-001-FR-005`: **`Order`/`Customer` domain adapter** (`src/server/order.rs`, behind `research`) — wraps `GenericProductionStore<OrderProductionStack>`. `get`/`filter_eq`(`Status`)/`scan_field`+`update_field`(`Amount`)/`parent`/`children`(`BelongsToCustomer`) map onto its existing generic methods; `neighbors` reports `ErrorCode::Unsupported` (no symmetric relation exists for `Order`/`Customer`).
- `SERVER-001-FR-006`: **A real, measured Nagle/delayed-ACK fix** — a synchronous request/response protocol pays a real ~40ms-per-round-trip cost with Nagle's algorithm left at its default on either side of the connection; confirmed directly (a concurrent-client integration test went from ~36s to well under a second after disabling it). `TCP_NODELAY` is set server-side (`handle_connection`) and documented as required client-side too.
- `SERVER-001-FR-007`: **A minimal server binary** (`src/bin/dog_server.rs`, `required-features = ["server"]`) — a real, runnable `Dog`-domain server seeded from a small hand-written sample dataset (not `generator`, which is research-gated), so it builds under `server` alone.
- `SERVER-001-FR-008`: **Real end-to-end test coverage**, not just `dispatch`'s in-process logic — `tests/server_dog_integration.rs` and `tests/server_order_integration.rs` drive a real `TcpListener`/`TcpStream` pair (a background thread with a real socket, not a genuinely separate OS process — see those files' own module docs on what that does and doesn't prove) through `GetById`/`FilterEq`/`ScanField`/`UpdateField`/`Parent`/`Children`/`Neighbors`, including the domain-appropriate `Unsupported` cases.
- `SERVER-001-FR-009`: **A flagship concurrent-client stress test**, matching this crate's established rigor — `tests/server_dog_integration.rs`'s `concurrent_clients_over_the_wire_match_a_sequential_replay` runs 8 real client connections × 200 interleaved `GetById`/`UpdateField` requests each against a small contended id pool, verified via sequential-replay linearizability against a fresh in-memory reference (the same pattern `run_concurrency_stress_test`/`production_integration.rs` use), with the write-log append made atomic with the request's round trip — the same fix (and the same false-positive failure mode) `run_concurrency_stress_test`'s own doc comment already documents.
- `SERVER-001-FR-010` (v0.2.0, ADR-0011): **Schema discovery** — `Request::DescribeSchema`/`Response::Schema(DomainSchema)`; `ConnectionStore::describe(&self) -> DomainSchema` (infallible, no store access needed). Both domain adapters implement it, reporting every named field's `ValueKind` and per-operation `FieldCapabilities` (`filter_eq`/`scan`/`update`) honestly — including fields that exist but support none of the three (`Order`'s `created_at_unix_ms`/`discount_cents`) — plus `RelationCapabilities` (`parent_children`/`neighbors`). Field *tags* remain the wire addressing scheme; this adds runtime discovery of what a compile-time client already knows, not a new addressing scheme.
- `SERVER-001-FR-011` (v0.2.0, ADR-0011): **Schema discovery is genuinely usable, not just descriptive** — `tests/server_dog_integration.rs`'s `a_schema_driven_client_discovers_and_uses_the_age_field` and `tests/server_order_integration.rs`'s `a_schema_driven_client_discovers_and_uses_the_status_field` each drive a real client that starts with zero compile-time field-tag knowledge, calls `DescribeSchema`, finds a field by name, and completes a real `UpdateField`/`FilterEq` using only the discovered tag.
- `SERVER-001-FR-012` (v0.3.0): **`Employee` domain adapter** (`src/server/employee.rs`, behind `research`) — wraps `GenericProductionStore<EmployeeProductionStack>`. `get`/`filter_eq`(`Department`)/`scan_field`+`update_field`(`SalaryCents`)/`parent`/`children`(`ReportsTo`)/`neighbors`(`CollaboratesWith`) all map onto real generic methods — the first domain adapter where no relation-kind request is `Unsupported`, since `Employee` (`src/generic_spike/employee_impl.rs`) is self-referential on both a directed (`reports_to`) and a symmetric (`collaborates_with`) relation at once. Building this adapter required a real fix one layer down: `Reversed` (the `ChildOf`-forwarding wrapper) never forwarded `Neighbors`, and `GenericProductionStore` had no `neighbors` method — neither had been needed by any domain stacking only one relation kind. Both fixed directly in `src/generic/{store,production}.rs`; see `docs/decisions/ADR-0009-generic-schema-design-proposal.md`'s "Acceptance and implementation" addendum for the full account. `crate::generic`'s own trait/wrapper design is otherwise unchanged.
- `SERVER-001-FR-013` (v0.3.0): **Real end-to-end coverage of `Employee`, including the first all-relations-real case** — `tests/server_employee_integration.rs` drives a real `TcpListener`/`TcpStream` pair through `GetById`/`FilterEq`/`ScanField`/`UpdateField`/`Parent`/`Children`/`Neighbors`, all succeeding (no domain-shaped `Response::Err`), plus the empty-but-real cases (`Parent` on a record with no manager returns `Response::NoParent`, not an error; `Neighbors` on a record with no collaborators returns an empty `RecordList`, not an error) and a schema-driven test (matching FR-011's pattern) confirming `DescribeSchema` reports both `RelationCapabilities` as `true` for the first time.

## Architecture and interfaces

`src/server/{mod,protocol,framing,dog}.rs` (unconditional under `server`); `src/server/order.rs`, `src/server/employee.rs` (additionally behind `research`). `src/bin/dog_server.rs` (`required-features = ["server"]`). `tests/server_dog_integration.rs` (`required-features = ["server"]`), `tests/server_order_integration.rs`, `tests/server_employee_integration.rs` (`required-features = ["server", "research"]`). No changes to `src/production.rs`, `src/store/**`, `src/durability/**`, `src/concurrency/**`. `src/generic/{store,production}.rs` gained the `Neighbors`-forwarding fix described in FR-012 — the one exception to this spec's original "no `crate::generic` changes" scope, a completion of already-accepted `STORAGE-012` capability rather than a new decision (see `STORAGE-012`'s own change history). No new `Cargo.toml` dependency — reuses `bincode`/`serde`/`uuid`, all already present.

## Data/state and invariants

- No new persistent state or on-disk format — the server process wraps an existing durable store; every byte on disk is written by `ProductionStore`/`GenericProductionStore` exactly as it already was.
- Per-connection state is limited to the TCP stream and its read/write buffers — no session, no per-client cursor, no cross-request server-side state, matching the "no transaction semantics" non-goal directly.
- Field tags are fixed at compile time per adapter (constants in `dog.rs`/`order.rs`), not negotiated at connection time.

## Errors, failure, recovery, and observability

- `dispatch` never panics on a well-formed but semantically invalid request (unknown field, unsupported operation, malformed value) — it returns `Response::Err { code, message }` with one of three `ErrorCode` variants (`UnknownField`/`Unsupported`/`Malformed`).
- `handle_connection` never panics on a malformed or oversized frame, or on a client disconnecting mid-request — the connection simply ends (`FrameError`, an ordinary `Result::Err`, not a panic).
- One bad `accept()` doesn't take down `serve`'s loop; one connection's error doesn't affect any other connection, since the only shared state is the wrapped store itself, already safe for concurrent access.
- Out of scope, named rather than silently assumed solved: structured logging/metrics, graceful shutdown/drain, a health-check RPC.

## Security, privacy, and compatibility

Identical posture to ADR-0010's own "Security, privacy, and compatibility": no authentication, no authorization, no transport encryption. `MAX_FRAME_BYTES` bounds one specific resource-exhaustion vector (a corrupt/hostile length prefix forcing an unbounded allocation) but is not a general defense against a hostile network peer — this spec's implementation does not change ADR-0010's "do not expose beyond a trusted, localhost/development network" conclusion.

## Acceptance criteria

- `cargo test --features server` passes (`Dog` domain: unit tests in `server::{dog,framing,protocol}` + `server::tests`, plus `tests/server_dog_integration.rs`'s four tests including the concurrent-client stress test and the schema-driven test).
- `cargo test --features server,research` additionally passes `server::order::tests::*`, `server::employee::tests::*`, `tests/server_order_integration.rs`'s two tests, and `tests/server_employee_integration.rs`'s three tests (the all-relations-real round trip, the schema-driven test, and the empty-but-real `Parent`/`Neighbors` cases).
- `cargo test --all-features`/`cargo test` (default) both still pass unchanged elsewhere — the `server` module adds no default-build surface.
- `cargo fmt --all -- --check` / `cargo clippy --all-targets --all-features -- -D warnings` / `cargo check --all-targets --all-features` all clean.
- No `src/production.rs`, `src/store/**`, `src/durability/**`, or `src/concurrency/**` changes — verified by diff. `src/generic/{store,production}.rs` changes are limited to the one `Neighbors`-forwarding completion FR-012 describes.

## Verification plan

- Unit tests: wire-format round trip (`protocol.rs`), framing round trip + oversized/truncated-frame handling (`framing.rs`), `dispatch`'s per-request-kind response mapping against a minimal fixture store (`mod.rs`), each domain adapter's own behavior against a real `ProductionStore`/`GenericProductionStore` (`dog.rs`/`order.rs`/`employee.rs`).
- Real end-to-end tests: a genuine `TcpListener`/`TcpStream` pair, all three domains, every request kind including the domain-appropriate `Unsupported` cases (`Dog`, `Order`/`Customer`) and the all-real case (`Employee`), a second independent connection observing the first's writes (proving the store, not connection-local state, is shared).
- Flagship stress test: 8 real client connections, 200 requests each, interleaved reads/writes against a 20-id contended pool, verified via sequential-replay linearizability.

## Traceability

Implements: the server/query-layer capability ADR-0010 (Accepted) and `docs/design/SERVER-QUERY-LAYER-DESIGN.md` (Accepted) proposed; v0.2.0's schema discovery implements ADR-0011 (Accepted); v0.3.0's `Employee` domain adapter completes `ADR-0009`'s "revisit if" trigger (`SymmetricRelation` + `ChildOf` together, untested until now) via `SERVER-QUERY-LAYER`'s own third-validation-domain round. No prior spec superseded.

## Change history

- 0.1.0: Initial implementation — `Request`/`Response` protocol, framing, thread-per-connection dispatch, both domain adapters, the flagship stress test.
- 0.2.0 (ADR-0011): Schema discovery — `DescribeSchema`/`Response::Schema(DomainSchema)`, `ConnectionStore::describe`, both adapters' shapes, two schema-driven integration tests (FR-010, FR-011).
- 0.3.0: Third validation domain, `Employee` (`server::employee`) — the first adapter with both `Parent`/`Children` and `Neighbors` real. Found and fixed a real gap in `crate::generic::{store,production}` (`Reversed` never forwarded `Neighbors`) along the way (FR-012, FR-013).

## Open questions

- No throughput/latency benchmark exists yet for the server layer (`benches/server.rs` was not built this round — see "Non-goals") — a real, unscoped follow-up if the owner wants server-layer numbers alongside the existing in-process ones.
- The thread-per-connection model's real connection-count ceiling is unmeasured — an accepted, deliberate limitation of ADR-0010's chosen concurrency model, not a proven-acceptable one.
- `bincode`'s wire-format stability across crate versions (client/server built from different versions) is unverified — a materially different compatibility bar than its existing on-disk use within one process's own lifetime.
