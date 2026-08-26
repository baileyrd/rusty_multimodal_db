//! A generic record/schema/query library: any domain that implements the
//! trait set in [`traits`] gets equality-indexed lookup, scannable-field
//! access, and symmetric/directed relationship traversal, composed from
//! reusable store layers ([`store`]) — including real durability via mmap
//! ([`mmap_store`]) and concurrent access via `RwLock` ([`production`]),
//! the same recipe `crate::production::ProductionStore` uses for `Dog`,
//! generalized.
//!
//! # From design, to spike, to library
//!
//! `docs/design/GENERIC-SCHEMA-DESIGN.md` and ADR-0009 proposed this
//! design; four validation spikes (`src/generic_spike/`, kept around as
//! historical record, not deleted) tested it against real code and real
//! benchmarks before any of it was treated as accepted:
//!
//! 1. **Dog overhead** — does genericizing `Dog`'s schema/query surface
//!    cost anything relative to `CanonicalCachedStore`? Negligible — but
//!    surfaced a real ambiguous-associated-type compile error.
//! 2. **Ambiguity diagnosis** (`Order`/`Customer`) — the ambiguity is
//!    worse than round 1 suggested: a same-trait multi-marker case hits a
//!    real Rust coherence limit (`E0119`), not just a naming collision.
//!    `IndexedField`/`ScannableField`'s associated types are renamed
//!    `IndexValue`/`ScanValue` (fixes the cross-trait case).
//! 3. **Macro-generated forwarding** — `forward_scannable_pairs!`
//!    (`store.rs`) generates the O(pairs) concrete forwarding impls the
//!    coherence limit requires, from a field list, so the human-maintained
//!    surface is one macro invocation, not hand-written impls per pair.
//! 4. **Directed-relation generalization** — does the adjacency-index
//!    pattern that made `littermate_of` traversal ~100,000× faster than a
//!    linear scan generalize to a *directed* relation
//!    (`Order belongs_to Customer`)? Yes, same order of magnitude,
//!    measured and explained.
//!
//! Every risk `docs/design/GENERIC-SCHEMA-DESIGN.md` §4 named has now been
//! individually resolved with real data. This module is that validated
//! design, promoted: [`traits`]/[`query`]/[`store`] are the same trait/
//! wrapper shapes the spikes proved out (not a rewrite), and
//! [`order_customer`] is the `Order`/`Customer` domain promoted from
//! prototype status to this library's real reference implementation.
//! [`mmap_store`]/[`production`] are new this round — the actual point of
//! the whole arc: a real generalized *production* store, not just traits
//! that compile in isolation. See ADR-0009 (now Accepted) for the full
//! acceptance record.
//!
//! # `Dog`/`ProductionStore` are not touched or replaced
//!
//! This module adds new, parallel capability. `crate::production::ProductionStore`,
//! `crate::store::DogStore`, and every benchmarked backend remain exactly
//! as they were — still the empirically-validated recommendation for the
//! `Dog` benchmark work this crate started as. Nothing in `src/generic/`
//! is wired into any of them, and nothing in `src/production.rs` or
//! `src/store/**` changed to build this module.
//!
//! # A known limitation, found while building this, not hidden
//!
//! `GetById::get` on a store composed purely from the in-memory
//! [`store::BaseStore`]/[`store::Indexed`]/[`store::Scanned`] layers does
//! **not** reflect a field `UpdateField::update` just wrote — `Scanned`
//! only updates its own cache, never writes through to `BaseStore`'s
//! records map, unlike every hand-written backend in this crate
//! (`CanonicalCachedStore::update_age` mutates both). [`mmap_store::GenericMmapStore`]
//! (the durable core — what [`production::GenericProductionStore`] is
//! actually built on) does **not** have this gap: it's a single hand-fused
//! struct, not a composition of separately-owned layers, so its `get`
//! merges the live mmap value directly (see that module's own docs, and
//! [`traits::ScannableField::set_scannable_value`], the method added to
//! make that merge possible). Closing this gap for the purely in-memory
//! composition (`DogGenericStore`/`OrderGenericStore`, still used by the
//! historical spikes) would need the same O(N²) marker-pair treatment
//! `forward_scannable_pairs!` already gives `ScanField`/`UpdateField` —
//! not attempted this round; flagged in `docs/PROJECT-STATUS.md` as
//! unscoped follow-up work, not silently worked around.

pub mod mmap_field;
pub mod mmap_store;
pub mod order_customer;
pub mod production;
pub mod query;
pub mod store;
pub mod traits;

pub use mmap_store::GenericMmapStore;
pub use order_customer::{
    build_order_generic_store, create_order_production_stack, open_order_production_stack, Order,
    OrderGenericStore, OrderProductionStack,
};
pub use production::GenericProductionStore;

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
