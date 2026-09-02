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
| `SERVER-TRANSACTION-BENCHMARK` | Bounded, additive extension of `SERVER-QUERY-LAYER`'s own throughput/latency benchmark (`benches/server.rs`, FR-014) to cover `Request::Transaction`: a directly-comparable `{domain}-txn` row set (`measure_transaction_latency`/`run_transaction_throughput`/`bench_transaction_domain`), same environment/pool/thread-count sweep as the existing `GetById` measurement; `SERVER-001` v0.8.0 (`SERVER-001-FR-018`); no ADR — completes already-accepted FR-014 scope, not a new decision | `SERVER-TRANSACTION` | `SERVER-001` | `cargo bench --features server,research --bench server` completes without panics, prints `{domain}-txn` rows for all three domains alongside the existing `GetById` rows, numbers in `RESULTS.md`'s `## Server / query layer` section; `cargo test --all-features`/`cargo test` both unaffected (bench-only change); `cargo fmt`/`clippy`/`check`/`doc` clean; finding: no meaningful latency or throughput cost relative to `GetById` at this benchmark's scale, on this session's shared container | Implemented | PR #59 |
| `SERVER-TLS-DESIGN` | A design for native transport encryption on the server/query layer: TLS termination inside this crate's own server process via `rusty_tls` (`Rusty-Mill/rusty_mill`, this owner's own ecosystem-wide `rustls` wrapper — not a direct `rustls` dependency, not an external proxy/tunnel), an opt-in `TlsConfig` mirroring `AuthConfig`'s own shape, a per-connection stream abstraction (`rusty_tls::TlsServerStream`) so `handle_connection`/`send_response` work uniformly whether or not TLS is active — `src/server/framing.rs` needs zero changes, already generic over `Read`/`Write`; ADR-0014 (**Accepted** — owner approved as revised); `docs/design/SERVER-TLS-DESIGN.md`; explicitly does not add mTLS (client identity remains `AuthConfig`'s existing token scheme) | `SERVER-AUTH`, `SERVER-TRANSACTION-BENCHMARK` | — (no spec written for the design itself; `SERVER-TLS` below registers the implementation against `SERVER-001`, matching `SERVER-AUTH-DESIGN`'s own precedent) | Design document and ADR written, incremental additions to `SERVER-001`'s already-compiling `protocol.rs`/`mod.rs` shapes (no standalone scratch probe built — see ADR-0014's own "Validation and revisit triggers" for why); owner reviewed and accepted the design and ADR-0014 as revised, no further changes requested | Accepted | PR #61, #62, #63 |
| `SERVER-TLS` | The real implementation of `SERVER-TLS-DESIGN`: `TlsConfig`/`TlsConfigError` (`src/server/mod.rs`), a new `ReadHalf`/`WriteHalf` enum pair (`Plain`'s existing `TcpStream::try_clone` split unchanged; `Tls` shares one `Rc<RefCell<rusty_tls::TlsServerStream<TcpStream>>>` between both halves — a real implementation-time finding, since `rustls`' connection state can't be split the way a raw socket can), a small hand-written PEM/base64 decoder (`src/server/pem.rs`, new, not a new dependency), `serve`'s new `tls: Option<TlsConfig>` parameter; `SERVER-001` v0.9.0 | `SERVER-TLS-DESIGN` | `SERVER-001` | `cargo test --features server` green including `tests/server_tls_integration.rs`'s five tests (a full round trip over TLS; `TlsConfig` composed with `AuthConfig`; a plain connection to a TLS-configured server never getting a valid response; `TlsConfig: None` reproducing plaintext behavior; the `Authenticate` token verifiably absent from the raw bytes sent on the wire, via a byte-recording stream wrapper) and `src/server/pem.rs`'s ten new unit tests; `cargo test`/`cargo test --all-features` unaffected; `cargo fmt`/`clippy`/`check`/`doc` clean; every `SERVER-TLS-DESIGN.md` functional acceptance criterion verified over a real socket; no `src/store/**`/`src/durability/**`/`src/concurrency/**` changes, and every pre-existing plaintext test passes unmodified (verifying the `Plain` half of the new stream abstraction introduced no behavior change) | Implemented | this PR |
| `EXTERNAL-DB-BENCHMARK` | The first external (not this-crate-implemented) comparison point: `ProductionStore` benchmarked against real SQLite (`rusqlite`), Postgres (`postgres`, local server), and DuckDB (`duckdb`) on the same three access-pattern shapes already benchmarked in-repo — `get`, `scan_ages`, one-hop `littermate_of` traversal (depth-bounded recursive CTE over an adjacency table). New `benches/external_db.rs`, gated behind a new `external-db-bench` Cargo feature; ADR-0015 (Accepted); `STORAGE-013` v0.1.0 (later extended through v0.5.0, one-through-five-hop graph traversal, tracked as version bumps to this same unit rather than new rows — see `PROJECT-STATUS.md`). Stays inside `docs/FUTURE-GROWTH.md`'s existing "Path to SQLite/DuckDB parity" scope line — no SQL parser/planner/arbitrary joins built or implied | `SERVER-TLS` (most recent prior round; no functional dependency) | `STORAGE-013` | `cargo check --bench external_db --features research,external-db-bench` clean; a real `cargo bench --features research,external-db-bench --bench external_db` run against a local Postgres instance, all three workload groups × three sizes × four systems, no panics; `RESULTS.md`'s `## External database comparison` section states real numbers and a verdict per workload, including any workload where `ProductionStore` loses; `cargo fmt`/`clippy --all-targets --all-features`/`check`/`doc` clean; no `src/` changes | Implemented | this PR |
| `PRODUCTION-STORE-PORTABILITY-DESIGN` | A design for closing `ProductionStore`'s file-portability gap against SQLite's/DuckDB's own self-describing `.db` files: a new companion file (bincode-serialized `id`/`breed`/`littermate_of` edges, write-to-temp-then-atomic-rename crash safety reusing `MmapAgeStore`'s own established pattern), a new, additive `ProductionStore::open_portable(path)` constructor needing no caller-supplied `records`/`edges`, implemented in terms of the existing, unchanged `open`; ADR-0016 (**Accepted** — owner approved as proposed); `docs/design/PRODUCTION-STORE-PORTABILITY-DESIGN.md`; explicitly leaves `MmapAgeStore`'s own file format and `create`/`open`'s existing signatures untouched — reframes ADR-0006's declined string-heap redesign as unnecessary, since `breed`/`id`/edges are never mutated, only `age` is | `EXTERNAL-DB-BENCHMARK` (most recent prior round; no functional dependency) | — (no spec written for the design itself; a `STORAGE-014` spec would be registered by the implementation unit that follows, matching `GENERIC-SCHEMA-DESIGN`'s own precedent) | Design document and ADR written, reusing `SnapshotFullStore`'s already-proven full-`DogRecord` bincode round trip and `MmapAgeStore`'s already-proven crash-safe rewrite mechanism rather than a from-scratch string-heap format; owner reviewed and accepted the design and ADR-0016 without requesting changes | Accepted | this PR |
| `PRODUCTION-STORE-PORTABILITY` | The real implementation of `PRODUCTION-STORE-PORTABILITY-DESIGN`: `src/durability/record_blob.rs` (new — `RecordBlob { records, edges }` bincode-serialized behind a `DOGBLOB\0` magic + `u32` version header, written to `<path>.records` via write-to-temp-then-atomic-rename), `ProductionStore::create` now also writes the blob, `ProductionStore::open` reads-and-byte-compares it and rewrites only when the record set changed (which also heals a pre-`STORAGE-014` directory holding only the ages file), a new additive `ProductionStore::open_portable(path)` reconstructing the whole store from the path alone, a new `DurabilityError::RecordBlobUnreadable { path, cause }`; `STORAGE-014` v0.1.0; `src/durability/mmap_store.rs` untouched (empty diff). One implementation-time finding the design did not predict: `open`'s common-case cost is *not* zero — `MmapAgeStore`'s rewrite decision is private to it, so `open` serializes and compares on every call (~+27% at 1M records, measured) — closed in `STORAGE-014` v0.2.0 by a 64-bit FNV-1a content fingerprint in the blob header (`BLOB_VERSION = 2`): `open` now reads the 20-byte header and compares fingerprints, +0.3-4% at 1M, and a version-1 blob is upgraded in place on its first `open` | `PRODUCTION-STORE-PORTABILITY-DESIGN` | `STORAGE-014` | `cargo test`/`cargo test --all-features` green including `record_blob.rs`'s 12 unit tests and `production.rs`'s 6 portability tests (path-alone reconstruction, both files copied to a fresh directory, missing blob a typed error naming the companion path, legacy ages-only directory healed by `open`, blob rewritten only on a changed record set, version-1 blob upgraded in place); `MmapAgeStore`'s existing suite passing unmodified; `cargo fmt`/`clippy --all-targets --all-features`/`doc` clean; a release-build `create`/`open`/`open_portable` cost table at 1K/100K/1M in `RESULTS.md`'s `### ProductionStore file portability` subsection; no benchmarked hot path affected (no Criterion group times `create`/`open` inside `b.iter()`) | Implemented | this PR |
| `GENERIC-STORE-PORTABILITY-DESIGN` | A design for the identical file-portability treatment for `GenericMmapStore`/`GenericProductionStore` — the generic half `ADR-0016` twice named as deliberately out of scope, and the one-durable-field wall `GENERIC-SCHEMA-DESIGN.md` §4.2 first hit with `Order`: a `<path>.records` companion blob of bincode-serialized `Vec<R>` behind the `STORAGE-014` v0.2.0 20-byte magic/version/FNV-1a header (`GENBLOB\0`, version 1), fingerprinted over the streamed encoding (no per-type hand-walking; includes the mmap-backed field, a named cost with a trait-method fingerprint as fallback); `create`/`open`'s existing bounds tightened with `Serialize + DeserializeOwned` (the one breaking change, named — every in-crate record type is one derive line away) rather than a parallel constructor pair that would leave a plain `open`'s blob silently stale; two new additive functions, `read_portable_records(path) -> Vec<R>` (persisted order preserved, so `Reversed`'s child order stays deterministic) and `open_portable(path)`; `open_order_production_stack_portable(path)` as the stack-level helper. Relation layers stay out of the blob (`Reversed` derives from records; `Symmetric`'s external edge list explicitly out of scope, a separate later decision); `GenericMmapStore`'s own `.mmap` format and `GenericProductionStore` unchanged; ADR-0017 (**Accepted** — owner approved as proposed); `docs/design/GENERIC-STORE-PORTABILITY-DESIGN.md` | `PRODUCTION-STORE-PORTABILITY` (`STORAGE-014` v0.2.0 — the header, hash, write path, and error variant it reuses) | — (no spec written for the design itself; a `STORAGE-015` spec would be registered by the implementation unit that follows, matching the `PRODUCTION-STORE-PORTABILITY-DESIGN` precedent) | Design document and ADR written from direct investigation of `src/generic/mmap_store.rs`, `order_customer.rs`, `traits.rs`, `store.rs`, `generic_spike/employee_impl.rs`, and `record_blob.rs`; no `src/` changes; `cargo fmt --all -- --check` clean; owner reviewed and accepted the design and ADR-0017 without requesting changes | Accepted | this PR |
| `GENERIC-STORE-PORTABILITY` | The real implementation of `GENERIC-STORE-PORTABILITY-DESIGN`: `src/generic/record_blob.rs` (new — `GENBLOB\0` magic, blob version 1, the `STORAGE-014` v0.2.0 20-byte magic/version/FNV-1a header, then the bincode `Vec<R>`; `GenericRecordBlob<'a, R>` borrows the record slice and fingerprints it by streaming `bincode::serialize_into` a `Fnv1a64` that `impl`s `io::Write`), `STORAGE-014`'s header/hash/temp-then-rename helpers made `pub(crate)` and shared (`RecordBlob`'s 12 tests unchanged); `GenericMmapStore::create` writes the blob and `open` refreshes it only when the 20-byte header's fingerprint is stale, both gaining the `R: Serialize + DeserializeOwned` bound (the one breaking change, named — serde derives added to `Order`/`OrderStatus`/`Customer`/`Employee`/`Department`); new additive `read_portable_records(path)` (persisted order preserved) and `open_portable(path)` (= `open(read_portable_records(path)?, path)`); `open_order_production_stack_portable(path)` rebuilds the whole `Order`/`Customer` stack, `Reversed` included, from the two files; `STORAGE-015` v0.1.0; the `.mmap` slot format untouched (the diff removes only the six bound lines). Two deliberate departures from the design's sketch, recorded in the spec: no bound on `R::Id`, no `Employee` portable helper (its `Symmetric` edge list stays the deferred decision). Cost, measured rather than assumed: `create` roughly doubles at every size (the blob write, ~76 B/record, 3× the `.mmap` file); `open` pays the streamed-serialization fingerprint on every call — a ~0.1 ms floor at 1K, +19–33% at 100K, and at 1M the throwaway's three-sample medians and the 20-sample Criterion `generic_production_open` group disagree (near zero vs. +24–27%), both recorded in `RESULTS.md` with the Criterion figure treated as the more trustworthy one; the design's fallback (a per-type trait-method fingerprint) is therefore the named next step, the owner's call | `GENERIC-STORE-PORTABILITY-DESIGN` | `STORAGE-015` | `cargo test`/`cargo test --all-features` green including `record_blob.rs`'s 9 unit tests, `mmap_store.rs`'s 6 portability tests (round trip in creation order, both files copied to a fresh directory, missing blob a typed error naming the companion path and healed by a plain `open`, blob rewritten only on a changed record set, a `DOGBLOB\0` file a magic error, an `.mmap`-file failure leaving a valid companion untouched), and `order_customer.rs`'s stack test; `mmap_store.rs`'s 8 pre-existing tests, `tests/mmap_record_identity_keying.rs`, `RecordBlob`'s 12 tests, and `production.rs`'s 6 portability tests passing unmodified; `cargo fmt`/`clippy --all-targets --all-features`/`doc` clean; a release-build `create`/`open`/`open_portable` cost table at 1K/100K/1M plus the same-session Criterion `generic_production_create`/`generic_production_open` before/after pair in `RESULTS.md`'s `### GenericMmapStore file portability` subsection | Implemented | this PR |
| `GENERIC-STORE-FINGERPRINT-MEASUREMENT` | Docs-only: `STORAGE-015`'s named fallback — the per-type trait-method fingerprint that would have been `STORAGE-015` v0.2.0 — measured at the owner's request before being built, and closed as not warranted by the owner's choice. An in-place A/B at 1M `Order` records (the same binary with `is_current_at`'s `fingerprint()` stubbed out) puts the shipped streamed-bincode fingerprint at ~52 ms of a 1,222 ms `open`, 4% — not the ~300 ms the Criterion pair implied (that group, re-run on unchanged code, drifted +8.8% against itself). Isolated candidates: streamed bincode 79 ms, hand-walk every field 72 ms (−7 ms, the same ~76 B/record hashed), id + customer + status 42 ms, id only 21 ms — the larger savings come only from hashing less of the record, which reopens the silently-stale-blob gap `ADR-0016` rejected. Outcome: streamed fingerprint stays, `BLOB_VERSION` stays 1, no record-trait API, `STORAGE-015` stays v0.1.0; `RESULTS.md` gains `#### Follow-up: the trait-method fingerprint, measured and not built` and its earlier verdict is marked superseded; design doc, ADR-0017, spec, and `PROJECT-STATUS.md` item 65 record the closure | `GENERIC-STORE-PORTABILITY` | `STORAGE-015` (unchanged, v0.1.0) | Zero `src/` changes, verified by diff; `cargo fmt --all -- --check` clean; the three measurements recorded with their method in `RESULTS.md`; the owner chose "close" from a three-way measured choice | Implemented | this PR |
| `SYMMETRIC-EDGE-PORTABILITY-DESIGN` | A design for closing the one gap `ADR-0017` named as its own deliberate limitation — `Symmetric`'s external edge list, the only relation-layer state not derivable from records, so `Employee`'s durable stack (`Reversed` over `Symmetric` over `GenericMmapStore`) still needs the caller to hold `collaboration_edges` and got no portable helper under `STORAGE-015`: a `Symmetric`-level companion blob at a caller-supplied `edges_path` (`GENEDGE\0`, version 1, the `STORAGE-014` v0.2.0 20-byte magic/version/FNV-1a header via the already-`pub(crate)` helpers, body a bincode `Vec<(R::Id, R::Id)>` in the order given — the edge list, not the adjacency map, so bytes are deterministic and half the size), a parallel `create`/`open`/`open_portable` triple plus `read_portable_edges` in a separately bounded `impl` block with `Symmetric::new` untouched (the parallel-constructor shape `ADR-0017` rejected is right here because `new` has no path to refresh a blob from; the residual by-convention gap is named), `edges_path(path) -> <path>.edges` as the single-relation convention, `RecordBlobUnreadable` reused with `path` naming the edge blob, `open_employee_production_stack_portable(path)` as the stack-level helper. `GenericMmapStore`/`.mmap`/`.records`/`Reversed`/`dog_impl.rs` unchanged; schema tag and stack manifest deferred; ADR-0018 (**Accepted** — owner approved as proposed); `docs/design/SYMMETRIC-EDGE-PORTABILITY-DESIGN.md` | `GENERIC-STORE-FINGERPRINT-MEASUREMENT` (most recent prior round), `GENERIC-STORE-PORTABILITY` (`STORAGE-015` v0.1.0 — the header, hash, write path, error variant, and `read_portable_records` it reuses) | — (no spec written for the design itself; a `STORAGE-016` spec would be registered by the implementation unit that follows, matching the `GENERIC-STORE-PORTABILITY-DESIGN` precedent) | Design document and ADR written from direct investigation of `src/generic/store.rs`, `src/generic/record_blob.rs`, `src/generic_spike/employee_impl.rs`, `src/generic_spike/dog_impl.rs`, and every `Symmetric::new`/`create_employee_production_stack` call site; no `src/` changes; `cargo fmt --all -- --check` clean; owner reviewed and accepted the design and ADR-0018 without requesting changes | Accepted | PR #98, #99, #100 |
| `SYMMETRIC-EDGE-PORTABILITY` | The accepted `SYMMETRIC-EDGE-PORTABILITY-DESIGN` implemented and registered as `STORAGE-016` v0.1.0: `src/generic/edge_blob.rs` (new, `pub(crate)`) — `GENEDGE\0` magic, blob version 1, the `STORAGE-014` v0.2.0 20-byte magic/version/FNV-1a header via the shared `pub(crate)` helpers (their third call site), body a bincode `Vec<(Id, Id)>` in caller order, `EdgeBlob<'a, Id>` borrowing the slice and fingerprinting by streaming into `Fnv1a64`, `read(path)` verifying magic/version/body-vs-header fingerprint with every failure a `RecordBlobUnreadable { path, cause }` naming the edge blob, `edges_path(path) -> <path>.edges`; one added `impl` block in `src/generic/store.rs` bounded `R::Id: Serialize + DeserializeOwned` — `Symmetric::create(inner, edges, edges_path)`, `open(inner, edges, edges_path)` (header check, rewrite only when stale/missing/unreadable, adjacency always from the caller's edges), `read_portable_edges(edges_path)`, `open_portable(inner, edges_path)` — with `Symmetric::new`, `Neighbors`, and every forwarding impl untouched; `create_employee_production_stack`/`open_employee_production_stack` switched to `create`/`open` at `<path>.edges` with signatures unchanged, new `open_employee_production_stack_portable(path)` rebuilding the whole stack from `<path>` + `<path>.records` + `<path>.edges`; `RecordBlobUnreadable`'s doc names the edge blob, no new variant; 23 new tests (12 blob, 6 `store.rs`, 5 `employee_impl.rs`) covering every acceptance criterion the design named; one recorded departure from the sketch (`fingerprint` returns a `Result`); cost not measured per the design; `GenericMmapStore`/`.mmap`/`.records`/`Reversed`/`dog_impl.rs` untouched, `STORAGE-015` stays v0.1.0 | `SYMMETRIC-EDGE-PORTABILITY-DESIGN`, `GENERIC-STORE-PORTABILITY` | `STORAGE-016` | `STORAGE-016` registered; `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`, `cargo test --all-features` clean; the design's acceptance criteria each pinned by a test; every pre-existing test unmodified | Implemented | PR #102 |
| `BLOB-SCHEMA-TAG-DESIGN` | A design for the one open question `STORAGE-015` and `STORAGE-016` both hold and agreed to resolve together — neither generic companion blob records which `R` it holds, so a foreign-`R` `.records` blob passes magic/version/fingerprint and fails (if at all) inside `bincode`, and every `Uuid`-keyed domain's `.edges` blob decodes as every other's: a new opt-in `SchemaTag { const SCHEMA_TAG: &'static str }` trait in `src/generic/traits.rs` (not a supertrait of `Record`; `std::any::type_name` rejected as unstable across compiler versions, a required const on `Record` rejected as touching nine impls), an 8-byte `u64` LE FNV-1a 64 hash of the tag at offset 20 after the unchanged 20-byte shared header (`TAGGED_HEADER_LEN` 28; `durability::record_blob` and `DOGBLOB\0` untouched; new `pub(crate)` helpers in `generic::record_blob` imported by `generic::edge_blob`), `GENBLOB\0` and `GENEDGE\0` `BLOB_VERSION` 1 → 2 (`STORAGE-015` v0.2.0, `STORAGE-016` v0.2.0 at implementation), check order magic → version → tag → fingerprint → decode with a tag mismatch a `RecordBlobUnreadable` naming the expected tag string (no new variant), `is_current_at` requiring all four so `open` heals version-1 and foreign-`R` blobs (the `DOGBLOB\0` 1 → 2 story), the `R: SchemaTag` bound on exactly the blob-touching functions (`GenericMmapStore`'s four moved into their own `impl` block; `Symmetric`'s `STORAGE-016` block), `Order`/`Employee`/doc-example `Widget` the whole in-crate impl set, the edge blob tagged with `R::SCHEMA_TAG` passed as a value (a `Marker`-level tag rejected); a readable fixed-width 32-byte string offered as the alternative at acceptance; `.mmap` tag, two-relations-per-`R`, and stack manifest deferred; ADR-0019 (**Accepted** — owner approved as proposed, the hashed 8-byte tag); `docs/design/BLOB-SCHEMA-TAG-DESIGN.md` | `SYMMETRIC-EDGE-PORTABILITY` (`STORAGE-016` v0.1.0), `GENERIC-STORE-PORTABILITY` (`STORAGE-015` v0.1.0) — the two blobs it tags | — (no spec written for the design itself; the `STORAGE-015` v0.2.0 / `STORAGE-016` v0.2.0 bumps would be registered by the implementation unit that follows) | Design document and ADR written from direct investigation of `src/generic/record_blob.rs`, `src/generic/edge_blob.rs`, `src/durability/record_blob.rs`, the `GenericMmapStore` and `Symmetric` `impl` blocks, every `Record` impl, and every `GenericMmapStore::<…>` call site; no `src/` changes; `cargo fmt --all -- --check` clean; accepted as proposed by the owner | Accepted | PR #104 (proposed), PR #106 (accepted) |
| `BLOB-SCHEMA-TAG` | The accepted `BLOB-SCHEMA-TAG-DESIGN` implemented and registered as `STORAGE-015` v0.2.0 / `STORAGE-016` v0.2.0: `pub trait SchemaTag { const SCHEMA_TAG: &'static str; }` in `src/generic/traits.rs` (public, not a supertrait of `Record`, documented as on-disk format); `src/generic/record_blob.rs` gains the `pub(crate)` tagged-header helpers (`TAG_OFFSET` 20, `TAGGED_HEADER_LEN` 28, `tag_hash` = FNV-1a 64 over the tag bytes pinned to the published test vectors, `encode_tagged_image`, `parse_tagged_header` = shared header then tag) and `GENBLOB\0` `BLOB_VERSION` 1 → 2; `src/generic/edge_blob.rs` imports them, `EdgeBlob::new(edges, tag)`/`read(path, tag)` take the tag as a value, `GENEDGE\0` `BLOB_VERSION` 1 → 2; check order magic → version → tag → fingerprint → decode with a foreign-`R` blob a `RecordBlobUnreadable` whose cause names the expected tag string and both hashes, a version-1 blob a `version` cause, a file cut inside the tag a short-tagged-header cause — no new variant; `is_current_at` requires all of them so `open` rewrites version-1 and foreign-`R` blobs (the heal, no migration step); `R: SchemaTag` on exactly the blob-touching functions (`GenericMmapStore`'s four file constructors moved into their own `impl` block; `Symmetric`'s `STORAGE-016` block), `Order` (`"order_customer::Order"`), `Employee` (`"employee::Employee"`), doctest `Widget` the whole impl set; `durability::record_blob`/`DOGBLOB\0`/`.mmap` untouched; 12 new tests (5 record blob, 3 edge blob, 2 `mmap_store.rs` `Employee`-as-`Order` and version-1 heal, 2 `store.rs` second-record-type and version-1 heal) pinning every acceptance criterion the design named; both specs' shared open question resolved | `BLOB-SCHEMA-TAG-DESIGN`, `SYMMETRIC-EDGE-PORTABILITY`, `GENERIC-STORE-PORTABILITY` | `STORAGE-015` v0.2.0, `STORAGE-016` v0.2.0 | Both specs bumped and re-registered; `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`, `cargo test --all-features` clean; `cargo doc --no-deps` at baseline; every pre-existing test passing | Implemented | PR #108 |
| `MULTI-FIELD-MMAP-DURABILITY-DESIGN` | A design answering `ADR-0006`'s named revisit trigger ("a future record shape needs more than one mutable field persisted" — met by `Order` per `GENERIC-SCHEMA-DESIGN` §4.2, carried as "genuinely unscoped" in `PROJECT-STATUS` item 22 since): `GenericMmapStore` persists exactly one `ScanMarker` in its `.mmap`; the `.records` blob makes every other field durable but immutable. Weighs the three shapes `PROJECT-STATUS` item 68 names — (1) a multi-slot `.mmap` layout (`SCHEMA_VERSION` 3, per-domain layout type, refuses every existing file, makes every slot helper and the benchmark-pinned `scan` fast path layout-generic, the AoS stride `ADR-0001` measured as bad for single-column scans), (2) per-field `.mmap` files via a composable `MmapScanned<S, R, Marker>` layer — the durable twin of `Scanned`, one file per marker in exactly the existing `GMMAPST\0` version-2 format, `get` patching on the way up, `Flush` own-then-inner, cross-marker forwards from `forward_scannable_pairs!` generalized to the layer type, the slot/header/commit/reconcile machinery extracted into a `pub(crate)` `SlotFile` both stores share (duplication the fallback), caller-supplied path per layer (`STORAGE-016` precedent) with the domain constructor deriving `<path>.discount_cents.mmap`, a slot-width check as the weak foreign-file guard and the tagged blob as the strong one — the shape §4.2 sketched, and (3) widening the blob's role (a full encode + fingerprint + write per `update`, or a second WAL; rejected for fixed-width fields, the named fallback for a variable-width one); proposes (2) with `OrderProductionStack` gaining `DiscountCents` as the in-crate proof and `CreatedAt` deliberately left in-memory; no format changes; multi-field atomic updates, variable-width fields, the `.mmap` tag, the stack manifest, and re-expressing `GenericMmapStore` as the layer all named as non-goals or revisit triggers; nine requirements `MFMD-FR-001`–`-009` and eight acceptance criteria for a `STORAGE-017` at implementation; ADR-0020 (**Proposed** — owner's call); `docs/design/MULTI-FIELD-MMAP-DURABILITY-DESIGN.md` | `BLOB-SCHEMA-TAG` (`STORAGE-015` v0.2.0 — the tagged blob the design leans on), `GENERIC-STORE-PORTABILITY`, `GENERIC-SCHEMA` (`STORAGE-012`), `DURABILITY-TIER2` (`ADR-0006`) | — (no spec for the design itself; `STORAGE-017` v0.1.0 is registered by `MULTI-FIELD-MMAP-DURABILITY`, the implementation unit that followed acceptance) | Design document and ADR written from direct investigation of `src/generic/mmap_store.rs` (`create`, `open`, every query impl, the slot helpers), `src/generic/store.rs` (`Scanned`, `forward_scannable_pairs!`), `src/generic/order_customer.rs` (`OrderProductionStack` and its constructors), `src/generic/mmap_field.rs`, and `ADR-0006`/§4.2's exact wording; no `src/` changes; `cargo fmt --all -- --check` clean; accepted as proposed by the owner | Accepted | PR #110 (proposed), PR #112 (accepted) |
| `MULTI-FIELD-MMAP-DURABILITY` | The accepted `MULTI-FIELD-MMAP-DURABILITY-DESIGN` implemented and registered as `STORAGE-017` v0.1.0: `src/generic/slot_file.rs` (new, `pub(crate)`) — the `GMMAPST\0` constants, slot arithmetic, header read/write, committed-slot read, `create(path, slots)`, `open(path)`, `committed_pairs()`, `append_committed_slots()`, `flush()` extracted verbatim from `GenericMmapStore`, which now delegates to it with its 16 tests textually unchanged and the `chunks_exact` scan fast path kept in each owner over `slot_bytes()`; `src/generic/mmap_scanned.rs` (new, `pub`) — `MmapScanned<S, R, Marker>`, the durable twin of `Scanned`: one more mutable, durable, scannable field over any inner store in its own version-2 slot file (no format change), `create`/`open` in one `R: SchemaTag`-bounded `impl` block, per-file reconciliation with the same four cases and `O_APPEND` repair `GenericMmapStore::open` has, `get` patching its field on the way up, `Flush` own-then-inner, generic `FilterEq`/`Neighbors`/`Children` forwards; `forward_scannable_pairs!` gains `for Scanned;` / `for MmapScanned;` entry arms (the layer path carried as one `tt` through the rotating accumulator, the bare spelling preserved) so the cross-marker forwards are generated for both layers; new `DurabilityError::SlotWidthMismatch { path, body_len, slot_width }` — `MmapScanned::open` refuses a file whose body is not a whole number of its slots (the design's weak foreign-file guard, also catching a truncated-mid-slot file the blob-backed store tolerates); `OrderProductionStack` becomes `Reversed<MmapScanned<GenericMmapStore<Order, Status, Amount>, Order, DiscountCents>, …>`, the three constructors keep their signatures and derive `<path>.discount_cents.mmap` via `discount_cents_path`, `CreatedAt` deliberately in-memory (a compile-time refusal through the durable stack); two Criterion groups (`scan_layer`, `update_layer`) beside the core's `scan`/`update`, before/after run recorded in `RESULTS.md` (core `scan`/`update` within noise; `get`/`parent` +22–67% for the second layer's slot read; `create`/`open` +15–42% for the second file); 9 new tests, 1 extended, 2 compile-time pair checks — every acceptance criterion the design named pinned; six implementation calls the design left open recorded under the spec's "Traceability" | `MULTI-FIELD-MMAP-DURABILITY-DESIGN`, `BLOB-SCHEMA-TAG`, `GENERIC-STORE-PORTABILITY` | `STORAGE-017` | `STORAGE-017` registered; `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test` (124 lib + 2 integration), `cargo test --all-features` (316 lib) clean; every pre-existing test unmodified; Criterion `scan`/`update` within noise of the pre-extraction baseline | Implemented | PR #114 |
| `BINCODE-ENCODING-STABILITY-DESIGN` | A design answering the limitation `ADR-0010` named when the server layer was proposed and `SERVER-001`/`PROJECT-STATUS` item 33 have carried since — *"`bincode`'s wire-format stability across crate versions is unverified"* — now that the three companion blobs (`STORAGE-014`–`016`) have raised the same cross-build bar for files: every `bincode` byte this crate writes comes from the crate's *free functions* with no configuration named in `src/`, and no test pins a byte. Verified (vendored `bincode` 1.3.3 source, `uuid` 1.25.0's serde impl, an executed probe) what the free functions pin — fixint integers at Rust width, little-endian, no limit, trailing bytes allowed on read; **not** `bincode::options()`/`DefaultOptions`, which are varint and reject trailing (a `Uuid` is 24 bytes, not 17; one probed enum value 22 bytes vs 5) — and the 1.x readme's promise (*"stable across minor revisions, provided the same configuration is used"*), whose "same configuration" condition this crate satisfies by convention only. Inventories the 23 production call sites (11 front-door: `server/framing.rs`, `durability/record_blob.rs`, `generic/record_blob.rs`, `generic/edge_blob.rs`; 12 `research`-gated or research-only-reached) and proposes: a `pub(crate)` `src/codec.rs` naming the configuration once (`DefaultOptions::new().with_fixint_encoding().with_little_endian().with_no_limit()` + `reject_trailing_bytes()`) behind `encode`/`encode_into`/`decode` with `bincode::Error` unchanged; all 23 sites routed so `bincode::` in production `src/` is a `grep`; golden byte vectors (primitives, every `Request`/`Response` variant, one body each of `DOGBLOB\0`/`GENBLOB\0`/`GENEDGE\0`) captured on the pre-change code and checked in as hex so the routing is proven byte-identical; the evolution rules for pinned types documented; no format bump, no dependency change, no public API change. Acceptance question: reject trailing bytes (proposed — frames with junk after a valid message become `FrameError::Encoding`; the blobs' fingerprint already refuses such a body) vs allow (a purely no-behavior-change pin) vs document-only; `bincode` 2.x (`config::legacy()`), a wire-protocol version/handshake, a `Uuid` newtype, and `ADR-0019`'s layout fingerprint (concluded not needed now; trigger stays armed) named as non-goals or revisit triggers; eight requirements `BINENC-FR-001`–`-008` and eight acceptance criteria for a spec at implementation (`STORAGE-018`, approved at acceptance); ADR-0021 (**Accepted** — as proposed by the owner); `docs/design/BINCODE-ENCODING-STABILITY-DESIGN.md` | `SERVER-QUERY-LAYER` (`SERVER-001` — the wire protocol), `PRODUCTION-STORE-PORTABILITY` (`STORAGE-014`), `GENERIC-STORE-PORTABILITY` (`STORAGE-015`), `SYMMETRIC-EDGE-PORTABILITY` (`STORAGE-016`), `BLOB-SCHEMA-TAG` (the blob formats at their current versions) | — (no spec for the design itself; a `STORAGE-018` would be registered by the implementation unit that follows acceptance) | Design document and ADR written from direct investigation of the vendored `bincode` 1.3.3 source (`lib.rs`, `config/mod.rs`, `config/int.rs`, `config/trailing.rs`), `uuid` 1.25.0's `Serialize` impl, an executed throwaway probe (not committed), and all 23 call sites with their feature gates; no `src/` changes; `cargo fmt --all -- --check` clean | Accepted | PR #116 (proposed), this PR (accepted) |

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

`SERVER-TLS-DESIGN` picks up the second of the two options the owner
selected alongside the transaction benchmark — transport encryption —
and, matching `SERVER-AUTH-DESIGN`/`SERVER-TRANSACTION-DESIGN`'s own
precedent, does not ship straight to implementation: this is the last
remaining half of the "no auth, no encryption" gap ADR-0010 named at
acceptance, and ADR-0012's own "Validation and revisit triggers" named
exactly this proposal as the condition for revisiting native TLS rather
than deferring it again by default. ADR-0014 (Accepted) and
`docs/design/SERVER-TLS-DESIGN.md` propose native TLS terminated inside
this crate's own server process, rather than continuing to require an
external TLS-terminating proxy/tunnel — reversing ADR-0012's original
rejection of native TLS once the dependency-weight objection is checked
on its actual merits: a synchronous, `Read`/`Write`-compatible TLS API
composes with the existing thread-per-connection model with **zero**
changes to `src/server/framing.rs` (already generic over `Read`/`Write`,
a verified finding, not an assumption), undercutting the earlier
"disproportionate dependency" framing that had implicitly bundled TLS
together with an async-runtime shift. **Revised mid-review**: the owner
asked whether this owner's own ecosystem already had a hand-rolled or
wrapped solution before this got proposed as a fresh `rustls`
dependency — it does (`rusty_tls`, `Rusty-Mill/rusty_mill`, purpose-built
so no consumer in that ecosystem depends on `rustls` directly), so this
proposal depends on `rusty_tls` rather than `rustls` directly, inheriting
its own tested, fuzzed rejection-path coverage rather than rebuilding
it. **Explicitly does not add mTLS** — client identity remains exactly
`AuthConfig`'s existing shared-secret token scheme, now traveling
encrypted (though the mechanism, `rusty_tls::TlsAcceptor::new_with_client_auth`,
already exists if that's ever revisited). The owner reviewed and
accepted the design as revised, no further changes requested.

`SERVER-TLS` is that implementation unit, following how `SERVER-AUTH`/
`SERVER-TRANSACTION` each followed their own design's acceptance:
`SERVER-001` v0.9.0 (`SERVER-001-FR-019`) implements
`SERVER-TLS-DESIGN` essentially as revised, no design decision reopened.
The one real implementation-time finding this round surfaced: the
existing plaintext path splits a connection into independent read/write
halves via `TcpStream::try_clone` (two real OS-level socket handles),
but `rusty_tls::TlsServerStream`'s `rustls::ServerConnection` state
can't be split that way — a read and a write both need to reach through
the *same* connection object. Resolved with a new `ReadHalf`/`WriteHalf`
enum pair: the `Plain` variant keeps the existing split completely
unchanged (zero behavior change, verified by the full existing
plaintext test suite passing unmodified); the `Tls` variant shares one
`Rc<RefCell<TlsServerStream<TcpStream>>>` between both halves — `Rc`/
`RefCell`, not `Arc`/`Mutex`, since each connection is served by exactly
one OS thread, so a single-threaded runtime borrow check is enough.
`tests/server_tls_integration.rs`'s flagship acceptance test captures
the real bytes a TLS client sends on the wire (a byte-recording
`TcpStream` wrapper) and asserts the plaintext `Authenticate` token is
genuinely absent — the direct evidence the design's whole purpose
depends on, not an assumption.

`EXTERNAL-DB-BENCHMARK` is a different kind of round from everything
before it in this roadmap: not a new backend, durability variant,
concurrency strategy, or server capability this crate itself implements,
but the first comparison against real, external, general-purpose
databases — SQLite, Postgres, DuckDB — on the same three access-pattern
shapes (`get`, `scan_ages`, one-hop `littermate_of` traversal) this
project has always benchmarked. ADR-0015 (Accepted) and `STORAGE-013`
record the decision; `RESULTS.md`'s `## External database comparison`
section has the numbers. `ProductionStore` wins every cell — expected,
since it is a purpose-built in-memory store with none of a general-purpose
engine's parsing/planning/serialization overhead to pay — but the more
interesting finding is where each external engine gets *close*: DuckDB
closes to within 3.3× of `ProductionStore` on `scan_ages` at 1M records
(exactly the columnar-scan strength the task predicted), while being the
*worst* of the three external engines on both `get` and graph traversal
at every size — a narrow, not general, strength. A real methodology bug
was found and fixed mid-round, not glossed over: Postgres's graph-traversal
numbers initially showed a ~1,400× cliff between 1K and 100K records,
traced to a missing `ANALYZE` after the bulk `COPY` load driving Postgres's
recursive-CTE cost estimate high enough to trigger LLVM JIT compilation on
every execution — confirmed directly via `EXPLAIN ANALYZE` (53 ms before
the fix, 0.196 ms after, same query, same data). See `RESULTS.md` for the
full investigation and the corrected numbers it produced.

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
  **Scoped, not built**, for the generic library's mmap store only, by
  `MULTI-FIELD-MMAP-DURABILITY-DESIGN`/ADR-0020 (Accepted): more than
  one *fixed-width* mutable durable field via a per-field `MmapScanned`
  layer; `redb`, LSM, and variable-width fields stay out.
- A wire-protocol version field or hello handshake, and a `bincode` 2.x
  migration — `SERVER-001`'s frames carry no version, and `Cargo.toml`
  pins `bincode = "1"`. **Scoped, not built**, by
  `BINCODE-ENCODING-STABILITY-DESIGN`/ADR-0021 (Accepted): the
  *encoding* under the `Request`/`Response` shape and the three blob
  bodies is named once and pinned with golden vectors; the shape's
  negotiation, 2.x (`config::legacy()` against those vectors), and a
  smaller `Uuid` encoding are its named revisit triggers.
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
