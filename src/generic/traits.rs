//! The generic schema traits — promoted from `docs/design/GENERIC-SCHEMA-DESIGN.md`
//! §1 by way of four validation spikes (`src/generic_spike/`), each of
//! which is what fixed one thing relative to the original design document:
//! `IndexedField`/`ScannableField`'s associated types are named
//! `IndexValue`/`ScanValue`, not both `Value` — the design doc's own
//! bare `Value` name is ambiguous the moment a record implements both
//! traits (any record with more than one field kind), a real compile
//! error the first spike hit and this rename fixes. See `store.rs`'s
//! module docs for the other half of that finding (the rename doesn't
//! fully solve the ambiguity pattern) and `docs/decisions/ADR-0009-generic-schema-design-proposal.md`
//! for the acceptance record.

use std::hash::Hash;

/// Every domain record has an id.
pub trait Record {
    type Id: Copy + Eq + Hash;
    fn id(&self) -> Self::Id;
}

/// `R` has an equality-indexable field, identified by the zero-sized marker
/// type `Marker` (one marker per field).
///
/// The associated type is named `IndexValue`, not `Value` — this is
/// "Candidate 1" (per-trait renaming) from the associated-type-ambiguity
/// diagnosis the `claude/generic-schema-ambiguity-fix` spike worked
/// through: a bare `R::Value` is ambiguous whenever `R` also implements
/// [`ScannableField`] (which also declared an assoc type named `Value`)
/// for any marker, which is the *common* case for a real record with more
/// than one field kind (e.g. `Order`, indexed on `Status` and scannable on
/// `Amount`/`CreatedAt`). Giving each trait its own name removes that
/// specific, cross-trait collision. It does **not** remove every instance
/// of this ambiguity class — see [`ScannableField`]'s own doc comment for
/// the one it can't touch.
pub trait IndexedField<Marker>: Record {
    type IndexValue: Eq + Hash + Clone;
    fn indexed_value(&self) -> &Self::IndexValue;
}

/// `R` has a scannable/aggregatable field. `IndexValue: Copy` is
/// deliberately a *tighter* bound than `IndexedField`'s — see the design
/// doc's §4.1 for why: it's what lets the packed-`Vec` cache trick
/// (`Scanned`, `store.rs`) exist at all, in memory or backed by mmap
/// (`mmap_store.rs`).
///
/// Named `ScanValue`, not `Value` — see [`IndexedField`]'s doc comment for
/// why the rename happened. **This rename does not fully solve the
/// ambiguity pattern it was made for.** A record with *two* scannable
/// fields (e.g. `Order`'s `Amount` and `CreatedAt`) implements
/// `ScannableField` twice, for two different `Marker`s — `R::ScanValue` is
/// just as ambiguous between those two instantiations as `R::Value` ever
/// was between `IndexedField`/`ScannableField`, because the ambiguity is
/// about *multiple trait bounds in scope sharing an associated-type name*,
/// not about which two traits happen to be involved. Worse: a *generic*
/// forwarding impl over "any other marker" for this case doesn't even
/// reach that ambiguity — it fails to compile at all, with `E0119`
/// (conflicting impls), since Rust's coherence checker can't express
/// "this marker is not that marker." See `store.rs`'s
/// `forward_scannable_pairs!` module docs and `order_customer.rs`'s single
/// macro invocation for the only way found to make this compile.
pub trait ScannableField<Marker>: Record {
    type ScanValue: Copy;
    fn scannable_value(&self) -> Self::ScanValue;

    /// Write a new value into this field on the record itself (not into
    /// any store-side cache) — added during promotion, not part of the
    /// original design doc. [`super::mmap_store::GenericMmapStore::get`]
    /// needs this to keep `GetById::get` write-through consistent with
    /// `UpdateField::update`, the same way every hand-written backend in
    /// this crate (`CanonicalCachedStore::update_age` included) mutates
    /// both its canonical record and its cache on every write. See
    /// `mmap_store.rs`'s module docs for the finding that motivated adding
    /// this method, and its limits: the in-memory `Scanned`/`BaseStore`
    /// composition (`store.rs`) does **not** yet call this on write, so
    /// `GetById::get` on a purely in-memory composed stack can still
    /// return a stale value for a field `UpdateField::update` just wrote —
    /// a known, documented gap, not silently papered over.
    fn set_scannable_value(&mut self, value: Self::ScanValue);
}

/// `R` participates in a symmetric (undirected) relation, identified by
/// `Marker` — the generalization of `littermate_of`.
pub trait SymmetricRelation<Marker>: Record {}

/// `R` is the *child* side of a directed, many-to-one relation, identified
/// by `Marker` — the generalization of `Order belongs_to Customer`.
pub trait ChildOf<Marker>: Record {
    type ParentId: Copy + Eq + Hash;
    fn parent_id(&self) -> Self::ParentId;
}
