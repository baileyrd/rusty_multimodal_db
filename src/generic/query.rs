//! The generic query-capability traits — promoted from
//! `docs/design/GENERIC-SCHEMA-DESIGN.md` §2, exercised end to end (by the
//! Dog and Order/Customer domains, in-memory and mmap-durable) before
//! promotion. `GetById` generalizes `get`, `FilterEq` generalizes
//! `same_breed`, `ScanField`/`UpdateField` generalize `scan_ages`/
//! `update_age`, `Neighbors` generalizes `neighbors` for a symmetric
//! relation, `Parent`/`Children` are the directed-relation analogues (one
//! hop up, one hop down).

use super::traits::{ChildOf, IndexedField, Record, ScannableField, SymmetricRelation};

/// Generalizes `get` — look up a record by its id.
pub trait GetById<R: Record> {
    fn get(&self, id: R::Id) -> Option<R>;
}

/// Every id this store holds, unspecified order — the one primitive
/// `SERVER-001`'s `Request::Query` needs and no existing query trait
/// exposes (`SQL-FR-005`, ADR-0034, `docs/design/SERVER-SQL-SELECT-DESIGN.md`):
/// `GetById` needs an id already in hand, `FilterEq` needs an index,
/// `ScanField` returns values with no id attached.
pub trait AllIds<R: Record> {
    fn all_ids(&self) -> Vec<R::Id>;
}

/// Generalizes `same_breed` — filter by equality on any `IndexedField`.
pub trait FilterEq<R, Marker>
where
    R: IndexedField<Marker>,
{
    fn filter_eq(&self, value: &R::IndexValue) -> Vec<R::Id>;
}

/// Generalizes `scan_ages`.
pub trait ScanField<R, Marker>
where
    R: ScannableField<Marker>,
{
    fn scan(&self) -> Vec<R::ScanValue>;
}

/// Generalizes `update_age`.
pub trait UpdateField<R, Marker>
where
    R: ScannableField<Marker>,
{
    fn update(&mut self, id: R::Id, value: R::ScanValue) -> Result<(), super::NotFound<R::Id>>;
}

/// Generalizes `neighbors` for a *symmetric* relation.
pub trait Neighbors<R, Marker>
where
    R: SymmetricRelation<Marker>,
{
    fn neighbors(&self, id: R::Id) -> Vec<R::Id>;
}

/// More than one named symmetric relation on the same record type `R`,
/// each identified by a runtime string label rather than a compile-time
/// marker type — `ENT2-FR-002`/`003` (ADR-0039). **Not** a generalization
/// of [`Neighbors`]: a `Symmetric<S, R, Marker>` already answers
/// `Neighbors<R, Marker>` for its own compile-time `Marker` via `self`'s
/// own adjacency map, and a second, independent `impl<.., R2, RelMarker>
/// Neighbors<R2, RelMarker> for Symmetric<..>` generic forwarding impl —
/// mirroring `Reversed`'s own `Neighbors`-forwarding fix (`FR-012`) —
/// was tried and confirmed (directly, with `rustc`) to conflict with that
/// existing direct impl: `Reversed` has no competing direct `Neighbors`
/// impl of its own to conflict with (its own relation is `ChildOf`, a
/// different trait entirely), but `Symmetric` does, so the identical
/// trick does not generalize. `MultiNeighbors` sidesteps the conflict
/// entirely by not using the marker-typed `Neighbors` trait for the
/// multi-relation case at all — a deliberate, load-bearing choice, not
/// an oversight, and one that happens to match the wire protocol's own
/// shape exactly: `Request::NeighborsByRelation`'s `relation` field was
/// always a runtime `String`, never a compile-time type.
pub trait MultiNeighbors<R: Record> {
    /// `None` if `relation` names no relation this store has at all
    /// (an unknown label — `ErrorCode::Malformed` at the wire boundary);
    /// `Some` (possibly empty) otherwise.
    fn neighbors_by_relation(&self, relation: &str, id: R::Id) -> Option<Vec<R::Id>>;

    /// The union of every named relation's neighbors — what a plain,
    /// relation-unfiltered `Request::Neighbors` answers.
    fn all_neighbors(&self, id: R::Id) -> Vec<R::Id>;

    /// Every relation label this store knows, unspecified order.
    fn relation_kinds(&self) -> Vec<String>;
}

/// A record that resolves under more than one string key — a primary
/// name plus zero or more aliases — `ENT3-FR-002`/`004` (ADR-0040). The
/// runtime-keyed analogue of [`super::traits::IndexedField`] for a
/// caller who doesn't yet have a record's id, only one of the strings it
/// might be called by. **Not** a second `IndexedField`: `GenericMmapStore`
/// structurally admits exactly one `IndexedField` marker per record
/// type, and a domain whose one index slot is already taken (`Entity`'s
/// `kind`) needs a separate mechanism, not a second marker — the same
/// "new wrapper primitive, not a rework of the structural constraint"
/// call [`MultiNeighbors`] made for relations one round earlier.
///
/// Keys are returned **un-normalized**; the [`super::store::NameIndex`]
/// layer normalizes every key identically at build time and at query
/// time (`ENT3-FR-003`), so normalization lives in exactly one place.
pub trait NameIndexed: Record {
    /// Every string this record should resolve under.
    fn index_keys(&self) -> Vec<String>;
}

/// Resolve every record id registered under `name`, normalized —
/// `ENT3-FR-005`'s in-process half. Zero, one, or many ids: two records
/// sharing a normalized key both come back, collision handling is the
/// caller's (see `docs/design/SERVER-ENTITY-ALIASES-DESIGN.md`'s own
/// Non-goals).
pub trait FindByName<R: NameIndexed> {
    fn find_by_name(&self, name: &str) -> Vec<R::Id>;
}

/// The "one hop up" side of a *directed* relation. Has a blanket impl
/// (`store.rs`) over anything providing `GetById<C>` — no new index
/// needed, the foreign key already lives on the child.
///
/// Returns `Result<Option<C::ParentId>, super::NotFound<C::Id>>`, not a
/// bare `Option` — matching `GetById::get`'s own "not found" shape
/// (`None`) folded into this trait's `Err`, so the two cases a caller
/// might otherwise confuse stay distinct: `Err` means `child_id` isn't a
/// real record at all, `Ok(None)` means it is a real record with no
/// parent, `Ok(Some(parent))` means it is a real record with one. See
/// `store.rs`'s blanket impl for why a single-level `Option` (this
/// trait's shape before this fix) couldn't tell those apart.
pub trait Parent<C, Marker>
where
    C: ChildOf<Marker>,
{
    fn parent(&self, child_id: C::Id) -> Result<Option<C::ParentId>, super::NotFound<C::Id>>;
}

/// The "one hop down" side of a *directed* relation — the expensive
/// direction, needing a real reverse index (`store::Reversed`).
pub trait Children<P, C, Marker>
where
    P: Record,
    C: ChildOf<Marker, ParentId = P::Id>,
{
    fn children(&self, parent_id: P::Id) -> Vec<C::Id>;
}
