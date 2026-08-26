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
pub mod production;
pub mod record;
pub mod store;

pub use generator::{generate, generate_littermates, GeneratorConfig};
pub use production::ProductionStore;
pub use record::DogRecord;
pub use store::{DogStore, StoreError};
