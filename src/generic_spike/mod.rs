//! Implementation spike for `docs/design/GENERIC-SCHEMA-DESIGN.md`.
//!
//! # History (two rounds so far)
//!
//! **Round 1** ([`dog_impl`]) measured what genericizing `Dog`'s
//! schema/query surface costs (§4's central question: does the packed-
//! `Vec` column cache behind `CanonicalCachedStore::scan_ages`'s speed
//! survive genericity). It did — negligible overhead — but also
//! surfaced a real ambiguous-associated-type compile error in a
//! forwarding impl (`R::Value` ambiguous between `ScannableField`'s and
//! `IndexedField`'s own `Value`), the same failure class the design
//! doc's own scratch crate reported catching once already (§4.3).
//!
//! **Round 2** ([`order_impl`]) diagnoses that ambiguity pattern's real
//! scope on `Order`/`Customer` — a genuinely harder composition (two
//! `ScannableField`s, a directed `ChildOf` relation) that Dog's single-
//! scannable-field, symmetric-only shape never exercised. **Dog is done
//! being built on**: it was a benchmark fixture for storage-engine
//! mechanics, never a target domain, and every generalization round from
//! here forward targets `Order`/`Customer` (or whatever comes after it).
//! [`dog_impl`] stays as historical reference (round 1's numbers still
//! stand), not extended further.
//!
//! # Isolation (deliberate)
//!
//! Neither round touches [`crate::production::ProductionStore`],
//! [`crate::store::DogStore`], or any benchmarked backend
//! (`AosStore`/`SoaStore`/`CanonicalStore`/`CanonicalCachedStore`) —
//! [`dog_impl::DogRecord`] impls are additive `impl` blocks on the
//! existing [`crate::record::DogRecord`] type, per the design doc's §5
//! migration shape. Throwaway-quality is acceptable here — the question
//! this module exists to answer is whether the design is worth hardening
//! into something real, not whether it already is.
//!
//! # What's implemented
//!
//! [`traits`]/[`query`] transcribe the design doc's §1/§2 schema and query
//! traits, now including `ChildOf`/`Parent`/`Children` (added for round 2 —
//! round 1 left them undefined/unimplemented since `Dog` has no directed
//! relation). [`store`] implements the composable wrapper layers
//! (`BaseStore`/`Indexed`/`Scanned`/`Symmetric`/`Reversed`) for real, which
//! is what makes the forwarding-boilerplate tax (design doc §4.5) show up
//! here too, not just in the design doc's own scratch crate — see
//! `store.rs`'s module docs.

pub mod dog_impl;
pub mod order_impl;
pub mod query;
pub mod store;
pub mod traits;

pub use dog_impl::{build_dog_generic_store, DogGenericStore};
pub use order_impl::{build_order_generic_store, OrderGenericStore};

use std::fmt;

/// Error returned by [`query::UpdateField::update`] when `id` has no
/// record — the generic analogue of `StoreError::NotFound`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotFound<Id>(pub Id);

impl<Id: fmt::Debug> fmt::Display for NotFound<Id> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "no record with id {:?}", self.0)
    }
}

impl<Id: fmt::Debug> std::error::Error for NotFound<Id> {}
