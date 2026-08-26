//! `DogRecord` implementing the generic schema traits, following the
//! design doc's §3 example and §5's migration shape: `DogRecord` itself is
//! unmodified (`src/record.rs` is not touched — these are `impl` blocks
//! for a type from elsewhere in this crate, not changes to that type),
//! gaining only the trait impls below.

use super::store::{BaseStore, Indexed, Scanned, Symmetric};
use super::traits::{IndexedField, Record, ScannableField, SymmetricRelation};
use crate::record::DogRecord;
use uuid::Uuid;

pub struct Breed;
pub struct Age;
pub struct LittermateOf;

impl Record for DogRecord {
    type Id = Uuid;
    fn id(&self) -> Uuid {
        self.id
    }
}

impl IndexedField<Breed> for DogRecord {
    type Value = String;
    fn indexed_value(&self) -> &String {
        &self.breed
    }
}

impl ScannableField<Age> for DogRecord {
    type Value = u32;
    fn scannable_value(&self) -> u32 {
        self.age
    }
}

impl SymmetricRelation<LittermateOf> for DogRecord {}

/// The full composed stack for `Dog`: `BaseStore` (owns the records) ->
/// `Indexed<.., Breed>` (`same_breed`'s generalization) -> `Scanned<..,
/// Age>` (`scan_ages`'s generalization — the layer this spike measures) ->
/// `Symmetric<.., LittermateOf>` (`neighbors`'s generalization). Matches
/// the design doc's §3 table exactly: `ChildOf` has no `Dog` instantiation
/// (see `traits.rs`), so this stack has no `Reversed` layer.
pub type DogGenericStore = Symmetric<
    Scanned<Indexed<BaseStore<DogRecord>, DogRecord, Breed>, DogRecord, Age>,
    DogRecord,
    LittermateOf,
>;

/// Build the full generic-path store for a dataset — the generic-path
/// analogue of `CanonicalCachedStore::new`.
pub fn build_dog_generic_store(records: &[DogRecord], edges: &[(Uuid, Uuid)]) -> DogGenericStore {
    let base = BaseStore::new(records.to_vec());
    let indexed = Indexed::<_, DogRecord, Breed>::new(base, records);
    let scanned = Scanned::<_, DogRecord, Age>::new(indexed, records);
    Symmetric::<_, DogRecord, LittermateOf>::new(scanned, edges)
}

#[cfg(test)]
mod tests {
    use super::super::query::{FilterEq, GetById, Neighbors, ScanField};
    use super::*;

    fn sample() -> Vec<DogRecord> {
        vec![
            DogRecord::new(Uuid::from_u128(1), "labrador", 3),
            DogRecord::new(Uuid::from_u128(2), "labrador", 5),
            DogRecord::new(Uuid::from_u128(3), "poodle", 2),
        ]
    }

    fn sample_edges() -> Vec<(Uuid, Uuid)> {
        vec![(Uuid::from_u128(1), Uuid::from_u128(2))]
    }

    /// The compile-and-run check the task's own check-in trigger names:
    /// does the design doc's trait set actually compile and behave
    /// correctly against `Dog` as sketched? All four capabilities the
    /// stack claims to forward/own are exercised here in one test.
    #[test]
    fn full_stack_get_scan_filter_and_neighbors_all_work() {
        let store = build_dog_generic_store(&sample(), &sample_edges());

        assert_eq!(store.get(Uuid::from_u128(1)).unwrap().breed, "labrador");
        assert_eq!(store.get(Uuid::from_u128(99)), None);

        let mut ages = store.scan();
        ages.sort_unstable();
        assert_eq!(ages, vec![2, 3, 5]);

        let mut labs = store.filter_eq(&"labrador".to_string());
        labs.sort();
        let mut expected = vec![Uuid::from_u128(1), Uuid::from_u128(2)];
        expected.sort();
        assert_eq!(labs, expected);

        assert_eq!(
            store.neighbors(Uuid::from_u128(1)),
            vec![Uuid::from_u128(2)]
        );
        assert!(store.neighbors(Uuid::from_u128(3)).is_empty());
    }

    #[test]
    fn update_field_writes_through_the_scan_cache() {
        use super::super::query::UpdateField;

        let mut store = build_dog_generic_store(&sample(), &sample_edges());
        store.update(Uuid::from_u128(1), 99).unwrap();
        assert!(store.scan().contains(&99));

        let err = store.update(Uuid::from_u128(99), 1).unwrap_err();
        assert_eq!(err.0, Uuid::from_u128(99));
    }
}
