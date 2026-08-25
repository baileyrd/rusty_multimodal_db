//! The fixed record shape under test.
//!
//! Deliberately three fields, no generic schema support — see
//! `docs/charter/CHARTER.md`'s non-goals and ADR-0001.

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
