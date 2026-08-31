# Server/Query Layer Design (Accepted)

- Status: **Accepted** (promoted from Proposed on 2026-08-31 — the owner
  approved the design as proposed, no changes requested). Acceptance
  authorizes the design; implementation still requires its own unit
  (registering `SERVER-001` and a planning packet) before any server code
  is written — see ADR-0010's "Decision" section. See
  `ADR-0010-server-query-layer-proposal.md` for the decision record this
  document backs.
- Date: 2026-08-31
- Related: `docs/FUTURE-GROWTH.md` (the "Path to a server / query layer"
  section this design realizes), ADR-0008 (`ProductionStore`), ADR-0009
  (`crate::generic`/`GenericProductionStore`)

## Purpose and scope

`docs/FUTURE-GROWTH.md` named a server/query layer as "genuinely additive —
no rework of the storage layer required," on top of the existing
`ProductionStore`/`GenericProductionStore` API. This document proposes the
smallest real slice of that: a single binary that owns a store and answers
requests from separate client processes over a socket, translating each
request into an existing trait-method call and serializing the result back.

**In scope for this proposal:**

- A binary-framed wire protocol covering the query operations `crate::generic`
  already exposes: `GetById`, `FilterEq`, `ScanField`, `UpdateField`,
  `Parent`, `Children`, `Neighbors`.
- A concurrency model for multiple simultaneous client connections.
- A dependency decision for the transport/framing layer.

