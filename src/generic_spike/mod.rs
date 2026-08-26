//! Implementation spike measuring what genericizing `Dog`'s schema/query
//! surface costs, per `docs/design/GENERIC-SCHEMA-DESIGN.md`'s §4 —
//! specifically whether the packed-`Vec` column cache behind
//! `CanonicalCachedStore::scan_ages`'s speed survives once the field is
//! reached through the generic traits instead of hardcoded as `age: u32`.
//! That was a projection in the design doc, not a measurement; this
//! module (plus `benches/generic_spike.rs`) is what turns it into one.
//!
//! # Isolation (deliberate)
//!
//! This module implements the design doc's traits for `Dog` only (not
//! `Order`/`Customer` — the design doc already validated genericity
//! structurally against both domains; this spike measures cost on the one
//! domain with real benchmark numbers to compare against). It does not
//! touch [`crate::production::ProductionStore`], [`crate::store::DogStore`],
//! or any benchmarked backend (`AosStore`/`SoaStore`/`CanonicalStore`/
//! `CanonicalCachedStore`) — [`dog_impl::DogRecord`] impls below are
//! additive `impl` blocks on the existing [`crate::record::DogRecord`]
//! type, per the design doc's §5 migration shape ("`DogRecord` itself
//! changes by *addition*, not by rewrite"). Throwaway-quality is
//! acceptable here per the task that motivated this spike — the question
//! this module exists to answer is whether it's worth hardening into
//! something real, not whether it already is.
//!
//! # What's implemented, and what's deliberately not
//!
//! [`traits`]/[`query`] transcribe the design doc's §1/§2 schema and query
//! traits. [`store`] implements the composable wrapper layers (`BaseStore`/
//! `Indexed`/`Scanned`/`Symmetric`) for real, which is what makes the
//! forwarding-boilerplate tax (design doc §4.5) show up here too, not just
//! in the design doc's own scratch crate — see `store.rs`'s module docs.
//! `ChildOf`/`Parent`/`Children` (the directed-relation side, relevant to
//! `Order`/`Customer`, not `Dog`) are out of scope this round per the
//! task's own instruction.

pub mod dog_impl;
pub mod query;
pub mod store;
pub mod traits;

pub use dog_impl::{build_dog_generic_store, DogGenericStore};

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
