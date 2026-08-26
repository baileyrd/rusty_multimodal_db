//! The generic query-capability traits — promoted from
//! `docs/design/GENERIC-SCHEMA-DESIGN.md` §2, exercised end to end (by the
//! Dog and Order/Customer domains, in-memory and mmap-durable) before
//! promotion. `GetById` generalizes `get`, `FilterEq` generalizes
//! `same_breed`, `ScanField`/`UpdateField` generalize `scan_ages`/
//! `update_age`, `Neighbors` generalizes `neighbors` for a symmetric
//! relation, `Parent`/`Children` are the directed-relation analogues (one
//! hop up, one hop down).

use super::traits::{ChildOf, IndexedField, Record, ScannableField, SymmetricRelation};

pub trait GetById<R: Record> {
    fn get(&self, id: R::Id) -> Option<R>;
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

/// The "one hop up" side of a *directed* relation. Has a blanket impl
/// (`store.rs`) over anything providing `GetById<C>` — no new index
/// needed, the foreign key already lives on the child.
pub trait Parent<C, Marker>
where
    C: ChildOf<Marker>,
{
    fn parent(&self, child_id: C::Id) -> Option<C::ParentId>;
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
