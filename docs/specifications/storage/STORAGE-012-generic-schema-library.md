# STORAGE-012 — Generic schema library: promote the generic record/schema/query design into a real, public library

- Version: 0.3.0 (`GenericProductionStore::with_exclusive` — see "Change history")
- Status: Accepted
- Owners: baileyrd
- Depends on: `STORAGE-001`, `STORAGE-002`, `STORAGE-005`, `STORAGE-009`, `STORAGE-010`, `STORAGE-011`
- Supersedes: none

## Purpose and scope

Four validation spikes (kept as historical record in `src/generic_spike/`) resolved every risk `docs/design/GENERIC-SCHEMA-DESIGN.md` §4 named for the generic record/schema/query design ADR-0009 proposed: Dog-overhead measurement, an associated-type-ambiguity diagnosis and fix, macro-generated per-marker-pair forwarding, and directed-relation-generalization measurement. This spec covers promoting that validated design into `src/generic/` — a real, public library, generic over any `Record`-implementing type — and building the piece none of the four spikes had: a generic equivalent of `ProductionStore`, wired to real mmap durability and `RwLock` concurrency. See ADR-0009 (Accepted) for which round justified each decision and `RESULTS.md`'s `## Generic schema library` section for the numbers.

## Non-goals

- Not a change to `crate::production::ProductionStore`, `crate::store::DogStore`, or any benchmarked `Dog` backend — this spec adds new, parallel capability; it doesn't modify any of them.
- Not porting `Dog` onto the generic core as a "third validation domain," despite the original design doc's §5 recommending that staging — `Dog` is a benchmark fixture for storage-engine mechanics, not a target domain for this arc (see `crate::generic`'s own module docs); `Order`/`Customer` is this library's real reference implementation instead.
- Not generalizing mmap durability to more than one mutable field — `GenericMmapStore` deliberately keeps `MmapAgeStore`'s exact one-`IndexedField`-plus-one-`ScannableField` scope, generically, per ADR-0009 §4.2's own finding that a real redesign (not an extension) would be needed to lift this, which this spec does not attempt.
- Not fixing the in-memory `Scanned`/`BaseStore` composition's write-through gap (`GetById::get` not reflecting `UpdateField::update`'s writes) — a real finding from this round (see ADR-0009's "Acceptance and implementation" section), flagged as unscoped follow-up work, not fixed here. `GenericMmapStore` (the durable path) does not have this gap.
- Not a generic dataset generator/benchmark-infrastructure — `crate::generic_spike::order_bench_support` remains `Order`-specific and hand-written, same as ADR-0009 §4.7 already flagged.
- Not a new dependency — this spec reuses `memmap2` (already a dependency, from `STORAGE-009`) and every other piece already in the crate's dependency graph.

## Context and terminology

- **`crate::generic`**: the new, real library module (`src/generic/`) — `traits`/`query`/`store` (promoted, not rewritten, from the spikes' `traits.rs`/`query.rs`/`store.rs`), `order_customer` (the `Order`/`Customer` reference implementation, promoted from `generic_spike/order_impl.rs`), `mmap_field`/`mmap_store` (new: the generic durable core), `production` (new: the generic `RwLock`-guarded wrapper).
- **`GenericMmapStore<R, IndexMarker, ScanMarker>`**: the generic, hand-fused analogue of `MmapAgeStore` — one equality index, one mmap-backed scannable field. Implements `GetById`/`FilterEq`/`ScanField`/`UpdateField`/`Flush` directly, mirroring `MmapAgeStore`'s own hand-fused (not composed) construction.
- **`GenericProductionStore<S>`**: the generic analogue of `ProductionStore` — wraps a composed generic store `S` (typically `GenericMmapStore` plus zero or more `Reversed`/`Symmetric`/`Indexed`/`Scanned` layers) in one `RwLock`, exposing `&self` inherent methods generic over whatever capability trait `S` implements.
- **`Flush`**: a new capability trait (`store.rs`) letting a composed stack forward a durability flush through however many wrapper layers sit on top of `GenericMmapStore` — added during promotion, not part of the original design doc.
- **`ScannableField::set_scannable_value`**: a new required trait method (`traits.rs`) letting `GenericMmapStore::get` write the live mmap value back into the returned record generically — added to close the write-through-consistency gap found while building the durable path.
- **`OrderProductionStack`**: the concrete type alias (`order_customer.rs`) — `Reversed<GenericMmapStore<Order, Status, Amount>, Customer, Order, BelongsToCustomer>` — `Amount` is the one durable field; `Status` the one equality index; `Children`/`Parent` layered on top via the unmodified `Reversed` wrapper.
- **Flagship test**: `tests/generic_production_integration.rs`, the highest-priority deliverable — the first test exercising `GenericMmapStore` durability and `GenericProductionStore` `RwLock` concurrency together, on `Order`/`Customer`, mirroring `tests/production_integration.rs`'s bar for `Dog`.

## Requirements

- `STORAGE-012-FR-001`: **Promotion, not rewrite** — `src/generic/traits.rs`/`query.rs`/`store.rs` carry forward every validated fix from the spikes (the `IndexValue`/`ScanValue` rename, `forward_scannable_pairs!`, the directed-relation `Parent`/`Children`/`Reversed` split) unchanged in behavior; `src/generic_spike/` is updated to point at the promoted module rather than duplicating it, and kept (not deleted) as the historical measurement record.
- `STORAGE-012-FR-002`: **`Order`/`Customer` reference implementation** — `src/generic/order_customer.rs` is a real reference implementation (not disposable prototype code), including the three-scannable-field `forward_scannable_pairs!` invocation and the full in-memory `OrderGenericStore` composition, unchanged in shape from the spike that validated it.
- `STORAGE-012-FR-003`: **`GenericMmapStore`** — `src/generic/mmap_store.rs` defines `GenericMmapStore<R, IndexMarker, ScanMarker>` with `create`/`open`/`flush` mirroring `MmapAgeStore`'s own methods of the same name, and implements `GetById<R>`/`FilterEq<R, IndexMarker>`/`ScanField<R, ScanMarker>`/`UpdateField<R, ScanMarker>`/`Flush` directly. `scan` uses a bulk `chunks_exact` read from the start (not a per-position loop needing a later fix, unlike `MmapAgeStore`'s own history). Generic over the scannable value's width via `MmapFieldValue` (`mmap_field.rs`), implemented for `u32`/`i64` — the two concrete types this crate's two domains actually use.
- `STORAGE-012-FR-004`: **`GenericProductionStore<S>`** — `src/generic/production.rs` wraps any composed store `S` in one `RwLock`, exposing `get`/`filter_eq`/`scan`/`update`/`parent`/`children`/`flush` as inherent `&self` methods, each generic over whatever capability trait bound `S` satisfies.
- `STORAGE-012-FR-005`: **Flagship integration test** — `tests/generic_production_integration.rs` runs two phases of 16 threads × 2,000 iterations each of interleaved `get`/`update` calls (matching `run_concurrency_stress_test`'s/`production_integration.rs`'s own rigor) against a shared `GenericProductionStore<OrderProductionStack>`, separated by a genuine drop + reopen from disk. Verified via sequential-replay linearizability against a plain `HashMap` reference (not `OrderGenericStore` — see FR-007) and a third, post-drop reopen for persistence.
- `STORAGE-012-FR-006`: **Benchmark suite, no regression** — `benches/generic_production.rs` measures `get`/`scan`/`filter_eq`/`parent`/`children` through `GenericProductionStore<OrderProductionStack>` at 1K/100K/1M, compared in `RESULTS.md` against the `directed-relation-spike` round's adjacency-index numbers (`parent`/`children`) and `ProductionStore`'s own post-fix `Dog` numbers (`get`/`scan`, same `chunks_exact` technique) to confirm nothing regressed in the move from scratch/spike code to this real implementation.
- `STORAGE-012-FR-007`: **Write-through consistency for the durable path, documented gap for the in-memory path** — `ScannableField::set_scannable_value` is added and used by `GenericMmapStore::get` so it reflects the latest `UpdateField::update` write. The pre-existing in-memory `Scanned`/`BaseStore` composition's lack of this property is documented (`crate::generic`'s module docs, ADR-0009, `RESULTS.md`) as a real, unscoped follow-up — not silently worked around, and not fixed by this spec.
- `STORAGE-012-FR-008`: **Documentation** — ADR-0009 moves from Proposed to Accepted with an implementation addendum; `docs/design/GENERIC-SCHEMA-DESIGN.md` gains an "Implementation status"/"What actually happened" addendum describing what changed between the original design and the real code; `src/lib.rs`/`README.md` describe `crate::generic` alongside the existing `Dog`/`ProductionStore` story; `docs/roadmap/ROADMAP.md`, `docs/traceability/TRACEABILITY.md`, `docs/specifications/SPEC-REGISTRY.md`, `docs/PROJECT-STATUS.md` are updated to reflect this as a real, implemented milestone.
- `STORAGE-012-FR-009`: **No new dependency** — this spec reuses `memmap2` (already present, from `STORAGE-009`) and every other existing dependency; no new entry in `Cargo.toml`'s `[dependencies]`.
- `STORAGE-012-FR-010` (v0.2.0): **`Reversed` forwards `Neighbors`** — `src/generic/store.rs` gains a `Neighbors<R, RelMarker>` forwarding impl on `Reversed<S, P, C, Marker>`, and `src/generic/production.rs` gains a `neighbors<R, Marker>` inherent method on `GenericProductionStore<S>`. Neither existed before: every prior consumer of `Reversed` (`Order`/`Customer`) had a `ChildOf` relation but no `SymmetricRelation`, so the gap was invisible until `SERVER-QUERY-LAYER`'s third validation domain (`Employee`, self-referential on both relation kinds at once) tried to stack `Symmetric` beneath `Reversed` and found `Neighbors` silently unavailable through the composed stack. This is a completion of FR-001's original promotion scope (every capability trait a wrapper's inner store implements should forward through it), not a new design decision — `crate::generic`'s trait/wrapper shapes are otherwise unchanged. See ADR-0009's "Acceptance and implementation" addendum for the full account.
- `STORAGE-012-FR-011` (v0.3.0): **`GenericProductionStore::with_exclusive`** — a new inherent method, `with_exclusive<R>(&self, f: impl FnOnce(&mut S) -> R) -> R`, runs `f` with the wrapped store `S` exclusively locked for `f`'s entire duration, the same internal `RwLock` every other method here already acquires and releases per call, held longer instead of duplicated. A plain inherent method, not a trait — unlike `crate::production::ProductionStore` (wrapped generically by `server::dog::DogConnectionStore<S>`, needing a trait bound to reach this capability through), every `*ConnectionStore` adapter wrapping `GenericProductionStore<S>` (`OrderConnectionStore`, `EmployeeConnectionStore`) is concretely typed over one specific `S`, so no generic caller needs a trait bound here. This is the generic analogue of `STORAGE-011`'s own `TransactionalStore` — together, the two halves of the storage-layer mechanism ADR-0013's `Request::Transaction` atomicity guarantee depends on (`docs/design/SERVER-TRANSACTION-DESIGN.md`).

## Architecture and interfaces

`src/generic/{mod,traits,query,store,mmap_field,mmap_store,order_customer,production}.rs` — the full library. `src/generic_spike/{mod,dog_impl,order_naive,order_bench_support}.rs` — updated import paths, kept as historical record. `tests/generic_production_integration.rs` — the flagship test. `benches/generic_production.rs` — the benchmark suite. No changes to `src/production.rs`, `src/store/**`, `src/durability/**`, `src/concurrency/**`.

## Data/state and invariants

- `GenericMmapStore<R, IndexMarker, ScanMarker>` is constructed from `(records: Vec<R>, path: &Path)`, mirroring `MmapAgeStore::create`/`open`'s signature exactly (one fewer parameter than `MmapAgeStore` since there's no separate `edges` — relations, when present, are supplied to whatever wrapper layer needs them, e.g. `Reversed::new(inner, children)`).
- Exactly one `ScannableField` marker is durable per `GenericMmapStore` instantiation — the single-mutable-field scope inherited directly from `MmapAgeStore`, not a new limitation.
- `GenericProductionStore<S>::get`/`filter_eq`/`scan`/`parent`/`children` take a read lock; `update`/`flush` take a write lock — same split `ProductionStore` already uses, same rationale (a checkpoint wants a quiescent snapshot).

