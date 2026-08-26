//! Historical validation spikes for `crate::generic` — kept as the
//! measurement record for the four rounds that validated the design before
//! it was promoted (see `crate::generic`'s own module docs for the full
//! account), not deleted once superseded. **The generic trait/query/store
//! definitions and the `Order`/`Customer` domain themselves have moved** —
//! `traits.rs`/`query.rs`/`store.rs`/`order_impl.rs` no longer live here;
//! they're `crate::generic::{traits, query, store}` and
//! `crate::generic::order_customer` now, the crate's real, promoted
//! library. What remains in this module is the two pieces that are
//! genuinely spike-only, not real API surface:
//!
//! - [`dog_impl`] — `DogRecord`'s generic trait impls (`Record`,
//!   `IndexedField<Breed>`, `ScannableField<Age>`,
//!   `SymmetricRelation<LittermateOf>`) and `benches/generic_spike.rs`'s
//!   `get`/`scan_ages` overhead measurement against `crate::generic`.
//!   `Dog` is done being built on (a benchmark fixture, not a target
//!   domain — every generalization round from the second spike forward
//!   targeted `Order`/`Customer`), so this stays historical reference, not
//!   promoted or extended further.
//! - [`order_naive`]/[`order_bench_support`] — the naive linear-scan
//!   baseline and synthetic dataset generator `benches/order_relation_spike.rs`
//!   used to measure whether the adjacency-index pattern generalizes to a
//!   directed relation. The measurement question this answered is closed
//!   (see `crate::generic`'s docs and `RESULTS.md`'s `## Generic schema
//!   library` section); the naive baseline itself has no reason to become
//!   real API, so it stays here.
//!
//! # Isolation (unchanged from every prior round)
//!
//! Nothing here — or in `crate::generic` — touches
//! [`crate::production::ProductionStore`], [`crate::store::DogStore`], or
//! any benchmarked backend. [`dog_impl::DogRecord`] impls are additive
//! `impl` blocks on the existing [`crate::record::DogRecord`] type.

pub mod dog_impl;
pub mod order_bench_support;
pub mod order_naive;

pub use dog_impl::{build_dog_generic_store, DogGenericStore};
pub use order_naive::NaiveOrderStore;
