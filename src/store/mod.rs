//! The `DogStore` trait shared by all four backends, and its error type.
//!
//! See `docs/decisions/ADR-0001-three-backend-empirical-comparison.md` for
//! why the first three implementations are compared behind one trait,
//! `docs/decisions/ADR-0003-eager-write-through-cache-invalidation.md` for
//! the fourth (`CanonicalCachedStore`),
//! `docs/decisions/ADR-0004-one-hop-neighbors-trait-method.md` for why
//! `neighbors` stops at one hop, and
//! `docs/specifications/storage/STORAGE-002-dogstore-backends.md` /
//! `STORAGE-005-canonical-cached-backend.md` /
//! `STORAGE-006-littermate-graph-traversal.md` for the requirements each
//! implementation satisfies.

// The four backends below are the benchmark comparison this crate's
// charter set out to run — not the recommended path (see `lib.rs`'s own
// top-level doc comment). Gated behind the `research` feature; the
// `DogStore` trait/`StoreError` type below stay unconditional since
// `crate::production::ProductionStore` implements this trait directly.
// Also available under plain `#[cfg(test)]`: `bench_support`'s own tests
// use `AosStore` for a sanity check — see `canonical_cached` above for
// why this doesn't widen what an external consumer actually sees.
#[cfg(any(test, feature = "research"))]
pub mod aos;
#[cfg(feature = "research")]
pub mod canonical;
// Also available under plain `#[cfg(test)]` regardless of `research`:
// `concurrency::test_support::run_concurrency_stress_test` (used by
// `ProductionStore`'s own flagship, always-on test) replays writes
// against a fresh `CanonicalCachedStore` as its single-threaded
// reference — `#[cfg(test)]` code never ships to a downstream consumer's
// build regardless of feature flags, so this doesn't widen what an
// external consumer actually sees.
#[cfg(any(test, feature = "research"))]
pub mod canonical_cached;
#[cfg(feature = "research")]
pub mod soa;

#[cfg(any(test, feature = "research"))]
pub use aos::AosStore;
#[cfg(feature = "research")]
pub use canonical::CanonicalStore;
#[cfg(any(test, feature = "research"))]
pub use canonical_cached::CanonicalCachedStore;
#[cfg(feature = "research")]
pub use soa::SoaStore;

use crate::record::DogRecord;
use thiserror::Error;
use uuid::Uuid;

/// The one fallible operation across all four backends — extended, as of
/// the durability work (`STORAGE-008`/`STORAGE-009`), with a second
/// variant so the durability variants in `src/durability/` (each of which
/// implements this same trait, so they're usable and testable the same
/// way as the other four backends) can report a real I/O/serialization
/// failure through `update_age` rather than conflating it with
/// `NotFound` or panicking. This is an additive change to the shared
/// trait/error definitions (not a change to any of the four existing
/// backend implementations, which don't construct `Durability` and are
/// unaffected) — no exhaustive `match` on `StoreError` exists anywhere in
/// this crate (verified before adding the variant), so nothing else needed
/// updating. Dropped `Copy` from the derive since `Durability`'s `String`
/// payload isn't `Copy`; nothing relied on `StoreError` being `Copy`.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// `update_age` was called with a UUID no record has.
    #[error("no record found for id {0}")]
    NotFound(Uuid),
    /// A durability variant's `update_age` failed for a reason other than
    /// an unknown UUID — e.g. a WAL append, fsync, or snapshot write
    /// failed. Carries the underlying `DurabilityError`'s message rather
    /// than the error itself, so this trait/error module doesn't need to
    /// depend on `src/durability`.
    #[error("durability failure: {0}")]
    Durability(String),
}

/// Shared storage interface implemented by [`AosStore`] (row-oriented),
/// [`SoaStore`] (column-oriented), [`CanonicalStore`] (UUID-canonical with
/// derived views), and [`CanonicalCachedStore`] (UUID-canonical plus a
/// materialized age cache).
///
/// `get` and `same_breed` treat an unknown UUID as a normal empty result
/// (`None` / `Vec::new()`), not an error — "not found" is an ordinary
/// outcome for a lookup. Only `update_age` returns `Err` for an unknown
/// UUID, since a caller asking to mutate a record that doesn't exist is a
/// genuine failure to report.
pub trait DogStore {
    /// Full-record read by UUID. Returns an owned clone so the trait stays
    /// object-safe and identically shaped across all three backends —
    /// benchmark numbers for `get` include that clone's cost for every
    /// backend equally.
    fn get(&self, id: Uuid) -> Option<DogRecord>;

    /// Every record's `age`, in unspecified order. Stands in for a
    /// column-scan/aggregate access pattern (e.g. computing an average).
    fn scan_ages(&self) -> Vec<u32>;

    /// Update one record's `age` in place.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if `id` has no record.
    fn update_age(&mut self, id: Uuid, age: u32) -> Result<(), StoreError>;

    /// All UUIDs (excluding `id` itself) sharing `id`'s breed. Stands in
    /// for a one-hop graph-view access pattern over a shared *attribute*,
    /// not a real edge — see `neighbors` for an actual relationship.
    /// Returns an empty `Vec` if `id` is unknown or no other record shares
    /// its breed.
    fn same_breed(&self, id: Uuid) -> Vec<Uuid>;

    /// All UUIDs connected to `id` by a `littermate_of` edge, in either
    /// direction (the relationship is symmetric — if `a` is a littermate of
    /// `b`, `b` is a littermate of `a`). Unlike `same_breed`, this is a real
    /// one-hop graph traversal over a generated edge relationship, not a
    /// shared-attribute grouping. Returns an empty `Vec` if `id` is unknown
    /// or has no littermate edges.
    ///
    /// Deliberately one-hop only: multi-hop traversal (e.g. "littermates of
    /// littermates") is built generically on top of repeated `neighbors`
    /// calls in benchmark/test code (see
    /// `rusty_multimodal_db::bench_support::two_hop_neighbors`), not as a
    /// trait method — see
    /// `docs/decisions/ADR-0004-one-hop-neighbors-trait-method.md`.
    fn neighbors(&self, id: Uuid) -> Vec<Uuid>;
}
