# ADR-0010: Add a network server/query layer in front of `ProductionStore`/`GenericProductionStore`

- Status: **Accepted** (promoted from Proposed on 2026-08-31 — the owner approved the design as proposed; no changes requested)
- Date: 2026-08-31 (proposed and accepted same day)
- Deciders: baileyrd
- Related: `docs/design/SERVER-QUERY-LAYER-DESIGN.md` (the full design
  document this ADR summarizes), `docs/FUTURE-GROWTH.md` ("Path to a
  server / query layer"), `docs/charter/CHARTER.md` (see "Consequences" —
  this proposal, if accepted, amends the charter's original "no server, no
  network surface" product-shape statement), ADR-0008 (`ProductionStore`),
  ADR-0009 (`crate::generic`/`GenericProductionStore`)
- Supersedes/Superseded by: none. Amends (does not supersede) the
  "Product shape" section of `docs/charter/CHARTER.md`, in the same way
  ADR-0005/ADR-0007/ADR-0008 already superseded that document's original
  "not implementing persistence... or concurrency control" non-goal
  without a matching charter rewrite at the time — see this ADR's
  Consequences for why this pass records that gap rather than silently
  repeating it.

## Acceptance and implementation

`SERVER-001` (`docs/specifications/server/SERVER-001-query-layer.md`) records the real implementation: `src/server/**`, gated behind a new `server` Cargo feature kept deliberately separate from `research` (this is new, additive capability, not a benchmarked alternative). Implements the accepted design essentially as proposed — `Request`/`Response` over length-prefixed `bincode` framing, thread-per-connection, a `ConnectionStore` trait `dispatch` is generic over — with the small, necessary completions `src/server/protocol.rs`'s own doc comment lists (`ScanValue::Str`, `Response::Id`/`ScanValues`, a named `ErrorCode` enum in place of a bare `u8`), none of which reopen any decision this ADR recorded.

Validated against both domains this ADR's Decision drivers named: `server::dog::DogConnectionStore` wraps `ProductionStore` (exercising `Neighbors`, `Dog`'s real symmetric relation); `server::order::OrderConnectionStore` (behind `research`, since `order_customer` itself is) wraps `GenericProductionStore<OrderProductionStack>` (exercising `Parent`/`Children`, `Order`/`Customer`'s real directed relation) — each domain validates the relation kind it actually has, the other reporting `ErrorCode::Unsupported` rather than a wrong answer. A real client/server round trip over a genuine `TcpListener`/`TcpStream` pair, not just `dispatch`'s in-process logic, is covered by `tests/server_dog_integration.rs`/`tests/server_order_integration.rs`, including a flagship concurrent-client stress test (8 real connections × 200 interleaved requests each against a contended id pool, verified via sequential-replay linearizability).

**Two real issues found and fixed during implementation, neither anticipated by the design, reported honestly rather than silently absorbed:**

- **A Nagle/delayed-ACK interaction**, confirmed directly: a synchronous request/response protocol with `TCP_NODELAY` left at its default cost ~40ms per round trip (a concurrent-client stress test ran in ~36s before the fix, well under a second after). Fixed by setting `TCP_NODELAY` server-side (`handle_connection`) and documenting the same requirement client-side. A real, measured cost of the chosen protocol shape (small, synchronous request/response frames), not a design flaw this ADR's protocol choice needs to be revisited over — the fix is a one-line socket option, not a different protocol.
- **A test-isolation bug**, not a server bug: two integration tests both deriving their mmap-backed temp-file path from the process id alone raced on the same file when `cargo test` ran them concurrently (its default). Fixed by adding a per-call counter to the test helper's path generation — unrelated to `ConnectionStore`/`dispatch`/framing correctness, which the same tests otherwise confirmed.

No existing source file outside `src/server/**`, `src/bin/dog_server.rs`, and `Cargo.toml`'s `[[bin]]`/`[[test]]`/`[features]` entries was modified — verified by diff, satisfying this ADR's own "additive, not a rewrite" decision driver.

## Context

This project's own `docs/FUTURE-GROWTH.md` (added last delivery cycle)
named two candidate future directions and explicitly deferred committing to
either. The owner has now chosen the server/query layer direction over the
SQLite/DuckDB-parity direction, in response to being asked which of the
two — if either — to pursue next. This ADR records that decision and the
design tradeoffs it entails, following this project's standing practice
(`adr-cadence.md` Regime 1: "establishing or changing a public interface,
data format, or protocol" is an explicit trigger for writing one) of
writing an ADR before implementation for anything establishing a new
public surface, matching how `ADR-0009` recorded the `crate::generic`
decision before any of it was implemented.

The original charter (`docs/charter/CHARTER.md`, "Product shape") states:
"No server, no persistence, no network surface, no CLI beyond what's
needed to drive benchmarks." Persistence and concurrency were both added
since then (`DURABILITY-TIER1`/`TIER2`, `CONCURRENCY-PROTOTYPES`,
`PRODUCTION-DEFAULT`) via their own ADRs (0005–0008), without the charter
document itself being amended to match — this project has consistently
treated ADRs as the record of scope expansion beyond the original charter,
not required the charter to be rewritten each time. This ADR follows that
same established pattern for the "no network surface" clause specifically.

## Decision drivers

- **Additive, not a rewrite.** `docs/FUTURE-GROWTH.md` already established
  that a server layer can sit on top of the existing storage API without
  changing the engine itself — this decision should preserve that property,
  not motivate touching `ProductionStore`/`GenericProductionStore`'s own
  code.
- **Minimal new dependency footprint**, matching the charter's standing
  "minimal dependencies; each new crate is justified" constraint — prefer
  reusing what's already justified (`bincode`, already present from the
  durability work) over adding a new serialization or async-runtime
  dependency without a demonstrated need.
- **Name what's genuinely new, don't quietly scope it in.**
  `docs/FUTURE-GROWTH.md` already separated "genuinely additive" work from
  "genuinely new" work (authentication, session/transaction semantics, a
  query language) — this decision keeps that separation explicit rather
  than letting the "genuinely new" items creep into a first design pass.
- **Design-first, matching `ADR-0009`'s own precedent.** A network-facing
  public protocol is comparably hard-to-reverse to the `crate::generic`
  schema decision (once a client depends on the wire format, changing it
  is a compatibility break) — this ADR proposes a design, and authorizes
  no implementation, for the same reason `ADR-0009` didn't.

## Considered options

See `docs/design/SERVER-QUERY-LAYER-DESIGN.md`'s "Considered options"
section for the full reasoning (protocol/framing, concurrency/async
runtime, and field-addressing choices). Summarized:

1. **Protocol**: JSON-over-HTTP (rejected — new HTTP + JSON dependencies
   for no current cross-language requirement), gRPC (rejected — codegen +
   runtime footprint disproportionate to this proposal's scope), or a
   hand-rolled length-prefixed binary protocol reusing the existing
   `bincode` dependency (**chosen**).
2. **Concurrency model**: `tokio`/async (rejected — a significant, viral
   new dependency for a benefit — many idle connections — with no evidence
   this project needs it) vs. `std::thread`-per-connection sharing one
   `Arc<RwLock<Store>>` (**chosen** — the same pattern
   `CONCURRENCY-PROTOTYPES`/`PRODUCTION-DEFAULT` already validated, applied
   to connections instead of benchmark threads).
3. **Field addressing**: string field names via a schema-description
   sub-protocol (deferred — real, separate scope) vs. small integer tags
   fixed per domain at server start (**chosen** for v1).

## Decision

- `docs/design/SERVER-QUERY-LAYER-DESIGN.md` records the full accepted
  design: a `Request`/`Response` enum pair covering
  `GetById`/`FilterEq`/`ScanField`/`UpdateField`/`Parent`/`Children`/
  `Neighbors`, length-prefixed `bincode` framing over
  `std::net::TcpStream`, thread-per-connection dispatch against a shared
  `ConnectionStore` trait object.
- No new dependency is introduced by this design; `bincode` (already
  present) is reused. `tokio`, an HTTP framework, and a gRPC toolchain are
  named and explicitly not added.
- **Acceptance of this ADR authorizes the design, not implementation
  code.** No existing source file is modified by this ADR itself. Per this
  ADR's own "Validation and revisit triggers" below, the next unit
  registers a `SERVER-001` specification and a real implementation packet
  (per `delivery-loop.md`'s "Plan" step) before any server code is
  written — matching how `STORAGE-012` followed `GENERIC-SCHEMA-DESIGN`'s
  own acceptance as a separate step, not the same commit.
- Authentication, authorization, transport encryption, and any query
  language beyond fixed field-tag addressing remain explicit non-goals of
  the *accepted* design, not silently deferred — see the design document's
  "Non-goals" and "Security, privacy, and compatibility" sections.
  **Accepting this ADR does not authorize deploying a server binary
  outside a trusted, localhost/development context** — that would require
  at minimum the authentication/encryption work this ADR explicitly defers,
  and would be its own decision when it's proposed.

## Consequences

### Positive

- A concrete, compiled (in a standalone scratch probe, not this
  repository) proof that the proposed request/response/dispatch shapes are
  real, statically-typed Rust — the same "prove signatures compile, don't
  just assert they would" discipline `ADR-0009` established.
- Reuses two already-validated pieces of this project's own prior work
  directly: `bincode` (already justified by the durability round) for
  encoding, and the `RwLock`-shared-store concurrency pattern
  (`CONCURRENCY-PROTOTYPES`/`PRODUCTION-DEFAULT`) for connection handling —
  no new concurrency primitive to design or verify from scratch.
- Keeps the charter's scope-expansion pattern consistent: this ADR names
  the "no server, no network surface" clause it would amend explicitly,
  rather than letting a future reader discover the contradiction
  unexplained the way the persistence/concurrency clauses currently sit
  unremarked in the charter text.

### Negative / tradeoffs

- **No authentication, authorization, or transport encryption** — a real,
  named gap, not a hidden one. This is the largest reason this proposal
  stops at design, not implementation: shipping a listening network binary
  without either would be a genuine security regression for this project
  the moment it left localhost.
- Thread-per-connection has a real, unmeasured practical ceiling on
  concurrent connections, accepted deliberately in exchange for avoiding a
  new async-runtime dependency — a real tradeoff, not a free choice; see
  the design document's open questions.
- Integer field tags require the client to be compiled against the same
  domain type as the server (or told the tag assignment out of band) — no
  schema-discovery story exists yet, so this design serves a single
  known-in-advance Rust client, not an arbitrary one.
- `bincode`'s wire-format stability across crate versions is unverified
  for this new use (client/server version skew) — previously only mattered
  within one process's own on-disk lifetime, a materially different
  compatibility bar. *Verified and scoped by `ADR-0021` /
  `docs/design/BINCODE-ENCODING-STABILITY-DESIGN.md` (Accepted,
  2026-09-02): the free functions pin fixint/little-endian/no-limit,
  stable across 1.x by `bincode`'s own promise "provided the same
  configuration is used"; the proposal names that configuration in one
  codec and pins it with golden vectors. Implemented as `STORAGE-018`
  v0.1.0 (`crate::codec`; `SERVER-001` v0.9.1) — resolved.* *The
  shape's evolution — a version and a handshake — followed as
  `ADR-0022` / `SERVER-001` v0.10.0 (FR-020): `PROTOCOL_VERSION = 2`,
  an optional first-frame `Hello`, append-only rules — resolved.*

## Validation and revisit triggers

- **Original proposal validation**: design-only, as `ADR-0009`'s original
  proposal was — the proposed types compiled (not executed) in a
  standalone, dependency-free scratch probe.
- **Real validation, post-acceptance**: `SERVER-001`
  (`docs/specifications/server/SERVER-001-query-layer.md`), a real
  implementation (`src/server/**`) validated against both `Dog` and
  `Order`/`Customer` over a genuine `TcpListener`/`TcpStream` pair,
  including a flagship concurrent-client stress test — see "Acceptance and
  implementation" above for the full account, including the two real
  issues (a Nagle/delayed-ACK cost, a test-isolation bug) found and fixed
  along the way.
- Revisit if: a non-Rust or cross-language client becomes a real
  requirement (reconsider gRPC/JSON-HTTP); the thread-per-connection model
  is measured and found to be the actual bottleneck under a real workload
  (reconsider `tokio` — still unmeasured, since this round built no
  throughput benchmark, see `SERVER-001`'s own "Open questions"); the
  project decides to pursue authentication/encryption, at which point this
  ADR's "no auth" consequence should be superseded rather than silently
  outdated; or a third domain beyond `Dog`/`Order`-`Customer` surfaces a
  request shape this protocol can't express (matching this project's own
  "validate against a genuinely different second domain" discipline before
  generalizing further — this round already cleared that bar with the two
  domains named above).
