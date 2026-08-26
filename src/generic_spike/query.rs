//! The query-capability traits from `docs/design/GENERIC-SCHEMA-DESIGN.md`
//! §2 that this spike actually exercises: `GetById` (generalizes `get`),
//! `FilterEq` (generalizes `same_breed`, included only because it's what
//! makes the forwarding-boilerplate tax real — see `store.rs`), `ScanField`
//! (generalizes `scan_ages`, this spike's real target), `UpdateField`
//! (needed so `Scanned` has a realistic shape, not measured directly), and
//! `Neighbors` (generalizes `neighbors`, included so `DogRecord`'s full
//! applicable trait set from the design doc's §3 table is actually
//! exercised, not just the two capabilities under measurement). `Parent`/
//! `Children` are not defined here — they're `ChildOf`'s query-side
//! counterparts, and `ChildOf` isn't implemented for `Dog` this round (see
//! `traits.rs`).

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

/// Generalizes `scan_ages` — this spike's real target.
pub trait ScanField<R, Marker>
where
    R: ScannableField<Marker>,
{
    fn scan(&self) -> Vec<R::ScanValue>;
}

/// Generalizes `update_age`. Not benchmarked this round (out of scope per
/// the task — only `get`/`scan_ages` are measured), but implemented for
/// real so `Scanned` is a genuine, not stubbed-out, capability layer.
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

/// The "one hop up" side of a *directed* relation — added for the
/// Order/Customer prototype (`order_impl.rs`), not exercised by the
/// `Dog`-only spike this crate already has. Per the design doc §2, this
/// has a blanket impl (`store.rs`) over anything providing `GetById<C>` —
/// no new index needed, the foreign key already lives on the child.
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
