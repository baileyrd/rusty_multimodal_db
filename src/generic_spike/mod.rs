//! Historical validation spikes for `crate::generic` — kept as the
//! measurement record for the four rounds that validated the design before
//! it was promoted (see `crate::generic`'s own module docs for the full
//! account), not deleted once superseded. **The generic trait/query/store
//! definitions and the `Order`/`Customer` domain themselves have moved** —
//! `traits.rs`/`query.rs`/`store.rs`/`order_impl.rs` no longer live here;
//! they're `crate::generic::{traits, query, store}` and
//! `crate::generic::order_customer` now, the crate's real, promoted
//! library. What remains in this module is the two pieces that are
//! genuinely spike-only, not real API surface:
//!
//! - [`dog_impl`] — `DogRecord`'s generic trait impls (`Record`,
//!   `IndexedField<Breed>`, `ScannableField<Age>`,
//!   `SymmetricRelation<LittermateOf>`) and `benches/generic_spike.rs`'s
//!   `get`/`scan_ages` overhead measurement against `crate::generic`.
//!   `Dog` is done being built on (a benchmark fixture, not a target
//!   domain — every generalization round from the second spike forward
//!   targeted `Order`/`Customer`), so this stays historical reference, not
//!   promoted or extended further.
//! - [`order_naive`]/[`order_bench_support`] — the naive linear-scan
//!   baseline and synthetic dataset generator `benches/order_relation_spike.rs`
//!   used to measure whether the adjacency-index pattern generalizes to a
//!   directed relation. The measurement question this answered is closed
//!   (see `crate::generic`'s docs and `RESULTS.md`'s `## Generic schema
//!   library` section); the naive baseline itself has no reason to become
//!   real API, so it stays here.
//! - [`rule_trace`]/[`rule_bench_support`] — a fifth spike round, testing
//!   `crate::generic` against a real, external requirements-traceability
//!   domain (`Rule`/`RuleRelation`) rather than another synthetic
//!   two-domain-validation exercise: recursive parent-chain traversal
//!   (composes cleanly via a plain loop over `Parent`, no new trait
//!   needed — see [`rule_trace`]'s own module docs for the one real
//!   schema wrinkle this surfaced) and multiple relation kinds between
//!   the same record type (hits the identical `E0119` coherence conflict
//!   multiple `ScannableField`s did, fixed by the identical
//!   `forward_scannable_pairs!`-style macro pattern, prototyped
//!   spike-locally as `forward_related_to_pairs!` rather than added to
//!   `crate::generic` itself — a promotion decision for a later round, not
//!   this one). `rule_trace` also carries `SelectionGroup` (a sixth-round
//!   addition, kept alongside `Rule` since its membership relation is
//!   modeled as `Rule` being a `ChildOf` a `SelectionGroup`).
//! - [`source`]/[`source_bench_support`] — the sixth spike round's
//!   `Source` piece: the same optional-nested-parent-chain shape `Rule`
//!   already established, applied to a different record, plus a
//!   root-lookup query (`domain_tags` only live on the root; a nested
//!   `Source`'s effective tags come from walking up to it).
//! - [`rule_derivation`] — the sixth spike round's `RuleDerivation`
//!   piece: a directed `Rule`-to-`Rule` "elaborates on" link, deliberately
//!   modeled via a *separate* trait triad from `rule_trace`'s own
//!   `RuleRelation` one (`DerivationRelation`/`DerivesFrom`/`Derived`,
//!   not `DirectedRelation`/`RelatedTo`/`DirectedRelated`) — see that
//!   module's own docs for why derivation links are firewalled from
//!   `RuleRelation`'s binding-dependency graph at the type level, not
//!   just by convention.
//!
//! # Isolation (unchanged from every prior round)
//!
//! Nothing here — or in `crate::generic` — touches
//! [`crate::production::ProductionStore`], [`crate::store::DogStore`], or
//! any benchmarked backend. [`dog_impl::DogRecord`] impls are additive
//! `impl` blocks on the existing [`crate::record::DogRecord`] type.

pub mod dog_impl;
pub mod order_bench_support;
pub mod order_naive;
pub mod rule_bench_support;
pub mod rule_derivation;
pub mod rule_trace;
pub mod source;
pub mod source_bench_support;

pub use dog_impl::{build_dog_generic_store, DogGenericStore};
pub use order_naive::NaiveOrderStore;