## Errors, failure, recovery, and observability

`GenericMmapStore::create`/`open`/`flush` and `GenericProductionStore::flush` return `Result<_, DurabilityError>` — the existing shared error type, no new error enum. `UpdateField::update`/`GenericProductionStore::update` return `Result<(), NotFound<R::Id>>` — the existing generic error type from `crate::generic`. `.expect()` appears only for `RwLock` poisoning (`GenericProductionStore`'s `LOCK_POISONED`, matching `ProductionStore`'s own documented exception) and in test-only code.

## Security, privacy, and compatibility

Not applicable — synthetic in-memory/on-disk data only, same as every other spec in this tree.

## Acceptance criteria

- `cargo test --all-features` passes, including `generic::{mmap_field,mmap_store,order_customer,production}::tests::*` and the new flagship integration test (`tests/generic_production_integration.rs`), run 5× consecutively with no flakiness.
- `cargo bench --bench generic_production` completes without panics, with numbers reported in `RESULTS.md`'s `## Generic schema library` section, compared against the `directed-relation-spike` round's and `ProductionStore`'s own numbers.
- No `src/production.rs`, `src/store/**`, `src/durability/**`, or `src/concurrency/**` changes — verified by the diff touching only `src/generic/**` (new), `src/generic_spike/**` (import-path updates only), `src/lib.rs`, `Cargo.toml` (new `[[bench]]` entries only), `benches/generic_production.rs` (new), `tests/generic_production_integration.rs` (new), and documentation.
- ADR-0009 reads Status: Accepted. `docs/design/GENERIC-SCHEMA-DESIGN.md` reads Status: Accepted and implemented.

