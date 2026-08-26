//! Benchmark harness comparing AoS, SoA, and UUID-canonical-store storage
//! layouts behind one [`store::DogStore`] trait — and, since six rounds of
//! that comparison (row/column/graph, mixed-workload, durability, and
//! three concurrency-throughput passes) all converged on one combination,
//! [`production::ProductionStore`], the crate's recommended entry point.
//!
//! # Start here: [`production::ProductionStore`]
//!
//! If you're looking for what to actually use, this is it:
//! `CanonicalCachedStore`'s storage architecture, made durable via mmap,
//! made safe for concurrent reader/writer access via one global `RwLock`.
//! It implements both [`store::DogStore`] (drop-in for single-owner code)
//! and [`concurrency::ConcurrentStore`] (for genuine multi-threaded
//! sharing). See `docs/decisions/ADR-0008-production-default.md` for which
//! round justified each of the three layers, and `RESULTS.md`'s
//! `## Production recommendation` section for the numbers.
//!
//! # A second, generalized story: [`generic::production::GenericProductionStore`]
//!
//! Everything above is `Dog`-specific, by design — the charter's whole
//! point was measuring one fixed record shape across storage layouts. A
//! separate, later arc asked whether that same storage/durability/
//! concurrency recipe generalizes to *any* record type, validated it
//! against a second, structurally different domain (`Order`/`Customer` —
//! a directed relation, a currency-like field, an enum categorical field),
//! and promoted the result into [`generic`], a real generic library:
//! `Record`/`IndexedField`/`ScannableField`/`SymmetricRelation`/`ChildOf`
//! traits, composable store wrapper layers, and
//! [`generic::production::GenericProductionStore`] — the generic
//! equivalent of [`production::ProductionStore`], wired to the same mmap
//! durability and global `RwLock` concurrency. See [`generic`]'s own
//! module docs for the four-spike validation history and
//! `docs/decisions/ADR-0009-generic-schema-design-proposal.md` (Accepted)
//! for the acceptance record. This is new, parallel capability — nothing
//! above changed to build it.
//!
//! # Everything else: benchmarked alternatives, not the recommended path
//!
//! `store`, `durability`, and `concurrency` hold the other three storage
//! backends, seven other durability variants, and three other concurrency
//! strategies this recommendation is built on — the evidence, not dead
//! code. Each module lost outright, tied, or won only in a narrow corner
//! the production pick's own module docs and `RESULTS.md` name explicitly.
//! Reach for one of them directly only if your workload's shape is
//! genuinely one of those narrow corners (e.g. `ShardedStore` for a small,
//! write-heavy, high-thread-count deployment — see
//! `docs/decisions/ADR-0008-production-default.md`); otherwise, use
//! [`production::ProductionStore`].
//!
//! See `docs/charter/CHARTER.md` for the original hypothesis under test
//! and `docs/decisions/ADR-0001-three-backend-empirical-comparison.md` for
//! why the first three backends are compared this way.

pub mod bench_support;
pub mod concurrency;
pub mod durability;
pub mod generator;
/// A generic record/schema/query library: any domain implementing
/// [`generic::traits::Record`] and friends gets equality-indexed lookup,
/// scannable-field access, symmetric/directed relationship traversal, and
/// — via [`generic::production::GenericProductionStore`] — real mmap
/// durability and `RwLock` concurrency, generalized from `ProductionStore`
/// above rather than hardcoded to `Dog`. Validated against `Dog` and a
/// second, structurally different domain (`Order`/`Customer`, this
/// module's real reference implementation) across four spikes before
/// promotion — see this module's own doc comment for the full history,
/// and ADR-0009 (Accepted) for the acceptance record. New, parallel
/// capability: nothing above changed to build this.
pub mod generic;
/// Historical validation spikes that led to `generic` above — kept as the
/// measurement record, not part of the recommended API surface. See its
/// own module docs.
pub mod generic_spike;
pub mod production;
pub mod record;
pub mod store;

pub use generator::{generate, generate_littermates, GeneratorConfig};
pub use production::ProductionStore;
pub use record::DogRecord;
pub use store::{DogStore, StoreError};
