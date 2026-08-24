//! The fixed record shape under test.
//!
//! Deliberately three fields, no generic schema support — see
//! `docs/charter/CHARTER.md`'s non-goals and ADR-0001.

use uuid::Uuid;

/// A single synthetic dog record: a UUID identity, a breed name, and an age
/// in years.
#[derive(Debug, Clone, PartialEq, Eq)]
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
