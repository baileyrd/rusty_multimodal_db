# Roadmap

Status vocabulary: `Proposed`, `Draft`, `Accepted`, `In Progress`,
`Implemented`, `Verified`, `Blocked`, `Deferred`, `Deprecated`,
`Superseded`.

| Unit | Outcome | Depends on | Specs | Exit gate | Status | Evidence |
|---|---|---|---|---|---|---|
| `BOOTSTRAP` | Charter, architecture, ADR-0001, ADR-0002, spec tree, roadmap, AGENTS.md/WORKFLOW.md/PROJECT-STATUS.md, Cargo scaffolding, CI | — | — | Docs merged, `cargo check` passes on empty scaffold | Implemented | this PR |
| `GENERATOR` | `DogRecord` + seeded, configurable dataset generator with unit tests | `BOOTSTRAP` | `STORAGE-001` | `cargo test` green for `src/generator.rs` | Implemented | this PR |
| `BACKENDS` | `DogStore` trait + `AosStore`/`SoaStore`/`CanonicalStore` with unit tests | `GENERATOR` | `STORAGE-002` | `cargo test` green for `src/store/**`, cross-backend equivalence tests pass | Implemented | this PR |
| `BENCH-SUITE` | Criterion suite: 4 workloads × 3 sizes × 3 backends, default features | `BACKENDS` | `STORAGE-003` | `cargo bench` completes all 36 cases | Implemented | this PR |
| `CACHE-MISS` | `perf-events`-gated cache-miss benchmark target, Linux-only | `BENCH-SUITE` | `STORAGE-003` | `cargo build --features perf-events` succeeds on Linux; ADR-0002 path documented | Implemented (build/feature only — real counter numbers deferred to a `baileyai` run, see `RESULTS.md`) | this PR |
| `RESULTS` | `RESULTS.md` with per-workload verdicts, explicit win/loss call-outs, open questions | `BENCH-SUITE`, `CACHE-MISS` | `STORAGE-004` | `RESULTS.md` merged, meets `STORAGE-004` acceptance criteria | Implemented | this PR |
| `HYBRID-BACKEND` | `CanonicalCachedStore` (canonical store + eager write-through age cache) closing `scan_ages`'s gap; ADR-0003; `RESULTS.md` revised to 4-way comparison | `RESULTS` | `STORAGE-005` | `cargo test`/`cargo bench` green for the 4th backend; `RESULTS.md` reports `scan_ages`/`update_age` specifically | Implemented | follow-on PR |
| `GRAPH-TRAVERSAL` | `littermate_of` edge generation; `DogStore::neighbors` (one-hop); ADR-0004; generic `two_hop_neighbors`; `neighbors_one_hop`/`neighbors_two_hop` benchmarks; `RESULTS.md`'s `## Graph traversal` section | `HYBRID-BACKEND` | `STORAGE-006` | `cargo test`/`cargo bench` green for `neighbors`/`two_hop_neighbors` across all 4 backends; `RESULTS.md` reports both workloads | Implemented | follow-on PR |
| `MIXED-WORKLOAD` | `MixedWorkloadDriver` (blended `get`/`update_age`/`scan_ages`, reusing `RoundRobin`); `mixed_workload_write{10,50,90}` benchmarks; `RESULTS.md`'s `## Mixed read/write workload` section, directly answering whether `CanonicalCachedStore` ever loses to `CanonicalStore`/`SoaStore` at any tested write ratio | `GRAPH-TRAVERSAL` | `STORAGE-007` | `cargo test`/`cargo bench` green for all 36 mixed-workload cases; `RESULTS.md` reports a verdict per (write ratio × size) | Implemented | follow-on PR |
| `DURABILITY-TIER1` | `CanonicalCachedStore`-only durability, five variants (WAL fsync, WAL buffered, snapshot rebuild, snapshot save-as-is, hybrid snapshot+WAL), built around a new shared `CanonicalCachedState` core; ADR-0005; `benches/durability.rs`'s per-write/checkpoint/load suite; `RESULTS.md`'s `## Durability` section | `MIXED-WORKLOAD` | `STORAGE-008` | `cargo test`/`cargo bench --bench durability` green for all 5 Tier 1 variants; `RESULTS.md` reports per-configuration numbers, the 1,000-write recoverability comparison, and an explicit recommendation | Implemented | follow-on PR |
| `DURABILITY-TIER2` | Three alternate durability architectures for `CanonicalCachedStore` (mmap-backed ages, LSM-tree-style with no compaction, `redb`-backed embedded engine), each scoped to the mutable `age` field only; ADR-0006; `RESULTS.md`'s `## Durability` section extended to cover all eight variants | `DURABILITY-TIER1` | `STORAGE-009` | `cargo test`/`cargo bench --bench durability` green for all 3 Tier 2 variants; `RESULTS.md` reports per-configuration numbers with the same rigor as Tier 1 | Implemented | follow-on PR |
| `CONCURRENCY-PROTOTYPES` | Four concurrent access strategies for `CanonicalCachedStore` (global `RwLock`, sharded locking, `dashmap`, actor/single-writer-thread), a new `ConcurrentStore` trait, a linearizability-style stress test, and a custom multi-threaded throughput harness; ADR-0007; `RESULTS.md`'s `## Concurrency` section | `DURABILITY-TIER2` | `STORAGE-010` | `cargo test` green for all four variants' flagship stress tests (16 threads × 2,000 iterations each, no lost updates/torn reads); `cargo bench --bench concurrency` completes the full size×ratio×thread-count×variant sweep; `RESULTS.md` reports a verdict per (size × write-ratio) configuration and an explicit recommendation | Implemented | follow-on PR |
| `PRODUCTION-DEFAULT` | `ProductionStore` (`src/production.rs`): `CanonicalCachedStore`'s architecture + mmap durability + global `RwLock` concurrency, consolidated into one recommended type implementing both `DogStore` and `ConcurrentStore`; ADR-0008; a flagship integration test exercising mmap durability and `RwLock` concurrency together for the first time; `src/lib.rs`/`README.md`/`docs/architecture/SYSTEM-ARCHITECTURE.md` reorganized to lead with it; `RESULTS.md`'s `## Production recommendation` section | `CONCURRENCY-PROTOTYPES` | `STORAGE-011` | `cargo test` green including the new flagship integration test; `cargo bench --bench workloads -- production` and `cargo bench --bench concurrency` report `ProductionStore` numbers; `RESULTS.md` ties all six prior rounds together | Implemented | follow-on PR |
| `GENERIC-SCHEMA-DESIGN` | A generic record/schema/query abstraction (`Record`/`IndexedField`/`ScannableField`/`SymmetricRelation`/`ChildOf` traits, composable store wrapper layers) validated against `Dog` and a second, structurally different domain (`Order`/`Customer` — directed relation, currency-like field, enum categorical field, timestamp field); ADR-0009 (**Accepted** — see `GENERIC-SCHEMA-LIBRARY` below); `docs/design/GENERIC-SCHEMA-DESIGN.md` | `PRODUCTION-DEFAULT` | — (no spec written for the design itself; implementation tracked by `STORAGE-012`) | Four validation spikes (`src/generic_spike/`) resolved every risk this design's §4 named, then `GENERIC-SCHEMA-LIBRARY` promoted it into a real library | Accepted/Implemented | this PR |
| `GENERIC-SCHEMA-LIBRARY` | Promotes `GENERIC-SCHEMA-DESIGN` into `crate::generic`: promoted traits/query/store (unchanged from four validation spikes), `Order`/`Customer` as the real reference implementation, `GenericMmapStore`/`GenericProductionStore` (generic mmap durability + `RwLock` concurrency — new beyond every spike), a flagship durability+concurrency integration test, a benchmark suite confirming no regression from spike to real code; ADR-0009 moved to Accepted; `STORAGE-012` | `GENERIC-SCHEMA-DESIGN` | `STORAGE-012` | `cargo test` green including the new flagship integration test (run 5× to rule out flakiness); `cargo bench --bench generic_production` completes and is reported in `RESULTS.md` alongside the spike rounds' numbers; no `src/production.rs`/`src/store/**`/`src/durability/**`/`src/concurrency/**` changes | Implemented | this PR |
| `SERVER-QUERY-LAYER-DESIGN` | A network server/query layer design in front of `ProductionStore`/`GenericProductionStore`: a `Request`/`Response` protocol (`GetById`/`FilterEq`/`ScanField`/`UpdateField`/`Parent`/`Children`/`Neighbors`), length-prefixed `bincode` framing, thread-per-connection dispatch reusing the existing `RwLock`-shared-store concurrency pattern; ADR-0010 (**Accepted** — owner approved as proposed); `docs/design/SERVER-QUERY-LAYER-DESIGN.md`; explicitly excludes authentication, transport encryption, transactions, and a query language | `GENERIC-SCHEMA-LIBRARY`, `PRODUCTION-DEFAULT` | — (no spec written for the design itself; a `SERVER-001` spec is registered by the implementation unit that follows, matching `GENERIC-SCHEMA-DESIGN`'s own precedent) | Request/response/dispatch shapes compiled in a standalone scratch probe (types only, not executed); owner reviewed and accepted the design and ADR-0010 without requesting changes | Accepted | this PR |
| `SERVER-QUERY-LAYER` | The real implementation of `SERVER-QUERY-LAYER-DESIGN`: `src/server/**` (`server` Cargo feature, off by default), `Dog`/`Order`-`Customer`/`Employee` domain adapters, a minimal server binary (`dog_server`), real end-to-end tests over a genuine socket including a flagship concurrent-client stress test, schema discovery (`DescribeSchema`/`ADR-0011`, v0.2.0), a third validation domain (`Employee`, v0.3.0 — the first with both `Parent`/`Children` and `Neighbors` real, which found and fixed a real `Reversed`/`Neighbors`-forwarding gap in `crate::generic`), a throughput/latency benchmark (`benches/server.rs`, v0.4.0), two real-hardware follow-ups retuning `benches/server.rs`'s `THREAD_COUNTS` (`Beast`, 24 cores; `baileyai`, 32 cores) that together resolve the thread-per-connection model's connection-count-ceiling open question, and a schema-driven client library (`server::client::SchemaDrivenClient`, v0.5.0 — a real, reusable, name-addressed client built purely from `DescribeSchema`, closing the "no schema-driven client library exists" gap); `SERVER-001` | `SERVER-QUERY-LAYER-DESIGN` | `SERVER-001` | `cargo test --features server` and `cargo test --features server,research` both green, including the flagship stress test, all three schema-driven tests, and the new client's own four-test integration suite; `cargo test`/`cargo test --all-features` unaffected (the `server` module adds no default-build surface); `cargo fmt`/`clippy`/`check` clean; no `src/production.rs`/`src/store/**`/`src/durability/**`/`src/concurrency/**` changes (the one exception, `src/generic/{store,production}.rs`'s `Neighbors`-forwarding completion, is named in `SERVER-001`'s own Non-goals); `cargo bench --features server,research --bench server` completes, numbers in `RESULTS.md` | Implemented | PR #35, #37, #39, #41, #43, #44, #46 |
| `SERVER-AUTH-DESIGN` | A design for authentication and coarse read/write authorization on the server/query layer: a new `Request::Authenticate` request, two static token classes (`ReadOnly`/`ReadWrite`), constant-time comparison via the `subtle` crate, and an explicit non-goal (native transport encryption, deferred to an external TLS-terminating proxy/tunnel); ADR-0012 (**Accepted** — owner approved as proposed); `docs/design/SERVER-AUTH-DESIGN.md`; explicitly does not, by itself, authorize deploying a server beyond a trusted network — closes only the access-control half of ADR-0010's own named gap, not the encryption half | `SERVER-QUERY-LAYER` | — (no spec written for the design itself; `SERVER-AUTH` below registers the implementation against `SERVER-001`, matching `SERVER-QUERY-LAYER-DESIGN`'s own precedent) | Design document written, incremental additions to `SERVER-001`'s already-compiling `protocol.rs`/`mod.rs` shapes (no standalone scratch probe built — see ADR-0012's own "Validation and revisit triggers" for why); owner reviewed and accepted the design and ADR-0012 without requesting changes | Accepted | PR #48, #50 |
| `SERVER-AUTH` | The real implementation of `SERVER-AUTH-DESIGN`: `AuthConfig`/`TokenClass` (`src/server/mod.rs`), `Request::Authenticate`/`ErrorCode::{Unauthenticated,Unauthorized}` (`src/server/protocol.rs`), constant-time token comparison via `subtle` (this crate's first new server-layer dependency), every `serve` call site updated (`AuthConfig::default()` everywhere except `src/bin/dog_server.rs`'s `AuthConfig::from_env()`), real end-to-end coverage (`tests/server_auth_integration.rs`) plus a timing measurement for the constant-time comparison claim; `SERVER-001` v0.6.0 | `SERVER-AUTH-DESIGN` | `SERVER-001` | `cargo test --features server` green including `tests/server_auth_integration.rs`'s five tests and `src/server/mod.rs`'s new `AuthConfig`/timing unit tests; `cargo test`/`cargo test --all-features` unaffected; `cargo fmt`/`clippy`/`check`/`doc` clean; no pre-existing `serve` call site's behavior changed (`AuthConfig::default()` reproduces the original unauthenticated behavior exactly, `AUTH-FR-007`); every `SERVER-AUTH-DESIGN.md` functional acceptance criterion verified over a real socket | Implemented | PR #52, #53 |
| `SERVER-TRANSACTION-DESIGN` | A design for atomic multi-operation transactions on the server/query layer: a new `Request::Transaction { updates }` request batching `UpdateField`-shaped writes, validate-then-apply atomicity (all-or-nothing, no undo log needed given this crate's own "no runtime deletion" invariant), isolation from concurrent access via a new minimal storage-layer critical-section primitive reusing each store's existing lock (no new lock at the server layer), and explicit non-goals (a multi-round-trip interactive session, crash-atomicity across a batch — both named directly, not left implicit); ADR-0013 (**Accepted** — owner approved as proposed); `docs/design/SERVER-TRANSACTION-DESIGN.md`; explicitly does not deliver ACID transactions — atomicity/isolation with respect to concurrent access only, not crash-atomicity | `SERVER-AUTH` | — (no spec written for the design itself; `SERVER-TRANSACTION` below registers the implementation against `SERVER-001`/`STORAGE-011`/`STORAGE-012`, matching `SERVER-AUTH-DESIGN`'s own precedent) | Design document and ADR written, incremental protocol additions to `SERVER-001`'s already-compiling shapes plus one new, real storage-layer primitive (flagged honestly as a bigger footprint than `SERVER-AUTH`'s purely server-layer-additive implementation); owner reviewed and accepted the design and ADR-0013 without requesting changes | Accepted | PR #54, #55, #56 |
| `SERVER-TRANSACTION` | The real implementation of `SERVER-TRANSACTION-DESIGN`: `Request::Transaction`/`TransactionOp`/`Response::TransactionFailed`/`ErrorCode::RecordNotFound` (`src/server/protocol.rs`), `ConnectionStore::apply_transaction` (`src/server/mod.rs`, plus a per-adapter implementation in `dog.rs`/`order.rs`/`employee.rs`), and the storage-layer critical-section primitive it depends on — `crate::production::TransactionalStore` (`STORAGE-011` v0.2.0) and `crate::generic::production::GenericProductionStore::with_exclusive` (`STORAGE-012` v0.3.0); `SERVER-001` v0.7.0 | `SERVER-TRANSACTION-DESIGN` | `SERVER-001`, `STORAGE-011`, `STORAGE-012` | `cargo test --features server` green including `tests/server_transaction_integration.rs`'s six tests (full success, failure at each position, malformed value, `ReadOnly` rejection, flagship concurrent stress test) and `src/server/mod.rs`'s new dispatch/protocol unit tests; `cargo test`/`cargo test --all-features` unaffected; `cargo fmt`/`clippy`/`check`/`doc` clean; every `SERVER-TRANSACTION-DESIGN.md` functional acceptance criterion verified over a real socket; no `src/store/**`/`src/durability/**`/`src/concurrency/**` changes, `src/production.rs`/`src/generic/production.rs` changes limited to the one new critical-section primitive each | Implemented | this PR |
| `SERVER-TRANSACTION-BENCHMARK` | Bounded, additive extension of `SERVER-QUERY-LAYER`'s own throughput/latency benchmark (`benches/server.rs`, FR-014) to cover `Request::Transaction`: a directly-comparable `{domain}-txn` row set (`measure_transaction_latency`/`run_transaction_throughput`/`bench_transaction_domain`), same environment/pool/thread-count sweep as the existing `GetById` measurement; `SERVER-001` v0.8.0 (`SERVER-001-FR-018`); no ADR — completes already-accepted FR-014 scope, not a new decision | `SERVER-TRANSACTION` | `SERVER-001` | `cargo bench --features server,research --bench server` completes without panics, prints `{domain}-txn` rows for all three domains alongside the existing `GetById` rows, numbers in `RESULTS.md`'s `## Server / query layer` section; `cargo test --all-features`/`cargo test` both unaffected (bench-only change); `cargo fmt`/`clippy`/`check`/`doc` clean; finding: no meaningful latency or throughput cost relative to `GetById` at this benchmark's scale, on this session's shared container | Implemented | this PR |

## Sequencing notes

This roadmap is intentionally linear (each unit depends on the previous)
because the task specifies this exact working order: "bootstrap → dataset
generator → trait + three backends (with unit tests) → benchmark suite →
run it → write up results," with an explicit check-in point before the
benchmark run if the record shape, workload list, or dataset sizes need to
change. `CACHE-MISS` runs alongside `BENCH-SUITE` rather than strictly
after it in practice (they land in the same PR), but is listed separately
because it has its own exit gate (ADR-0002 compliance) distinct from the
default suite's.

`HYBRID-BACKEND` was itself an out-of-scope item from the original
roadmap, promoted to an actual unit once `RESULTS.md`'s first pass made
the case for it concrete (`scan_ages` losing to both baselines). That's
the expected lifecycle for this section: an item here becomes a real unit
when a finding motivates it, not on a fixed schedule.

`GRAPH-TRAVERSAL` follows the same lifecycle: it was the untested third
leg of the original row/column/graph hypothesis, promoted once a real
edge relationship (`littermate_of`) and 1-2 hop traversal became the
concrete, scoped next step per the task that motivated it — not a general
graph-query-layer unit (see ADR-0004 and `STORAGE-006`'s non-goals).

`MIXED-WORKLOAD` follows the same lifecycle again: "mixed read/write
workload benchmarking" was an explicit out-of-scope item on this roadmap
until it became the concrete next step for testing whether
`HYBRID-BACKEND`'s eager write-through tax (ADR-0003) changes the
recommendation once reads and writes are actually blended — it doesn't
(see `RESULTS.md`), so no ADR revision followed, per `STORAGE-007`'s own
scoping.

`DURABILITY-TIER1`/`DURABILITY-TIER2` are the first units in this
roadmap not motivated by a prior finding — every workload above them is
purely in-memory, and durability was always the next open question once
`CanonicalCachedStore` was settled as the recommendation. Split into two
units (rather than one) to match the two rigor levels the motivating task
specified: `DURABILITY-TIER1` covers five fully-tested, fully-benchmarked
WAL/snapshot/hybrid variants (ADR-0005); `DURABILITY-TIER2` covers three
lighter proof-of-concept alternate architectures (ADR-0006), explicitly
not held to the same production-hardening bar. Both apply to
`CanonicalCachedStore` only, per the same "don't build for backends
nobody would deploy" reasoning `HYBRID-BACKEND` established for choosing
which backend gets new capability.

`CONCURRENCY-PROTOTYPES` follows `DURABILITY-TIER1`/`DURABILITY-TIER2` as
the next previously-out-of-scope axis: every workload up to and including
durability assumes single-threaded access, and reader/writer safety was
the natural next open question once `CanonicalCachedStore` had both a
recommendation and a durability story. Deliberately kept as one unit,
unlike the durability split, because the task's own two-tier rigor split
(three full-rigor Tier 1 variants, one lighter Tier 2 variant) fit inside
a single roadmap entry without needing a second one — and deliberately
*not* combined with `DURABILITY-TIER1`/`DURABILITY-TIER2` into a
"concurrent durable store" unit, since the motivating task named that
combination as a distinct, later follow-up round rather than part of this
one (see ADR-0007's Context).

`PRODUCTION-DEFAULT` is that distinct follow-up round, once it actually
arrived: six rounds of empirical work — row/column/graph,
`DURABILITY-TIER1`/`TIER2`, and `CONCURRENCY-PROTOTYPES` (measured three
times, across a container, a Windows desktop, and `baileyai`) — had all
converged on the same combination (`CanonicalCachedStore` architecture +
mmap + global `RwLock`), so this unit's job was to wire that one
already-justified combination together, verify it as a composed stack for
the first time, and make it the crate's documented default — not to
re-derive or re-benchmark any of the three picks, and not to build a
general "any concurrency strategy × any durability variant" combinatorial
matrix (see ADR-0008's Context and Non-goals in `STORAGE-011`).

`GENERIC-SCHEMA-DESIGN` follows `PRODUCTION-DEFAULT` as the natural next
question once the recommendation was fully realized on `Dog`: should the
crate's schema/query surface generalize beyond one domain at all, and if
so, to what shape? Deliberately staged as design-only, not an
implementation unit — the motivating task named this the most
hard-to-reverse decision the project has faced (the abstraction would
become the crate's public API surface), and required stopping for review
before any implementation code, per this project's own working-style
convention of checking in before large or hard-to-reverse changes. Its
"Depends on" `PRODUCTION-DEFAULT` reflects that the recommendation needed
to exist and be settled before it was worth asking whether it should
generalize, not that the design touches `ProductionStore` itself (it
doesn't — see ADR-0009 and the design doc's own Context).

`GENERIC-SCHEMA-LIBRARY` follows once every risk `GENERIC-SCHEMA-DESIGN`'s
own §4 named had been individually resolved: four validation spikes (kept
as historical record in `src/generic_spike/`, not deleted) tested the
design against real code and real benchmarks — Dog-overhead measurement,
an associated-type-ambiguity diagnosis and fix, macro-generated per-marker-
pair forwarding, and directed-relation-generalization measurement — before
any of it was treated as accepted. This unit promotes that validated
design into `crate::generic`, a real public library, and builds the piece
none of the four spikes attempted: a generic equivalent of `ProductionStore`
(`GenericMmapStore`/`GenericProductionStore`), verified by a flagship
durability-plus-concurrency integration test on `Order`/`Customer` — the
same bar `PRODUCTION-DEFAULT` set for `Dog`. Deliberately does *not*
follow the original design doc's §5 staged migration literally (port `Dog`
onto the generic core as a third validation domain, only then build a
production store) — `Dog` stays a benchmark fixture, not a target domain,
so `Order`/`Customer` became the real reference implementation directly;
see ADR-0009's "Acceptance and implementation" section for why this
deviation was made rather than followed as originally written.

`SERVER-QUERY-LAYER-DESIGN` follows `GENERIC-SCHEMA-LIBRARY` as this
project's first roadmap unit motivated by `docs/FUTURE-GROWTH.md` rather
than by a benchmark finding — the owner chose it over the other named
direction (SQLite/DuckDB-tier parity) when asked which of the two, if
either, to pursue after the project reached a fully-`Implemented` roadmap
with no unit in progress. Deliberately staged as design-only, matching
`GENERIC-SCHEMA-DESIGN`'s own precedent: a network-facing protocol is a
comparably hard-to-reverse public-surface decision, and this unit stopped
for owner review before any server implementation code was written. The
owner reviewed and accepted the design as proposed (ADR-0010, Accepted).
`SERVER-QUERY-LAYER` (below) is the real implementation that followed,
registering `SERVER-001`, matching how `STORAGE-012` followed
`GENERIC-SCHEMA-DESIGN`'s own acceptance. Its
"Depends on" `GENERIC-SCHEMA-LIBRARY`/`PRODUCTION-DEFAULT` reflects that
both store types it wraps (`ProductionStore`, `GenericProductionStore`)
need to already exist, not that either is modified by this design.

`SERVER-QUERY-LAYER` follows `SERVER-QUERY-LAYER-DESIGN` once the owner
accepted it: the real `src/server/**` implementation, behind a new
`server` Cargo feature kept deliberately separate from `research` (new,
additive capability, not a benchmarked alternative). Validated against
both domains the design's own acceptance criteria named — `Dog`
(`Neighbors`, a real symmetric relation) and `Order`/`Customer`
(`Parent`/`Children`, a real directed relation) — over a genuine
`TcpListener`/`TcpStream` pair, not just `dispatch`'s in-process logic.
Surfaced and fixed two real issues along the way, both documented in
`docs/PROJECT-STATUS.md`'s own entry for this unit: a Nagle/delayed-ACK
interaction that made every request/response round trip cost ~40ms until
`TCP_NODELAY` was set, and a test-isolation bug (two integration tests
racing on the same mmap-backed temp path) unrelated to the server itself.
Deliberately does not include a throughput benchmark (`benches/server.rs`)
— this round's acceptance criteria are correctness, not performance; see
`SERVER-001`'s own "Open questions."

`SERVER-QUERY-LAYER`'s own follow-up round added schema discovery
(`DescribeSchema`/`Response::Schema`, ADR-0011, `SERVER-001` v0.2.0) —
closing the one item ADR-0010's original design explicitly deferred
rather than rejected. The owner picked this from a short list of concrete
next directions offered after the initial implementation landed. Bounded
and additive (field *tags* stay the addressing scheme; this only adds
runtime discovery of names/capabilities), so it followed this project's
more common ADR-and-implementation-together cadence rather than the
design-only-first treatment `SERVER-QUERY-LAYER-DESIGN` itself got —
see ADR-0011's own Context for why that distinction was made explicitly,
not just assumed.

A second follow-up round added a third validation domain, `Employee`
(`server::employee`, `SERVER-001` v0.3.0), again from the same short list
of next directions and again treated as bounded/additive rather than a
new design-review gate — this domain doesn't change the protocol, the
framing, or the concurrency model, only adds a third adapter and (as it
turned out) completes existing `crate::generic` capability. `Employee` was
purpose-built (unlike `Order`/`Customer`, which had an external reference
domain) specifically to combine `SymmetricRelation` and `ChildOf` on one
self-referential record type — a combination ADR-0009's own "revisit if"
bullet named as untested. It found a real gap: `Reversed` (the
`ChildOf`-forwarding wrapper) never forwarded `Neighbors`, so no domain
stacking `Symmetric` beneath `Reversed` could reach a symmetric relation
from outside the stack. Fixed directly in `src/generic/{store,production}.rs`
(the forwarding impl plus a new `GenericProductionStore::neighbors`
method) — recorded as an addendum to already-Accepted ADR-0009, per the
same "completion of accepted capability, not a new decision" treatment
`GENERIC-SCHEMA-WRITE-THROUGH-FIX` established, not a new ADR. `Employee`
is the first `ConnectionStore` domain adapter where every relation-kind
request (`Parent`/`Children`/`Neighbors`) is a real operation, none
`Unsupported` — verified both at the `crate::generic` layer (a durable
flush-plus-reopen test) and over the real wire protocol
(`tests/server_employee_integration.rs`).

A third follow-up round closed the throughput/latency gap `SERVER-001`'s
own "Open questions" had named since its v0.1.0 acceptance criteria —
the "2" in the owner's "3 then 2" ("this domain, then a throughput
benchmark next"). `benches/server.rs` (`SERVER-001` v0.4.0) is a custom,
non-Criterion harness matching `benches/concurrency.rs`'s own shape (a
`Barrier`-synchronized thread sweep, aggregate ops/sec from the slowest
thread), the first benchmark in this crate to put a real
`TcpListener`/`TcpStream` pair in its timed path rather than measuring
`dispatch` in-process or a real socket only for pass/fail correctness.
Measures single-connection `GetById` round-trip latency and aggregate
throughput under a 1/4/8/16-thread sweep, across all three domains.
Headline finding: at this record-count scale, the network/framing
layer's own cost (~37 µs per round trip, this session's container) so
thoroughly dominates any per-domain in-process operation cost (tens to
low-hundreds of nanoseconds, per `RESULTS.md`'s existing sections) that
all three domains' numbers land in the same band — see `RESULTS.md`'s
`## Server / query layer` section for the full account and its own
caveats (this session's 4-core container bounds the thread-count sweep;
a real-hardware follow-up would be needed for a real per-core ceiling,
matching `## Concurrency`'s own established container-then-real-hardware
precedent).

A real-hardware follow-up round closed that remaining caveat: run
directly on the owner's Windows dev machine (`Beast`, confirmed via
`hostname`/`whoami`/`nproc` — 24 real cores, not a container),
`benches/server.rs`'s `THREAD_COUNTS` retuned from `[1, 4, 8, 16]` to
`[1, 4, 24, 48]` (the same array `benches/concurrency.rs` already
established for this machine). Throughput genuinely keeps climbing from
4 to 24 threads on real hardware — confirming the container's flat
4-through-16 plateau was that environment's own ceiling, not the
model's — then flattens or degrades at 48 (2× cores, deliberate
oversubscription), the same signature `## Concurrency`'s own
real-hardware passes already documented. See `RESULTS.md`'s real-hardware
subsection for the full tables and one honest, unexplained surprise this
pass surfaced (this machine's peak throughput running lower than the
container's own plateau).

A second real-hardware follow-up round, run directly on `baileyai` itself
(32 real cores, no SSH substitution needed this time), retuned
`THREAD_COUNTS` again, from `Beast`'s `[1, 4, 24, 48]` to
`[1, 4, 32, 64]` (matching `benches/concurrency.rs`'s own `baileyai`
sweep). Same qualitative shape as the `Beast` pass — throughput climbs
strongly from 4 to 32 threads, then flattens to mildly negative at 64
(2× cores) — confirming the ceiling again sits at or near real core
count. This pass also resolved the `Beast` pass's own open surprise:
`baileyai`'s peak throughput (over 1M ops/sec) is far higher than both
the container's and `Beast`'s own plateaus, and its latency is a third
to a quarter of either — real dedicated Linux hardware genuinely is
faster end to end, meaning `Beast`'s Windows loopback-TCP/scheduling
stack was the actual outlier, not the container. See `RESULTS.md`'s
second real-hardware subsection for the full tables.

A fourth follow-up round, picked from the same short list of concrete
next directions offered once "3 then 2" and both real-hardware passes
were done, closed a different named gap: `PROJECT-STATUS.md`'s own
"no schema-driven client *library* exists" item. `server::client::
SchemaDrivenClient` (`SERVER-001` v0.5.0) promotes the one-off logic
each domain's own schema-driven integration test had reimplemented by
hand (FR-011/FR-013) into real, reusable API — `connect` sends
`Request::DescribeSchema` first and keeps the result, and every
subsequent method is addressed by field *name*, never a domain's own
`FIELD_*` constant. Capability checks (`filter_eq`/`scan`/`update`
against `FieldCapabilities`, `parent`/`children`/`neighbors` against
`RelationCapabilities`) run client-side first, matching what the schema
already reports, rather than paying a round trip to learn what a typed
`Response::Err` would have said anyway. Bounded and additive — no new
`Request`/`Response` variant, no wire-format change — so this followed
ADR-0011's own bounded/additive-cadence precedent rather than opening a
new ADR: it completes an already-accepted decision (schema discovery)
rather than deciding something new. Verified against all three domains
in one test file (`tests/server_schema_driven_client.rs`) that imports
no domain-specific `FIELD_*` constant at all, proving the promotion
didn't quietly reintroduce compile-time domain knowledge through the
test's own back door.

`SERVER-AUTH-DESIGN` picks up the second of the owner's three directions
— authentication/encryption — and, unlike the client library, did not
ship straight to implementation: `docs/FUTURE-GROWTH.md` names this
"genuinely new," not incremental, the same category ADR-0010 itself was
in before `SERVER-QUERY-LAYER-DESIGN` was written, so this followed that
exact same design-first, stop-for-review treatment. ADR-0012 (Accepted)
and `docs/design/SERVER-AUTH-DESIGN.md` propose a shared-secret token
(`Request::Authenticate`), checked once per connection before any other
request is served, with two coarse classes (`ReadOnly`/`ReadWrite`) and
a constant-time comparison (a new, narrow dependency, `subtle` — the one
place this design departs from this project's usual "avoid new
dependencies" posture, deliberately, since hand-rolling constant-time
comparison is a well-known place DIY security code silently regresses).
**Explicitly, this design does not close the transport-encryption half
of the gap** — it requires pairing with an external TLS-terminating
proxy/tunnel for any non-localhost deployment, native TLS (`rustls`)
named as a real, larger follow-up rather than added by default, matching
the same dependency-weight reasoning ADR-0010 used to defer `tokio` and
gRPC. The owner reviewed and accepted the design as proposed, with no
changes requested.

`SERVER-AUTH` is that implementation unit, following how `SERVER-QUERY-LAYER`
followed `SERVER-QUERY-LAYER-DESIGN`'s own acceptance: `SERVER-001` v0.6.0
(`SERVER-001-FR-016`) implements `SERVER-AUTH-DESIGN` essentially as
proposed, no design decision reopened. `AuthConfig::new`/`AuthConfig::from_env`
resolve the design's own left-open "exact environment-variable naming"
question (`SERVER_AUTH_READ_ONLY_TOKEN`/`SERVER_AUTH_READ_WRITE_TOKEN`);
every pre-existing `serve` call site now passes `AuthConfig::default()`
(the one exception, `src/bin/dog_server.rs`, uses `::from_env()` so the
real binary can opt in) and required no other change — direct evidence
for `AUTH-FR-007`'s backward-compatibility claim, not just an assertion of
it. The one acceptance criterion needing empirical rather than
read-through evidence — that constant-time comparison actually holds — is
measured directly against `AuthConfig::check` (mean latency over 20,000
iterations, first-byte-mismatch vs. last-byte-mismatch tokens) rather than
over a real TCP round trip: network jitter at the millisecond scale would
swamp a signal this small, so a network-level timing test would pass
regardless of whether the comparison were actually constant-time — no real
evidence at all.

`SERVER-TRANSACTION-DESIGN` picks up the third and last of the owner's
three directions — session/transaction semantics — and, like
`SERVER-AUTH-DESIGN` before it, did not ship straight to implementation:
`docs/FUTURE-GROWTH.md` names this "genuinely new" too, so it got the
same design-first, stop-for-review treatment. ADR-0013 (**Accepted**)
and `docs/design/SERVER-TRANSACTION-DESIGN.md` propose the smallest real
slice of what `docs/FUTURE-GROWTH.md`'s broader "session/transaction
semantics across multiple requests" framing names: one new request kind,
`Request::Transaction`, batching several `UpdateField`-shaped writes into
one all-or-nothing operation, isolated from concurrent connections by
holding each store's existing internal lock for the whole batch instead
of once per write — no second lock introduced, matching ADR-0010's own
"no new lock at this layer" principle. **Deliberately does not propose
the literal multi-round-trip "session" half of the framing** — a
`BeginTransaction`/several-requests/`Commit` design would hold a
connection's exclusive lock open across an unbounded number of client
round trips, a real liveness/denial-of-service risk this project has
never accepted anywhere else in the server layer; named as a real
revisit trigger, not ruled out forever. **Also does not deliver crash-
atomicity** — a process crash between two of a batch's writes landing on
stable storage can leave a partial batch durably applied; this proposal
delivers atomicity/isolation with respect to concurrent access only, a
real but narrower guarantee than a full ACID transaction. Honestly
flags a real cost beyond `SERVER-AUTH`'s own purely server-layer-
additive implementation: the atomicity mechanism needs a new, minimal
primitive on the storage layer itself (`ProductionStore`/
`GenericProductionStore`), already-accepted and "closed" modules — the
second time this project would deliberately reopen them by design,
after the `Employee` round's real `Neighbors`-forwarding fix found a gap
there by accident. The owner reviewed and accepted the design as
proposed, with no changes requested.

`SERVER-TRANSACTION` is that implementation unit, following how
`SERVER-AUTH` followed `SERVER-AUTH-DESIGN`'s own acceptance:
`SERVER-001` v0.7.0 (`SERVER-001-FR-017`) implements
`SERVER-TRANSACTION-DESIGN` essentially as proposed, no design decision
reopened. The storage-layer critical-section primitive the design named
as a real cost up front landed exactly as scoped: `crate::production::TransactionalStore`
(`STORAGE-011` v0.2.0, `Dog`) and `crate::generic::production::GenericProductionStore::with_exclusive`
(`STORAGE-012` v0.3.0, `Order`/`Employee`) each let a caller hold the
store's existing lock across multiple logical operations, purely
additive — no existing `ProductionStore`/`GenericProductionStore` method's
behavior changed, verified by diff. `tests/server_transaction_integration.rs`'s
flagship concurrent test uses sequential-replay linearizability (the same
established pattern `SERVER-001-FR-009` uses), not a per-instant "never
observed half-written" check — that approach was tried first and dropped
once it became clear two independent, un-synchronized client round trips
can't distinguish a real atomicity bug from legitimately straddling two
different, fully-completed transactions, since this protocol has no
multi-field read to observe two fields at one consistent instant; the
single-threaded before/after tests prove batch atomicity deterministically
instead, with no such confound.

`SERVER-TRANSACTION-BENCHMARK` closes the one open follow-up question
`SERVER-TRANSACTION`'s own implementation left implicit: does
`Request::Transaction`'s longer-held lock — the real, named cost
`SERVER-TRANSACTION-DESIGN`'s own "Architecture" section flagged up
front — actually show up in the numbers? `benches/server.rs` (`SERVER-001`
v0.8.0, `SERVER-001-FR-018`) was extended with a directly-comparable
`{domain}-txn` row set, same environment/pool/thread-count sweep as the
existing `GetById` measurement, no ADR needed (bounded, additive
completion of already-accepted `FR-014` scope, matching how `SERVER-001`
v0.5.0's client library and the two real-hardware follow-ups were each
reasoned about). The answer: no meaningful latency or throughput cost was
found relative to plain `GetById`, on this session's shared container —
the longer-held lock is real in principle but swamped by the same
network/framing cost `SERVER-QUERY-LAYER`'s own original benchmark pass
already identified as this whole harness's dominant cost at this
record-count scale. See `RESULTS.md`'s `## Server / query layer`,
`### \`Request::Transaction\` follow-up` subsection for the full numbers.

## Out of scope for this roadmap (see architecture doc "where this can go
next")

- A fifth backend/mode implementing lazy (dirty-flag) cache invalidation,
  as an alternative to `HYBRID-BACKEND`'s eager write-through — see
  ADR-0003's revisit triggers. `MIXED-WORKLOAD` tested the scenario most
  likely to motivate this (a write-heavy blended workload) and found no
  crossover, so this remains out of scope, not promoted.
- General N-hop (3+) or typed/weighted-edge graph traversal — `GRAPH-TRAVERSAL`
  scoped this to one relationship type (`littermate_of`) and 1-2 hops only;
  see ADR-0004 and `STORAGE-006`'s open questions.
- Memory-overhead-per-backend measurement.
- Dataset sizes beyond 1M.
- Write ratios beyond 90%, and bursty/correlated (as opposed to
  independently-drawn) mixed-workload access patterns — see
  `STORAGE-007`'s open questions.
- LSM-tree compaction — `DURABILITY-TIER2`'s `LsmStore` explicitly ships
  without it, per the motivating task's own instruction to scope this
  down rather than let it become its own multi-day project; see
  ADR-0006 and `RESULTS.md`'s durability open questions.
- Durability for `AosStore`/`SoaStore`/`CanonicalStore` — both
  `DURABILITY-TIER1` and `DURABILITY-TIER2` apply to `CanonicalCachedStore`
  only, per ADR-0005's Context.
- Extending any durability variant to persist more than `age` (i.e. full
  record durability for the Tier 2 mmap/`redb` variants, which currently
  persist only the mutable field) — see ADR-0006's revisit triggers.
- An explicit physical-disk (`fsync`) guarantee on `checkpoint()` for the
  Tier 1 variants whose `checkpoint` doesn't currently call `sync_all` —
  see ADR-0005's Consequences and revisit triggers.
- Batching multiple writes into one `redb` transaction — `RedbStore`
  commits one transaction per `update_age` call; see ADR-0006.
- A concurrency strategy paired with a specific durability variant (e.g.
  a sharded store where each shard also owns its own WAL) — named
  explicitly by the motivating task as a distinct future round, not part
  of `CONCURRENCY-PROTOTYPES`; see ADR-0007's Context. **One specific
  instance of this — global `RwLock` + mmap — is now implemented as
  `PRODUCTION-DEFAULT`'s `ProductionStore`**; every *other* pairing (e.g.
  sharded + a WAL variant) remains out of scope, per `STORAGE-011`'s own
  Non-goals.
- `ShardedStore`'s shard count swept against alternatives to its fixed
  `SHARD_COUNT = 64` — see `RESULTS.md`'s concurrency open questions.
- Scaling `ActorStore` beyond one owning thread (e.g. sharding the actors
  themselves) — its measured throughput ceiling is a structural property
  of the single-writer-thread pattern this pass didn't attempt to lift;
  see ADR-0007's revisit triggers.
- `same_breed`/`neighbors` support for the sharded-locking and `dashmap`
  concurrency variants — both cover only `get`/`update_age`/`scan_ages`,
  the `ConcurrentStore` trait's scope; see ADR-0007's Considered options
  and `STORAGE-010`'s Non-goals.

These may become future roadmap units if `RESULTS.md`'s open questions
motivate them, but are not committed to now.
