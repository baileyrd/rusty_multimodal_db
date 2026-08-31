//! The composable capability-wrapper layers — promoted from
//! `docs/design/GENERIC-SCHEMA-DESIGN.md` §2/§4. Each layer adds exactly
//! one capability on top of an inner store that already provides
//! `GetById`, and — since Rust has no trait delegation — must manually
//! forward every other capability trait its inner store already provides,
//! or that capability silently disappears once wrapped. That forwarding
//! tax is real (§4.5 of the design doc) and is paid explicitly below, not
//! hidden.
//!
//! `Flush` (added during promotion, not part of the original design doc)
//! is the one new capability this round adds: a store composed with
//! [`crate::generic::mmap_store::GenericMmapStore`] somewhere inside it
//! needs a way to force its durable field to disk through however many
//! wrapper layers sit on top — [`GenericProductionStore`](super::production::GenericProductionStore)
//! is generic over the whole composed stack and has no other way to reach
//! in. Every wrapper below forwards it, same as every other capability.

use super::query::{Children, FilterEq, GetById, Neighbors, Parent, ScanField, UpdateField};
use super::traits::{ChildOf, IndexedField, Record, ScannableField, SymmetricRelation};
use super::NotFound;
use crate::durability::DurabilityError;
use std::collections::HashMap;
use std::marker::PhantomData;

/// Owns the records — the base of every composed stack. The generic
/// analogue of `CanonicalStore`'s `HashMap<Uuid, DogRecord>`.
pub struct BaseStore<R: Record> {
    records: HashMap<R::Id, R>,
}

impl<R: Record + Clone> BaseStore<R> {
    pub fn new(records: Vec<R>) -> Self {
        Self {
            records: records.into_iter().map(|r| (r.id(), r)).collect(),
        }
    }
}

impl<R: Record + Clone> GetById<R> for BaseStore<R> {
    fn get(&self, id: R::Id) -> Option<R> {
        self.records.get(&id).cloned()
    }
}

/// A store with no durable field forwards `flush` as a no-op — `BaseStore`
/// is always purely in-memory (durability, when present, is added by
/// [`crate::generic::mmap_store::GenericMmapStore`] sitting somewhere
/// inside the composed stack, not by `BaseStore` itself).
impl<R: Record + Clone> Flush for BaseStore<R> {
    fn flush(&self) -> Result<(), DurabilityError> {
        Ok(())
    }
}

/// Forces a store's durable field(s) to physical disk — see this module's
/// docs for why this exists. A no-op for any layer/stack with nothing
/// durable inside it.
pub trait Flush {
    fn flush(&self) -> Result<(), DurabilityError>;
}

/// Adds one `FilterEq` capability over an inner store — the generic
/// analogue of `CanonicalStore`'s `breed_index`.
pub struct Indexed<S, R, Marker>
where
    R: IndexedField<Marker>,
{
    inner: S,
    index: HashMap<R::IndexValue, Vec<R::Id>>,
    _marker: PhantomData<Marker>,
}

impl<S, R, Marker> Indexed<S, R, Marker>
where
    R: IndexedField<Marker>,
{
    pub fn new(inner: S, records: &[R]) -> Self {
        let mut index: HashMap<R::IndexValue, Vec<R::Id>> = HashMap::new();
        for record in records {
            index
                .entry(record.indexed_value().clone())
                .or_default()
                .push(record.id());
        }
        Self {
            inner,
            index,
            _marker: PhantomData,
        }
    }
}

impl<S, R, Marker> FilterEq<R, Marker> for Indexed<S, R, Marker>
where
    R: IndexedField<Marker>,
{
    fn filter_eq(&self, value: &R::IndexValue) -> Vec<R::Id> {
        self.index.get(value).cloned().unwrap_or_default()
    }
}

// Forwarding impl: without this, `Indexed<S, ..>` doesn't expose
// `GetById` even though its inner store already does.
impl<S, R, Marker> GetById<R> for Indexed<S, R, Marker>
where
    R: IndexedField<Marker>,
    S: GetById<R>,
{
    fn get(&self, id: R::Id) -> Option<R> {
        self.inner.get(id)
    }
}

