//! The fixed record shape under test.
//!
//! Deliberately three fields, no generic schema support — see
//! `docs/charter/CHARTER.md`'s non-goals and ADR-0001.
//!
//! # Why this stays unconditionally `pub`, unlike the rest of the `Dog` story
//!
//! The `research` feature (see `lib.rs`'s own doc comment) gates away
//! the benchmarked-alternative backends/variants/strategies and the
//! dataset-generation infrastructure built around `Dog` — but not
//! [`DogRecord`] itself. [`crate::production::ProductionStore`] (front
//! door, unconditional) implements [`crate::store::DogStore`], whose
//! trait methods return/accept `DogRecord` directly; a real caller can't
//! use `ProductionStore` through that trait at all without `DogRecord`
//! being nameable. It's the one piece of the `Dog` domain that's part of
//! the public contract, not the evidence.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A single synthetic dog record: a UUID identity, a breed name, and an age
/// in years.
///
/// `Serialize`/`Deserialize` are new as of the durability work
/// (`STORAGE-008`/`STORAGE-009`) — every WAL/snapshot-based durability
/// variant needs to write and read `DogRecord`s. Purely additive (derives
/// don't change behavior); no other backend or field changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DogRecord {
    pub id: Uuid,
    pub breed: String,
    pub age: u32,
}

impl DogRecord {
    /// Build a record directly from its fields.
    pub fn new(id: Uuid, breed: impl Into<String>, age: u32) -> Self {
        Self {
            id,
            breed: breed.into(),
            age,
        }
    }
}
