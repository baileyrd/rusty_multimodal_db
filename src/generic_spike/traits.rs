//! The generic schema traits from `docs/design/GENERIC-SCHEMA-DESIGN.md`
//! §1 — originally transcribed verbatim, now with one deliberate deviation:
//! [`IndexedField`]/[`ScannableField`]'s associated types are named
//! `IndexValue`/`ScanValue`, not both `Value`, per a follow-up task's
//! diagnosis of a real ambiguous-associated-type bug this design hits in
//! practice (see those two traits' own doc comments, and `store.rs`'s and
//! `order_impl.rs`'s module docs, for the full account). This is a real
//! change to the design doc's public trait signatures, prototyped here for
//! review — `GENERIC-SCHEMA-DESIGN.md`/`ADR-0009` are deliberately not
//! updated yet.

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
/// diagnosis this module's docs summarize: a bare `R::Value` is
/// ambiguous whenever `R` also implements [`ScannableField`] (which also
/// declared an assoc type named `Value`) for any marker, which is the
/// *common* case for a real record with more than one field kind (e.g.
/// `Order`, indexed on `Status` and scannable on `Amount`/`CreatedAt`).
/// Giving each trait its own name removes that specific, cross-trait
/// collision. It does **not** remove every instance of this ambiguity
/// class — see [`ScannableField`]'s own doc comment for the one it can't
/// touch.
pub trait IndexedField<Marker>: Record {
    type IndexValue: Eq + Hash + Clone;
    fn indexed_value(&self) -> &Self::IndexValue;
}

/// `R` has a scannable/aggregatable field. `IndexValue: Copy` is
/// deliberately a *tighter* bound than `IndexedField`'s — see the design
/// doc's §4.1 for why: it's what lets the packed-`Vec` cache trick this
/// spike measures exist at all.
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
/// "this marker is not that marker." See `store.rs`'s module docs and
/// `order_impl.rs`'s concrete, per-marker-pair forwarding impls, the only
/// way found to make this compile.
pub trait ScannableField<Marker>: Record {
    type ScanValue: Copy;
    fn scannable_value(&self) -> Self::ScanValue;
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
