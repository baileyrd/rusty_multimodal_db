//! The `DogStore` trait shared by all four backends, and its error type.
//!
//! See `docs/decisions/ADR-0001-three-backend-empirical-comparison.md` for
//! why the first three implementations are compared behind one trait,
//! `docs/decisions/ADR-0003-eager-write-through-cache-invalidation.md` for
//! the fourth (`CanonicalCachedStore`), and
//! `docs/specifications/storage/STORAGE-002-dogstore-backends.md` /
//! `STORAGE-005-canonical-cached-backend.md` for the requirements each
//! implementation satisfies.

pub mod aos;
pub mod canonical;
pub mod canonical_cached;
pub mod soa;

pub use aos::AosStore;
pub use canonical::CanonicalStore;
pub use canonical_cached::CanonicalCachedStore;
pub use soa::SoaStore;

use crate::record::DogRecord;
use thiserror::Error;
use uuid::Uuid;

/// The one fallible operation across all four backends.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    /// `update_age` was called with a UUID no record has.
    #[error("no record found for id {0}")]
    NotFound(Uuid),
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
    /// for a one-hop graph-view access pattern. Returns an empty `Vec` if
    /// `id` is unknown or no other record shares its breed.
    fn same_breed(&self, id: Uuid) -> Vec<Uuid>;
}
