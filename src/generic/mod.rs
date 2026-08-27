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
//! # Write-through consistency, in both the durable and in-memory paths
//!
//! `GetById::get` reflects a field `UpdateField::update` just wrote, on
//! both the durable core ([`mmap_store::GenericMmapStore`]) and the
//! purely in-memory [`store::BaseStore`]/[`store::Indexed`]/[`store::Scanned`]
//! composition — the same guarantee every hand-written backend in this
//! crate has (`CanonicalCachedStore::update_age` mutates both its
//! canonical record and its cache). The two paths get there by different
//! mechanisms, since they have structurally different shapes:
//!
//! - [`mmap_store::GenericMmapStore`] is a single hand-fused struct that
//!   owns both the (possibly stale) constructed-from record and the live
//!   mmap value directly — its `get` merges the two on read, via
//!   [`traits::ScannableField::set_scannable_value`].
//! - [`store::Scanned`] is a separate struct layered *on top of* whatever
//!   owns the record (typically [`store::BaseStore`], several layers
//!   down) — it has no way to reach down and mutate that owner's storage.
//!   Its `GetById` forwarding impl instead patches the record it gets
//!   back from its inner store with its own cached value, using the same
//!   `set_scannable_value`, before returning it. When multiple `Scanned`
//!   layers stack (e.g. `Order`'s `Amount`/`CreatedAt`/`DiscountCents`),
//!   each one patches only its own field as `get` unwinds back up through
//!   the stack, so the record is fully consistent by the time it reaches
//!   the caller — no change needed in `Indexed`/`Symmetric`/`Reversed`,
//!   none of which own any `ScannableField` data to patch.
//!
//! **A real, measured cost, not free**: unlike the durable core (where the
//! merge replaces work that already had to happen), the in-memory fix adds
//! one `HashMap` lookup per `Scanned` layer on every `get` call. Measured
//! directly (same-session, back-to-back, `benches/generic_spike.rs`'s
//! `generic_get` on `Dog`'s single-`Scanned`-layer stack): roughly 43–88%
//! slower across 1K/100K/1M records than before this fix. `scan`/`scan_ages`
//! and every other capability are untouched and unaffected — see
//! `RESULTS.md`'s `## Generic schema library` section for the full
//! numbers. This is the accepted cost of the correctness guarantee, not
//! an unexamined regression.

pub mod mmap_field;
pub mod mmap_store;
/// `Order`/`Customer` — this library's reference implementation, proving
/// the design against a second, structurally different domain than `Dog`.
/// Not part of the recommended path to build *your own* domain (see
/// [`super`]'s top-level doc comment for what is); kept as evidence,
/// gated behind the `research` feature.
#[cfg(feature = "research")]
pub mod order_customer;
pub mod production;
pub mod query;
pub mod store;
pub mod traits;

pub use mmap_store::GenericMmapStore;
#[cfg(feature = "research")]
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
