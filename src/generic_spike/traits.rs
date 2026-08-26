//! The generic schema traits from `docs/design/GENERIC-SCHEMA-DESIGN.md`
//! §1, transcribed verbatim (not re-derived) so this spike measures the
//! design as actually proposed, not a convenient reinterpretation of it.

use std::hash::Hash;

/// Every domain record has an id.
pub trait Record {
    type Id: Copy + Eq + Hash;
    fn id(&self) -> Self::Id;
}

/// `R` has an equality-indexable field, identified by the zero-sized marker
/// type `Marker` (one marker per field).
pub trait IndexedField<Marker>: Record {
    type Value: Eq + Hash + Clone;
    fn indexed_value(&self) -> &Self::Value;
}

/// `R` has a scannable/aggregatable field. `Value: Copy` is deliberately a
/// *tighter* bound than `IndexedField`'s — see the design doc's §4.1 for
/// why: it's what lets the packed-`Vec` cache trick this spike measures
/// exist at all.
pub trait ScannableField<Marker>: Record {
    type Value: Copy;
    fn scannable_value(&self) -> Self::Value;
}

/// `R` participates in a symmetric (undirected) relation, identified by
/// `Marker` — the generalization of `littermate_of`.
pub trait SymmetricRelation<Marker>: Record {}

/// `R` is the *child* side of a directed, many-to-one relation, identified
/// by `Marker` — the generalization of `Order belongs_to Customer`.
///
/// Defined here for completeness with the design doc's §1 (all five schema
/// traits), but deliberately **not implemented for `DogRecord`** in this
/// spike: `Dog` has no directed relation (see the design doc's §3 table —
/// `ChildOf` is one of the blank cells for `Dog`), and the directed-
/// relation/adjacency-index question is explicitly out of scope for this
/// round (that's `Order`/`Customer`'s question, a different spike).
#[allow(dead_code)]
pub trait ChildOf<Marker>: Record {
    type ParentId: Copy + Eq + Hash;
    fn parent_id(&self) -> Self::ParentId;
}
