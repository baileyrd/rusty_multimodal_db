//! Implementation spike for `docs/design/GENERIC-SCHEMA-DESIGN.md`.
//!
//! # History (four rounds so far)
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
//! scannable-field, symmetric-only shape never exercised, and finds it's
//! worse than expected: a same-trait multi-marker case that hits a real
//! Rust coherence limit (`E0119`), not just a naming collision. Renames
//! `IndexedField`/`ScannableField`'s associated types to `IndexValue`/
//! `ScanValue` (fixes the cross-trait case; doesn't touch the coherence
//! one).
//!
//! **Round 3** ([`store::forward_scannable_pairs`]) replaces round 2's
//! hand-written per-marker-pair forwarding impls (the coherence-limit
//! workaround) with a `macro_rules!` that generates them from a field
//! list — the O(pairs) compiler cost stays, the human-maintained surface
//! shrinks to one invocation.
//!
//! **Round 4** ([`order_naive`]/[`order_bench_support`]) measures the
//! design doc's last untested §4 risk: does the adjacency-index pattern
//! that made `littermate_of` traversal ~100,000× faster than a linear
//! scan generalize from a symmetric relation to `Order belongs_to
//! Customer`, a directed one-to-many one? See `order_naive`'s module docs
//! for the naive baseline and `benches/order_relation_spike.rs` for the
//! numbers.
//!
//! **`Dog` is done being built on**: it was a benchmark fixture for
//! storage-engine mechanics, never a target domain, and every
//! generalization round from round 2 forward targets `Order`/`Customer`
//! (or whatever comes after it). [`dog_impl`] stays as historical
//! reference (round 1's numbers still stand), not extended further.
//!
//! # Isolation (deliberate)
//!
//! No round touches [`crate::production::ProductionStore`],
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
//! traits, including `ChildOf`/`Parent`/`Children` (added for round 2 —
//! round 1 left them undefined/unimplemented since `Dog` has no directed
//! relation). [`store`] implements the composable wrapper layers
//! (`BaseStore`/`Indexed`/`Scanned`/`Symmetric`/`Reversed`) for real, which
//! is what makes the forwarding-boilerplate tax (design doc §4.5) show up
//! here too, not just in the design doc's own scratch crate — see
//! `store.rs`'s module docs.

pub mod dog_impl;
pub mod order_bench_support;
pub mod order_impl;
pub mod order_naive;
pub mod query;
pub mod store;
pub mod traits;

pub use dog_impl::{build_dog_generic_store, DogGenericStore};
pub use order_impl::{build_order_generic_store, OrderGenericStore};
pub use order_naive::NaiveOrderStore;

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
