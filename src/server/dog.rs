//! [`ConnectionStore`] adapter wrapping [`crate::production::ProductionStore`]
//! for `Dog` — the front-door validation domain (real symmetric relation,
//! `littermate_of`, via `Neighbors`; no directed relation, so `Parent`/
//! `Children` are unsupported here — see [`super::order`] for the
//! complementary case).

use super::protocol::{ErrorCode, FieldRef, ParentLookup, RecordId, ScanValue};
use super::ConnectionStore;
use crate::concurrency::{ConcurrencyError, ConcurrentStore};
use crate::store::{DogStore, StoreError};

/// `Dog::breed` — read-only over this protocol: no `ScannableField`/
/// `UpdateField` exists for it in-process either (only `age` is mutable,
/// via `update_age`).
pub const FIELD_BREED: FieldRef = 0;
/// `Dog::age` — the one mutable, scannable field.
pub const FIELD_AGE: FieldRef = 1;

/// Wraps any `S: DogStore + ConcurrentStore` (in practice,
/// [`crate::production::ProductionStore`], the only type implementing
/// both). Uses `DogStore`'s `&self` methods (`get`/`scan_ages`/
/// `same_breed`/`neighbors` all take `&self` already) plus
/// `ConcurrentStore::update_age` (the one `&self`-shaped mutator) — never
/// `DogStore::update_age`, which needs `&mut self` and so can't be called
/// through the `Arc<S>` every connection thread shares.
pub struct DogConnectionStore<S> {
    store: S,
}

impl<S> DogConnectionStore<S> {
    pub fn new(store: S) -> Self {
        Self { store }
    }
}

impl<S: DogStore + ConcurrentStore + Send + Sync> ConnectionStore for DogConnectionStore<S> {
    fn get(&self, id: RecordId) -> Option<Vec<(FieldRef, ScanValue)>> {
        DogStore::get(&self.store, id).map(|record| {
            vec![
                (FIELD_BREED, ScanValue::Str(record.breed)),
                (FIELD_AGE, ScanValue::U32(record.age)),
            ]
        })
    }

    fn filter_eq(&self, _field: FieldRef, _value: &ScanValue) -> Result<Vec<RecordId>, ErrorCode> {
        // No IndexedField-shaped "give me every record equal to this
        // value" exists for Dog in-process — `same_breed` filters by
        // *another record's id*, a different shape this protocol's
        // FilterEq (by value) doesn't represent. Named as an out-of-scope
        // gap for v1 (see docs/PROJECT-STATUS.md), not silently faked by
        // reinterpreting FilterEq as something it isn't.
        Err(ErrorCode::Unsupported)
    }

    fn scan_field(&self, field: FieldRef) -> Result<Vec<ScanValue>, ErrorCode> {
        match field {
            FIELD_AGE => Ok(DogStore::scan_ages(&self.store)
                .into_iter()
                .map(ScanValue::U32)
                .collect()),
            FIELD_BREED => Err(ErrorCode::Unsupported),
            _ => Err(ErrorCode::UnknownField),
        }
    }

    fn update_field(
        &self,
        id: RecordId,
        field: FieldRef,
        value: ScanValue,
    ) -> Result<bool, ErrorCode> {
        match (field, value) {
            (FIELD_AGE, ScanValue::U32(age)) => {
                match ConcurrentStore::update_age(&self.store, id, age) {
                    Ok(()) => Ok(true),
                    Err(ConcurrencyError::Store(StoreError::NotFound(_))) => Ok(false),
                    // A real I/O/durability failure, not a missing record —
                    // surfaced as a server error rather than misreported as
                    // NotFound.
                    Err(_) => Err(ErrorCode::Malformed),
                }
            }
            (FIELD_AGE, _) => Err(ErrorCode::Malformed),
            (FIELD_BREED, _) => Err(ErrorCode::Unsupported),
            _ => Err(ErrorCode::UnknownField),
        }
    }

    fn parent(&self, _id: RecordId) -> Result<ParentLookup, ErrorCode> {
        // Dog has no ChildOf-shaped directed relation.
        Err(ErrorCode::Unsupported)
    }

    fn children(&self, _id: RecordId) -> Result<Vec<RecordId>, ErrorCode> {
        Err(ErrorCode::Unsupported)
    }

    fn neighbors(&self, id: RecordId) -> Result<Vec<RecordId>, ErrorCode> {
        Ok(DogStore::neighbors(&self.store, id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::production::ProductionStore;
    use crate::record::DogRecord;
    use crate::test_support::fresh_temp_dir;
    use uuid::Uuid;

    fn sample_adapter() -> DogConnectionStore<ProductionStore> {
        let dir = fresh_temp_dir("server_dog_adapter").unwrap();
        let path = dir.join("dogs.mmap");
        let records = vec![
            DogRecord::new(Uuid::from_u128(1), "labrador", 3),
            DogRecord::new(Uuid::from_u128(2), "labrador", 5),
        ];
        let edges = vec![(Uuid::from_u128(1), Uuid::from_u128(2))];
        let store = ProductionStore::create(records, edges, &path).unwrap();
        DogConnectionStore::new(store)
    }

    #[test]
    fn get_returns_breed_and_age() {
        let adapter = sample_adapter();
        let fields = adapter.get(Uuid::from_u128(1)).unwrap();
        assert_eq!(
            fields,
            vec![
                (FIELD_BREED, ScanValue::Str("labrador".into())),
                (FIELD_AGE, ScanValue::U32(3)),
            ]
        );
        assert!(adapter.get(Uuid::from_u128(99)).is_none());
    }

    #[test]
    fn update_field_updates_age_and_reports_missing_ids() {
        let adapter = sample_adapter();
        assert_eq!(
            adapter.update_field(Uuid::from_u128(1), FIELD_AGE, ScanValue::U32(9)),
            Ok(true)
        );
        assert_eq!(
            adapter.get(Uuid::from_u128(1)).unwrap()[1],
            (FIELD_AGE, ScanValue::U32(9))
        );
        assert_eq!(
            adapter.update_field(Uuid::from_u128(99), FIELD_AGE, ScanValue::U32(1)),
            Ok(false)
        );
        assert_eq!(
            adapter.update_field(Uuid::from_u128(1), FIELD_AGE, ScanValue::Bool(true)),
            Err(ErrorCode::Malformed)
        );
    }

    #[test]
    fn neighbors_reflects_the_littermate_edge() {
        let adapter = sample_adapter();
        assert_eq!(
            adapter.neighbors(Uuid::from_u128(1)),
            Ok(vec![Uuid::from_u128(2)])
        );
    }

    #[test]
    fn filter_eq_and_parent_and_children_are_unsupported() {
        let adapter = sample_adapter();
        assert_eq!(
            adapter.filter_eq(FIELD_BREED, &ScanValue::Str("labrador".into())),
            Err(ErrorCode::Unsupported)
        );
        assert_eq!(
            adapter.parent(Uuid::from_u128(1)),
            Err(ErrorCode::Unsupported)
        );
        assert_eq!(
            adapter.children(Uuid::from_u128(1)),
            Err(ErrorCode::Unsupported)
        );
    }
}