impl<S, R, Marker> Flush for Indexed<S, R, Marker>
where
    R: IndexedField<Marker>,
    S: Flush,
{
    fn flush(&self) -> Result<(), DurabilityError> {
        self.inner.flush()
    }
}

/// Adds one `ScanField`/`UpdateField` capability over an inner store — the
/// generic analogue of `CanonicalCachedStore`'s `age_cache` +
/// `position_index`, entirely in-memory (see
/// [`crate::generic::mmap_store::GenericMmapStore`] for the durable
/// analogue of this same shape).
pub struct Scanned<S, R, Marker>
where
    R: ScannableField<Marker>,
{
    inner: S,
    position_index: HashMap<R::Id, usize>,
    cache: Vec<R::ScanValue>,
    _marker: PhantomData<Marker>,
}

impl<S, R, Marker> Scanned<S, R, Marker>
where
    R: ScannableField<Marker>,
{
    /// Access to the inner store — needed by domain-specific, concrete
    /// (non-generic) forwarding impls like `forward_scannable_pairs!`'s
    /// generated pairs (see this file's module docs on why that can't be
    /// one generic impl). `inner`/`inner_mut` rather than a public field:
    /// keeps the rest of `Scanned`'s representation (`position_index`,
    /// `cache`) private.
    pub fn inner(&self) -> &S {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut S {
        &mut self.inner
    }

    pub fn new(inner: S, records: &[R]) -> Self {
        let mut position_index = HashMap::with_capacity(records.len());
        let mut cache = Vec::with_capacity(records.len());
        for (position, record) in records.iter().enumerate() {
            position_index.insert(record.id(), position);
            cache.push(record.scannable_value());
        }
        Self {
            inner,
            position_index,
            cache,
            _marker: PhantomData,
        }
    }
}

impl<S, R, Marker> ScanField<R, Marker> for Scanned<S, R, Marker>
where
    R: ScannableField<Marker>,
{
    fn scan(&self) -> Vec<R::ScanValue> {
        self.cache.clone()
    }
}

impl<S, R, Marker> UpdateField<R, Marker> for Scanned<S, R, Marker>
where
    R: ScannableField<Marker>,
{
    fn update(&mut self, id: R::Id, value: R::ScanValue) -> Result<(), NotFound<R::Id>> {
        let position = *self.position_index.get(&id).ok_or(NotFound(id))?;
        self.cache[position] = value;
        Ok(())
    }
}

// Forwarding impl: `Scanned<S, ..>` re-exposing `GetById` from its inner
// store — write-through consistent with `UpdateField::update`, unlike an
// earlier version of this impl (see the fix note below).
impl<S, R, Marker> GetById<R> for Scanned<S, R, Marker>
where
    R: ScannableField<Marker>,
    S: GetById<R>,
{
    /// Patches the record `inner.get` returns with this layer's own live
    /// cached value before returning it — not a blind forward. An earlier
    /// version of this impl returned `self.inner.get(id)` unmodified,
    /// which meant a `Scanned` layer's own `UpdateField::update` (which
    /// only ever writes into `self.cache`, never down into `BaseStore`'s
    /// records map several layers below) was invisible to `get`. Reusing
    /// `set_scannable_value` (added for [`super::mmap_store::GenericMmapStore`]'s
    /// analogous gap) fixes it here too, but the mechanism is different: a
    /// single hand-fused struct like `GenericMmapStore` merges two views
    /// it owns directly, while `Scanned` — a separate struct layered on
    /// top of whatever owns the record — has no way to reach down into
    /// that owner's storage, so it patches on the way *up* through `get`
    /// instead. When multiple `Scanned` layers stack (e.g. `Order`'s
    /// `Amount`/`CreatedAt`/`DiscountCents`), each one patches only its
    /// own field as the call unwinds, so the record is fully consistent
    /// by the time it reaches the outermost caller — no change needed in
    /// `Indexed`/`Symmetric`/`Reversed`, none of which own any
    /// `ScannableField` data to patch.
    fn get(&self, id: R::Id) -> Option<R> {
        let mut record = self.inner.get(id)?;
        if let Some(&position) = self.position_index.get(&id) {
            record.set_scannable_value(self.cache[position]);
        }
        Some(record)
    }
}

impl<S, R, Marker> Flush for Scanned<S, R, Marker>
where
    R: ScannableField<Marker>,
    S: Flush,
{
    fn flush(&self) -> Result<(), DurabilityError> {
        self.inner.flush()
    }
}

// Forwarding impl: `Scanned<S, ..>` re-exposing `FilterEq` from its inner
// store (e.g. `Scanned<Indexed<BaseStore<R>, R, Breed>, R, Age>` still
// needs to answer `filter_eq` on `Breed`) — note the two distinct marker
// type parameters (`IndexMarker` for the field `FilterEq` is being
// forwarded for, `Marker` for the field `Scanned` itself owns).
impl<S, R, Marker, IndexMarker> FilterEq<R, IndexMarker> for Scanned<S, R, Marker>
where
    R: ScannableField<Marker> + IndexedField<IndexMarker>,
    S: FilterEq<R, IndexMarker>,
{
    // Bare `R::IndexValue` is unambiguous here even though `R` is bound
    // by both `ScannableField<Marker>` and `IndexedField<IndexMarker>` —
    // see `traits.rs`'s module docs for the associated-type rename this
    // relies on.
    fn filter_eq(&self, value: &R::IndexValue) -> Vec<R::Id> {
        self.inner.filter_eq(value)
    }
}

// Forwarding impl, generated for every ordered pair of `ScannableField`
// markers a record declares: `Scanned<S, ..>` needs to re-expose
// `ScanField`/`UpdateField` for a field *other* than its own once a
// record has more than one scannable field (`Order` has `Amount`,
// `CreatedAt`, `DiscountCents`) and the layers stack.
//
// **This is NOT expressible as one generic impl over "any other marker,"
// unlike every other forwarding impl in this file** — a first attempt,
// `impl<S, R, Marker, OtherMarker> ScanField<R, OtherMarker> for
// Scanned<S, R, Marker>`, doesn't even reach an associated-type ambiguity:
// it fails to compile at all, with `E0119: conflicting implementations of
// trait ScanField<_, _> for type Scanned<_, _, _>`. Rust's coherence
// checker has no way to know `OtherMarker != Marker`, so that impl and
// `Scanned`'s own direct `ScanField<R, Marker>` impl above are seen as
// *potentially* the same impl (the case `OtherMarker = Marker`) — a real
// orphan/overlap violation, not a naming or inference problem, and not
// fixable by disambiguating `R::ScanValue` (fully-qualified syntax
// doesn't touch impl coherence at all). Stable Rust has no negative bound
// expressing "these two type parameters are unequal," so there is no way
// to write this as one generic impl.
//
// The only way to make this compile: one concrete, non-generic impl per
// *ordered pair* of markers — the tax for N scannable fields on one
// record is O(N²), not O(N). What's not unavoidable is a human hand-
// writing and maintaining each pair — the macro below generates them from
// a field list, so a new scannable field costs one macro-invocation
// entry, not new hand-written impls. See `order_customer.rs`'s
// invocation.
#[macro_export]
macro_rules! forward_scannable_pairs {
    // Entry point: a record type and its `ScannableField` markers, each
    // with its concrete `ScanValue` type (needed because macro_rules!
    // can't look up an associated type).
    // `$record; Marker1: Value1, Marker2: Value2, ...`
    ($record:ty; $($marker:ident : $value:ty),+ $(,)?) => {
        $crate::forward_scannable_pairs!(@rotate $record; []; [$($marker : $value),+]);
    };

    // Peels `$owner` off the front of the not-yet-processed list, emits
    // its pairs against everything else (`$prefix` — already-processed
    // owners, still needed as forwarding targets — plus `$rest`, the
    // still-to-be-processed owners), then recurses with `$owner` moved
    // into `$prefix`. The standard "rotating accumulator" trick for
    // generating all off-diagonal pairs from a list in `macro_rules!` —
    // chosen specifically because `macro_rules!` cannot compare two
    // matched fragments for equality (there is no `$a == $b` for
    // `:ident`/`:ty` matchers), so the diagonal (`owner == owner`) has to
    // be excluded *structurally*, by construction, rather than by a
    // runtime-style check. `$owner` never appears in the "everything
    // else" list at the point it's used, by construction — it's been
    // removed from `$rest` and not yet added to `$prefix`.
    (@rotate $record:ty; [$($prefix:ident : $prefix_value:ty),*]; [$owner:ident : $owner_value:ty $(, $rest:ident : $rest_value:ty)*]) => {
        $crate::forward_scannable_pairs!(
            @pairs $record; $owner : $owner_value;
            [$($prefix : $prefix_value,)* $($rest : $rest_value),*]
        );
        $crate::forward_scannable_pairs!(
            @rotate $record; [$($prefix : $prefix_value,)* $owner : $owner_value]; [$($rest : $rest_value),*]
        );
    };
    // Base case: nothing left to peel off — every owner has had its
    // pairs generated.
    (@rotate $record:ty; [$($prefix:ident : $prefix_value:ty),*]; []) => {};

    // Emits one `@impl_pair` per marker in the "everything else" list,
    // for the fixed `$owner`.
    (@pairs $record:ty; $owner:ident : $owner_value:ty; [$($forwarded:ident : $forwarded_value:ty),* $(,)?]) => {
        $(
            $crate::forward_scannable_pairs!(@impl_pair $record; $owner : $owner_value; $forwarded : $forwarded_value);
        )*
    };

    // The actual payload: one concrete `ScanField`/`UpdateField` pair.
    (@impl_pair $record:ty; $owner:ident : $owner_value:ty; $forwarded:ident : $forwarded_value:ty) => {
        impl<S> $crate::generic::query::ScanField<$record, $forwarded>
            for $crate::generic::store::Scanned<S, $record, $owner>
        where
            S: $crate::generic::query::ScanField<$record, $forwarded>,
        {
            fn scan(&self) -> Vec<$forwarded_value> {
                $crate::generic::store::Scanned::inner(self).scan()
            }
        }

        impl<S> $crate::generic::query::UpdateField<$record, $forwarded>
            for $crate::generic::store::Scanned<S, $record, $owner>
        where
            S: $crate::generic::query::UpdateField<$record, $forwarded>,
        {
            fn update(
                &mut self,
                id: <$record as $crate::generic::traits::Record>::Id,
                value: $forwarded_value,
            ) -> Result<
                (),
                $crate::generic::NotFound<<$record as $crate::generic::traits::Record>::Id>,
            > {
                $crate::generic::store::Scanned::inner_mut(self).update(id, value)
            }
        }
    };
}

/// Adds one `Neighbors` capability over an inner store — the generic
/// analogue of `CanonicalCachedStore`'s `adjacency_index`.
pub struct Symmetric<S, R, Marker>
where
    R: SymmetricRelation<Marker>,
{
    inner: S,
    adjacency: HashMap<R::Id, Vec<R::Id>>,
    _marker: PhantomData<Marker>,
}

impl<S, R, Marker> Symmetric<S, R, Marker>
where
    R: SymmetricRelation<Marker>,
{
    pub fn new(inner: S, edges: &[(R::Id, R::Id)]) -> Self {
        let mut adjacency: HashMap<R::Id, Vec<R::Id>> = HashMap::new();
        for &(a, b) in edges {
            adjacency.entry(a).or_default().push(b);
            adjacency.entry(b).or_default().push(a);
        }
        Self {
            inner,
            adjacency,
            _marker: PhantomData,
        }
    }
}

impl<S, R, Marker> Neighbors<R, Marker> for Symmetric<S, R, Marker>
where
    R: SymmetricRelation<Marker>,
{
    fn neighbors(&self, id: R::Id) -> Vec<R::Id> {
        self.adjacency.get(&id).cloned().unwrap_or_default()
    }
}

// Forwarding impl: `Symmetric<S, ..>` re-exposing `GetById`.
impl<S, R, Marker> GetById<R> for Symmetric<S, R, Marker>
where
    R: SymmetricRelation<Marker>,
    S: GetById<R>,
{
    fn get(&self, id: R::Id) -> Option<R> {
        self.inner.get(id)
    }
}

impl<S, R, Marker> Flush for Symmetric<S, R, Marker>
where
    R: SymmetricRelation<Marker>,
    S: Flush,
{
    fn flush(&self) -> Result<(), DurabilityError> {
        self.inner.flush()
    }
}

// Forwarding impl: `Symmetric<S, ..>` re-exposing `FilterEq`.
impl<S, R, Marker, IndexMarker> FilterEq<R, IndexMarker> for Symmetric<S, R, Marker>
where
    R: SymmetricRelation<Marker> + IndexedField<IndexMarker>,
    S: FilterEq<R, IndexMarker>,
{
    fn filter_eq(&self, value: &R::IndexValue) -> Vec<R::Id> {
        self.inner.filter_eq(value)
    }
}

// Forwarding impl: `Symmetric<S, ..>` re-exposing `ScanField`.
impl<S, R, Marker, ScanMarker> ScanField<R, ScanMarker> for Symmetric<S, R, Marker>
where
    R: SymmetricRelation<Marker> + ScannableField<ScanMarker>,
    S: ScanField<R, ScanMarker>,
{
    fn scan(&self) -> Vec<R::ScanValue> {
        self.inner.scan()
    }
}

// Forwarding impl: `Symmetric<S, ..>` re-exposing `UpdateField`.
impl<S, R, Marker, ScanMarker> UpdateField<R, ScanMarker> for Symmetric<S, R, Marker>
where
    R: SymmetricRelation<Marker> + ScannableField<ScanMarker>,
    S: UpdateField<R, ScanMarker>,
{
    fn update(&mut self, id: R::Id, value: R::ScanValue) -> Result<(), NotFound<R::Id>> {
        self.inner.update(id, value)
    }
}

/// `Parent` (the cheap direction of a directed relation) needs no new
/// store state at all — a blanket impl over anything that already
/// provides `GetById<C>`, per the design doc §2.
///
/// `self.get(child_id)` supplies `GetById`'s own "not found" shape
/// directly, turned into this trait's `Err(NotFound(child_id))` via
/// `.ok_or(...)?`; `.parent_id()` (itself `Option<C::ParentId>` — see
/// `ChildOf`'s own doc comment) becomes the `Ok(...)` payload unchanged.
/// "Child not found" and "child found, has no parent" — which a bare
/// single-level `Option` return once collapsed to the same `None` (a
/// gap `Rule`'s `chain_to_root` had to work around directly) — are
/// distinct outcomes again: `Err` vs. `Ok(None)`.
impl<S, C, Marker> Parent<C, Marker> for S
where
    C: ChildOf<Marker>,
    S: GetById<C>,
{
    fn parent(&self, child_id: C::Id) -> Result<Option<C::ParentId>, NotFound<C::Id>> {
        let child = self.get(child_id).ok_or(NotFound(child_id))?;
        Ok(child.parent_id())
    }
}

/// Adds one `Children` capability over an inner store — the generic
/// analogue of a `HashMap<CustomerId, Vec<OrderId>>` reverse index, the
/// expensive direction of a directed relation (design doc §4.3).
pub struct Reversed<S, P, C, Marker>
where
    P: Record,
    C: ChildOf<Marker, ParentId = P::Id>,
{
    inner: S,
    children_of: HashMap<P::Id, Vec<C::Id>>,
    _marker: PhantomData<(P, Marker)>,
}

impl<S, P, C, Marker> Reversed<S, P, C, Marker>
where
    P: Record,
    C: ChildOf<Marker, ParentId = P::Id>,
{
    /// Entries with no parent (`child.parent_id()` returns `None`) are
    /// skipped naturally — not an error, not a special case, just nothing
    /// to index for a record that isn't anyone's child. Before the
    /// optional-parent fix, `ChildOf::parent_id` returned a bare
    /// `Self::ParentId`, so every record necessarily had exactly one
    /// parent entry to insert; this `if let` is the one behavioral change
    /// that fix required here, and it's a no-op for a domain like `Order`
    /// whose `parent_id` never returns `None`.
    pub fn new(inner: S, children: &[C]) -> Self {
        let mut children_of: HashMap<P::Id, Vec<C::Id>> = HashMap::new();
        for child in children {
            if let Some(parent_id) = child.parent_id() {
                children_of.entry(parent_id).or_default().push(child.id());
            }
        }
        Self {
            inner,
            children_of,
            _marker: PhantomData,
        }
    }
}

impl<S, P, C, Marker> Children<P, C, Marker> for Reversed<S, P, C, Marker>
where
    P: Record,
    C: ChildOf<Marker, ParentId = P::Id>,
{
    fn children(&self, parent_id: P::Id) -> Vec<C::Id> {
        self.children_of
            .get(&parent_id)
            .cloned()
            .unwrap_or_default()
    }
}

// Forwarding impl: `Reversed<S, ..>` re-exposing `GetById` (for the child
// record type `C` — a store built entirely from `&[C]` can never actually
// provide `GetById<P>`, the parent type; see the design doc §4.3's own
// account of the real mistake caught here during the original design
// pass).
impl<S, P, C, Marker> GetById<C> for Reversed<S, P, C, Marker>
where
    P: Record,
    C: ChildOf<Marker, ParentId = P::Id>,
    S: GetById<C>,
{
    fn get(&self, id: C::Id) -> Option<C> {
        self.inner.get(id)
    }
}

impl<S, P, C, Marker> Flush for Reversed<S, P, C, Marker>
where
    P: Record,
    C: ChildOf<Marker, ParentId = P::Id>,
    S: Flush,
{
    fn flush(&self) -> Result<(), DurabilityError> {
        self.inner.flush()
    }
}

// Forwarding impl: `Reversed<S, ..>` re-exposing `FilterEq` on `C`.
impl<S, P, C, Marker, IndexMarker> FilterEq<C, IndexMarker> for Reversed<S, P, C, Marker>
where
    P: Record,
    C: ChildOf<Marker, ParentId = P::Id> + IndexedField<IndexMarker>,
    S: FilterEq<C, IndexMarker>,
{
    fn filter_eq(&self, value: &C::IndexValue) -> Vec<C::Id> {
        self.inner.filter_eq(value)
    }
}

// Forwarding impl: `Reversed<S, ..>` re-exposing `ScanField` on `C`.
impl<S, P, C, Marker, ScanMarker> ScanField<C, ScanMarker> for Reversed<S, P, C, Marker>
where
    P: Record,
    C: ChildOf<Marker, ParentId = P::Id> + ScannableField<ScanMarker>,
    S: ScanField<C, ScanMarker>,
{
    fn scan(&self) -> Vec<C::ScanValue> {
        self.inner.scan()
    }
}

// Forwarding impl: `Reversed<S, ..>` re-exposing `UpdateField` on `C`.
impl<S, P, C, Marker, ScanMarker> UpdateField<C, ScanMarker> for Reversed<S, P, C, Marker>
where
    P: Record,
    C: ChildOf<Marker, ParentId = P::Id> + ScannableField<ScanMarker>,
    S: UpdateField<C, ScanMarker>,
{
    fn update(&mut self, id: C::Id, value: C::ScanValue) -> Result<(), NotFound<C::Id>> {
        self.inner.update(id, value)
    }
}

// Forwarding impl: `Reversed<S, ..>` re-exposing `Neighbors` — added for
// the first domain needing `SymmetricRelation` and `ChildOf` together on
// one record type (an `Employee`-style domain: `reports_to`, a directed
// self-relation, plus `collaborates_with`, a symmetric one) —
// `SERVER-QUERY-LAYER`'s third-domain validation round. `R`/`RelMarker`
// are independent of this impl's own `P`/`C`/`Marker`, not tied to the
// `ChildOf` relation `Reversed` itself indexes — in the self-referential
// case that motivated this (`R = P = C`), the same record type
// participates in both relations, but nothing here requires that.
impl<S, P, C, Marker, R, RelMarker> Neighbors<R, RelMarker> for Reversed<S, P, C, Marker>
where
    P: Record,
    C: ChildOf<Marker, ParentId = P::Id>,
    R: SymmetricRelation<RelMarker>,
    S: Neighbors<R, RelMarker>,
{
    fn neighbors(&self, id: R::Id) -> Vec<R::Id> {
        self.inner.neighbors(id)
    }
}