**Explicitly out of scope, named directly rather than left implicit** (per
`docs/FUTURE-GROWTH.md`'s own "genuinely new" list):

- Authentication/authorization. Nothing like it exists in this crate today;
  a real deployment would need it before this went anywhere near an
  untrusted network, but designing it is a separate, later decision.
- Session/transaction semantics across multiple requests. Every RPC in this
  proposal is single-shot, mirroring every existing store method today.
- A query language. Requests name a field by a fixed, server-assigned tag
  (see "Field addressing" below), not a parsed expression — there is no
  parser, no planner, no `WHERE`-clause equivalent in this proposal.

## Non-goals

- Not a network protocol usable by non-Rust clients in this pass (no schema
  description sub-protocol, no code-gen'd client stubs) — a single Rust
  client crate speaking the same wire format is the only consumer this
  design accounts for.
- Not multi-server / distributed. One process owns one store; there is no
  replication, sharding-across-servers, or failover story here.
- Not a replacement for in-process use. `ProductionStore`/
  `GenericProductionStore` remain the way to use this crate from within the
  same process; the server is an additive way to use it from a different
  one.

## Context and terminology

Every workload this crate has ever benchmarked runs in-process. `RESULTS.md`
and `PRODUCTION-DEFAULT`/`GENERIC-SCHEMA-LIBRARY` establish that
`ProductionStore`/`GenericProductionStore` already do the concurrency-safe,
crash-safe, multi-process-safe (as of `GENERIC-MMAP-APPEND-SLOT-RACE-FIX`)
job of owning a store. A server is a thin translation layer in front of
that existing, already-validated core — not a new storage engine.

"Field addressing": the query traits (`ScannableField<Marker>`,
`IndexedField<Marker>`) are markers resolved at compile time in-process.
Over the wire, a field has to be named some other way, since a client
process doesn't share the server's type system. This proposal names each
field with a small integer tag, fixed per domain at server start, rather
than a string — avoiding a schema-description sub-protocol for v1 (see
"Considered options" below).

## Requirements

- `SERVER-FR-001`: The server accepts concurrent client connections and
  answers each one's requests against a shared store instance with no lost
  updates or torn reads — reusing `ProductionStore`/`GenericProductionStore`'s
  existing `RwLock`-based guarantee, not a new one.
- `SERVER-FR-002`: `GetById`, `FilterEq`, `ScanField`, `UpdateField` are
  exposed for at least one domain implementing `crate::generic`'s traits.
- `SERVER-FR-003`: `Parent`/`Children`/`Neighbors` are exposed for at least
  one domain exercising each (matching this crate's own "validate against a
  second, structurally different domain" discipline — see "Validation
  plan").
- `SERVER-FR-004`: A malformed or oversized request is rejected with a
  typed error response, not a panic or a silently truncated read.
- `SERVER-NFR-001`: The RPC dispatch layer adds no coordination beyond what
  `ProductionStore`/`GenericProductionStore` already provide — no new lock,
  no new shared mutable state outside the wrapped store.

## Architecture and interfaces

### Considered options

**Protocol/framing: JSON-over-HTTP vs. gRPC vs. a hand-rolled length-prefixed
binary protocol.**

1. *JSON-over-HTTP.* Rejected for v1 — pulls in an HTTP server dependency
   (`axum`/`hyper` or similar) and a JSON serialization dependency neither
   of which this crate has ever needed, for a benefit (human-readable
   wire format, browser-reachable) that doesn't matter yet: the only
   planned consumer is a Rust client speaking the same protocol.
2. *gRPC.* Rejected for v1 — needs a `.proto` schema, codegen, and a gRPC
   runtime (`tonic` + `prost`), a materially bigger dependency and build-
   step footprint than this proposal's scope justifies; revisit if a
   non-Rust or cross-language client ever becomes a real requirement (see
   revisit triggers).
3. *Hand-rolled length-prefixed binary framing over `std::net::TcpStream`,
   payload serialized with `bincode`* (already a dependency, from
   `DURABILITY-TIER1`). Chosen — no new *runtime* dependency for framing
   itself; `bincode`'s existing use for the durability path establishes it
   as an already-justified choice for compact binary encoding. Verified as
   a real, compiling shape in a standalone probe (see "Validation plan").

**Concurrency/async runtime: `tokio` vs. a plain thread-per-connection
model.**

1. *`tokio` (async).* Considered — the conventional choice for a server
   handling many concurrent connections. Rejected for v1: it's a
   significant new dependency (and a viral one — async infects call-site
   signatures throughout) for a benefit (handling thousands of idle
   connections cheaply) this proposal has no evidence it needs yet; nothing
   in this crate's benchmark history suggests connection count, not
   store throughput, would be the bottleneck.
2. *`std::thread`-per-connection, each thread holding a reference into the
   same `Arc<ProductionStore>`/`Arc<GenericProductionStore>`.* Chosen —
   no new dependency, and it's the same pattern
   `CONCURRENCY-PROTOTYPES`/`PRODUCTION-DEFAULT` already validated
   (many threads, one shared `RwLock`-guarded store) applied to
   connections instead of benchmark worker threads. A real, measured
   connection-count ceiling (thread-per-connection has a real practical
   limit, unlike an async runtime) is a named, accepted limitation of this
   choice, not an unknown risk — see Consequences.

**Field addressing: string field names vs. integer tags.**

1. *String field names* (`"age"`, `"amount"`), resolved server-side via a
   `HashMap<String, FieldRef>` built from the domain's own marker types.
   Considered — more self-describing over the wire. Deferred, not rejected
   outright: it needs a real design for how a client discovers valid field
   names for a domain it didn't compile against (a schema-description
   RPC, itself new scope), which this proposal's non-goals exclude.
2. *Integer tags, assigned in a fixed, server-published order per domain
   at startup.* Chosen for v1 — no schema-description sub-protocol needed;
   a single Rust client crate compiled against the same domain type already
   knows the tag assignment at compile time (the same way it already knows
   `Order`'s field markers today).

### Proposed shape (validated as compiling Rust — see below)

```rust
enum Request {
    GetById { id: RecordId },
    FilterEq { field: FieldRef, value: ScanValue },
    ScanField { field: FieldRef },
    UpdateField { id: RecordId, field: FieldRef, value: ScanValue },
    Parent { id: RecordId },
    Children { id: RecordId },
    Neighbors { id: RecordId },
}

enum Response {
    Record { id: RecordId, fields: Vec<(FieldRef, ScanValue)> },
    RecordList { records: Vec<RecordId> },
    NotFound,
    NoParent,
    Ok,
    Err { code: u8, message: String },
}

// One shared trait the dispatch loop is generic over, implemented by a
// thin adapter around ProductionStore/GenericProductionStore — the
// dispatch loop itself never depends on which concrete store it's serving.
trait ConnectionStore {
    fn get(&self, id: RecordId) -> Option<Vec<(FieldRef, ScanValue)>>;
    fn filter_eq(&self, field: FieldRef, value: ScanValue) -> Vec<RecordId>;
    fn update(&self, id: RecordId, field: FieldRef, value: ScanValue) -> bool;
    // ScanField/Parent/Children/Neighbors follow the same shape.
}
```

Length-prefixed framing (4-byte little-endian length, then a `bincode`-
encoded `Request`/`Response`) over `std::net::TcpStream`, one OS thread per
connection, each thread taking `&self` on an `Arc<S: ConnectionStore>`
shared across all connection threads.

## Data/state and invariants

- No new persistent state. The server process wraps an existing durable
  store (`ProductionStore`/`GenericProductionStore`); it introduces no new
  on-disk format.
- Per-connection state is limited to the TCP stream and its read buffer —
  no session, no per-client cursor, no cross-request server-side state,
  matching the "no transaction semantics" non-goal directly.

## Errors, failure, recovery, and observability

- A connection whose request fails to parse (bad length prefix, truncated
  frame, unrecognized request tag) is answered with `Response::Err` once,
  then the connection is closed — never a panic, matching this crate's
  existing `Result`+`?` discipline (`AGENTS.md`).
- A client disconnecting mid-request is treated as an ordinary I/O error on
  that connection's thread; other connections and the shared store are
  unaffected — no shared state to poison, since the only shared state is
  the store itself, already `RwLock`-guarded.
- Out of scope for this proposal: structured logging/metrics, graceful
  shutdown/drain, and a health-check RPC — real gaps for anything beyond a
  local development/testing deployment, named here rather than silently
  assumed solved.

## Security, privacy, and compatibility

- **No authentication or authorization in this proposal** — named as a
  blocking gap for any deployment beyond localhost/trusted-network use, not
  a deferred nice-to-have. This is the single biggest reason this document
  proposes a design, not authorizes shipping a network-facing binary.
- No transport encryption (no TLS) — same status as authentication: a real
  gap for untrusted-network use, unaddressed by this proposal.
- Wire format is versioned implicitly by the fixed `Request`/`Response`
  enum shape; no explicit version negotiation exists in this proposal — a
  client and server built from different crate versions have no compat
  guarantee. A real revisit trigger if this ever needs mixed-version
  deployments (see below).

## Acceptance criteria

- A real client process, in a separate OS process from the server, can
  successfully `GetById`/`FilterEq`/`ScanField`/`UpdateField` against a
  running server wrapping `ProductionStore` (`Dog`) and, separately, one
  wrapping `GenericProductionStore<OrderProductionStack>` (`Order`), with
  results matching what an in-process call to the same store would return.
- `Parent`/`Children`/`Neighbors` are exercised against `Order`/`Customer`
  (the domain that already has real `Parent`/`Children` support) and, for
  `Neighbors`, against `Dog` (`littermate_of`) — covering both relation
  kinds `crate::generic`/`DogStore` already model.
- A concurrent-client stress test (many client processes/threads issuing
  interleaved reads and writes) shows no lost update and no torn read,
  reusing this crate's existing sequential-replay-linearizability
  verification pattern (`run_concurrency_stress_test`,
  `production_integration.rs`'s flagship test) rather than inventing a new
  verification method.

## Verification plan

- **Original proposal**: the `Request`/`Response`/`ConnectionStore`/framing
  shapes above were compiled (not just asserted) in a standalone, `std`-only
  scratch probe outside this repository, proving the types and the generic
  dispatch function type-check — the same "prove signatures compile"
  discipline `GENERIC-SCHEMA-DESIGN.md` used. This probe used a `u128`
  `RecordId` stand-in (no `uuid` dependency available in the throwaway
  probe) and a `DummyStore` implementation; it was not run, only
  type-checked (`rustc --edition 2021 --crate-type lib`), and is not part
  of this repository.
- **Real implementation, post-acceptance**: `SERVER-001`
  (`docs/specifications/server/SERVER-001-query-layer.md`) — real,
  compiled, tested code (`src/server/**`) against both `Dog` and
  `Order`/`Customer`, over a genuine `TcpListener`/`TcpStream` pair,
  including the flagship concurrent-client stress test named in
  "Acceptance criteria" above. See `docs/decisions/ADR-0010-server-query-layer-proposal.md`'s
  "Acceptance and implementation" section for the full account, including
  two real issues found and fixed along the way (a Nagle/delayed-ACK
  interaction, a test-isolation bug unrelated to the server itself).
  Throughput benchmarking (the "connection-count ceiling for the
  thread-per-connection model" question below) was not attempted this
  round — see `SERVER-001`'s own "Open questions."

## Traceability

`SERVER-001` (`docs/specifications/server/SERVER-001-query-layer.md`)
implements this design, registered once the design was accepted —
matching `GENERIC-SCHEMA-DESIGN`'s own precedent (no spec for the design
document itself; implementation tracked by `STORAGE-012`, here by
`SERVER-001`).

## Open questions

- Whether `bincode`'s existing crate version and encoding stability
  guarantees are adequate for a wire protocol (as opposed to its current
  use, on-disk durability within one process's own lifetime) is unverified
  by this pass — a real question for the implementation unit, not decided
  here.
- The thread-per-connection model's real connection-count ceiling is
  unmeasured (no benchmark exists yet) — named as an accepted, deliberate
  limitation of choosing it over `tokio`, not a proven-acceptable one.
- Whether a schema-description RPC (enabling string field names and
  non-Rust clients) is ever worth building is explicitly deferred to a
  future decision, not ruled out permanently.

## Change history

- 2026-08-31: Initial proposal, in response to the owner selecting
  "server/query layer" as the next direction from `docs/FUTURE-GROWTH.md`'s
  two named options.