## Verification plan

- Unit tests: `MmapFieldValue` round-trip (`u32`/`i64`), `GenericMmapStore` create/open/flush round trip + `GetById`/`FilterEq`/`ScanField`/`UpdateField` behavior, `order_customer`'s full in-memory-stack test plus the production-stack create/flush/reopen tests, `GenericProductionStore`'s lock-forwarding tests.
- Flagship integration test: two 16-thread × 2,000-iteration concurrent contention phases separated by a genuine drop + reopen, verified via sequential-replay linearizability (against a plain `HashMap` reference, not the in-memory composition — see FR-007) and a third, post-drop reopen for persistence.
- Benchmark run: `get`/`scan`/`filter_eq`/`parent`/`children` through `GenericProductionStore<OrderProductionStack>` at 1K/100K/1M, cross-checked against the `directed-relation-spike` round's and `ProductionStore`'s own established numbers for consistency (not a fresh, unconstrained baseline).

## Traceability

Implements: the "full implementation of the generic schema library" deliverable, promoting ADR-0009 from Proposed to Accepted. v0.2.0's `Neighbors`-forwarding completion closes ADR-0009's own "revisit if" trigger (`SymmetricRelation` + `ChildOf` together, untested until `SERVER-QUERY-LAYER`'s `Employee` domain exercised it). v0.3.0's `with_exclusive` implements the generic-store half of ADR-0013's accepted `SERVER-TRANSACTION-DESIGN` — the other half (`Request::Transaction`, `ConnectionStore::apply_transaction`) lives in `SERVER-001`.

## Change history

- 0.1.0: Initial promotion — `src/generic/**` as a real, public library; `Order`/`Customer` reference implementation; `GenericMmapStore`/`GenericProductionStore`; the flagship durability-plus-concurrency integration test; the write-through-consistency fix for the durable path (FR-007).
- 0.2.0: `Reversed`/`Neighbors` forwarding completion (FR-010) — found and fixed while validating `SERVER-QUERY-LAYER`'s third domain, `Employee`; no other `crate::generic` behavior changed.
- 0.3.0 (ADR-0013): `GenericProductionStore::with_exclusive` (FR-011) — the generic-store half of the storage-layer critical-section primitive the server layer's `Request::Transaction` atomicity guarantee depends on (`STORAGE-011`'s `TransactionalStore` is the `Dog`-specific half). No change to any existing `GenericProductionStore<S>` method's behavior; purely additive.
