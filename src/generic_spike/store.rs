//! The composable capability-wrapper layers from
//! `docs/design/GENERIC-SCHEMA-DESIGN.md` §2/§4, implemented for real (not
//! stubs) — this is where the design doc's §4.5 forwarding-boilerplate tax
//! either shows up again under a real build, or doesn't. It does: see the
//! forwarding `impl` blocks below, each one required for `Symmetric<..>`
//! (the outermost layer in `DogGenericStore`) to still expose `GetById`/
//! `FilterEq`/`ScanField`/`UpdateField` from the layers underneath it.
//! Compare against the design doc's own count in its module docs' report.

use super::query::{Children, FilterEq, GetById, Neighbors, Parent, ScanField, UpdateField};
use super::traits::{ChildOf, IndexedField, Record, ScannableField, SymmetricRelation};
use super::NotFound;
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

// Forwarding impl #1: without this, `Indexed<S, ..>` doesn't expose
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

/// Adds one `ScanField`/`UpdateField` capability over an inner store — the
/// generic analogue of `CanonicalCachedStore`'s `age_cache` +
/// `position_index`. This is the layer this spike's numbers are actually
/// about.
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
    /// (non-generic) forwarding impls like `order_impl.rs`'s
    /// `ScanField<Order, Amount> for Scanned<S, Order, CreatedAt>` (see
    /// this file's module docs on why that can't be one generic impl
    /// here). `inner`/`inner_mut` rather than a public field: keeps the
    /// rest of `Scanned`'s representation (`position_index`, `cache`)
    /// private.
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

// Forwarding impl #2: `Scanned<S, ..>` re-exposing `GetById` from its
// inner store.
impl<S, R, Marker> GetById<R> for Scanned<S, R, Marker>
where
    R: ScannableField<Marker>,
    S: GetById<R>,
{
    fn get(&self, id: R::Id) -> Option<R> {
        self.inner.get(id)
    }
}

// Forwarding impl #3: `Scanned<S, ..>` re-exposing `FilterEq` from its
// inner store (e.g. `Scanned<Indexed<BaseStore<R>, R, Breed>, R, Age>`
// still needs to answer `filter_eq` on `Breed`) — note the two distinct
// marker type parameters (`IndexMarker` for the field `FilterEq` is being
// forwarded for, `Marker` for the field `Scanned` itself owns), exactly
// the shape the design doc's own module docs called out as the source of
// the tax.
impl<S, R, Marker, IndexMarker> FilterEq<R, IndexMarker> for Scanned<S, R, Marker>
where
    R: ScannableField<Marker> + IndexedField<IndexMarker>,
    S: FilterEq<R, IndexMarker>,
{
    // Bare `R::IndexValue` is unambiguous here even though `R` is bound
    // by both `ScannableField<Marker>` and `IndexedField<IndexMarker>`:
    // this is the fixed state, after the associated-type rename
    // (traits.rs) — before the rename, this was `R::Value`, ambiguous
    // between the two traits' identically-named assoc types, the exact
    // "ambiguous-associated-type" failure mode the design doc's own
    // scratch crate hit once already (§4.3) and this forwarding impl hit
    // again independently.
    fn filter_eq(&self, value: &R::IndexValue) -> Vec<R::Id> {
        self.inner.filter_eq(value)
    }
}

// Forwarding impl #3b, new for Order/Customer (`Dog` only ever had one
// `ScannableField`, so this case never came up before): `Scanned<S, ..>`
// needs to re-expose `ScanField`/`UpdateField` for a field *other* than
// its own once a record has more than one scannable field (`Order` has
// `Amount` and `CreatedAt`) and the layers stack (`Scanned<Scanned<..,
// Amount>, .., CreatedAt>`).
//
// **This is NOT expressible as one generic impl over "any other marker,"
// unlike every other forwarding impl in this file** — a first attempt,
// `impl<S, R, Marker, OtherMarker> ScanField<R, OtherMarker> for
// Scanned<S, R, Marker>`, doesn't even reach the associated-type
// ambiguity this file's other forwarding impls hit: it fails to compile
// at all, with `E0119: conflicting implementations of trait ScanField<_,
// _> for type Scanned<_, _, _>`. Rust's coherence checker has no way to
// know `OtherMarker != Marker`, so that impl and `Scanned`'s own direct
// `ScanField<R, Marker>` impl above are seen as *potentially* the same
// impl (the case `OtherMarker = Marker`) — a real orphan/overlap
// violation, not a naming or inference problem, and not fixable by
// disambiguating `R::Value` (fully-qualified syntax doesn't touch impl
// coherence at all). Stable Rust has no negative bound expressing "these
// two type parameters are unequal," so there is no way to write this as
// one generic impl.
//
// The only way to make this compile: one concrete, non-generic impl per
// *ordered pair* of markers. That's a real, unavoidable cost in what the
// compiler has to check — the tax for N scannable fields on one record
// isn't O(N) forwarding impls (the design doc's §4.5 accounting, which
// only ever validated a single-scannable-field domain, had no visibility
// into this), it's O(N²): one concrete impl per ordered pair, not one
// generic impl per field. What's NOT unavoidable is a human hand-writing
// and maintaining each pair — [`forward_scannable_pairs`] below generates
// them from a field list, so a new scannable field costs one macro-
// invocation entry, not new hand-written impls. See `order_impl.rs`'s
// invocation and its module docs for the before/after.
#[macro_export]
macro_rules! forward_scannable_pairs {
    // Entry point: a record type and its `ScannableField` markers, each
    // with its concrete `ScanValue` type (needed because macro_rules!
    // can't look up an associated type — see this macro's module docs).
    // `$record; Marker1: Value1, Marker2: Value2, ...`
    ($record:ty; $($marker:ident : $value:ty),+ $(,)?) => {
        $crate::forward_scannable_pairs!(@rotate $record; []; [$($marker : $value),+]);
    };

    // Peels `$owner` off the front of the not-yet-processed list, emits
    // its pairs against everything else (`$prefix` — already-processed
    // owners, still needed as forwarding targets — plus `$rest`, the
    // still-to-be-processed owners), then recurses with `$owner` moved
    // into `$prefix`. This is the standard "rotating accumulator" trick
    // for generating all off-diagonal pairs from a list in `macro_rules!`
    // — chosen specifically because `macro_rules!` cannot compare two
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

    // The actual payload: one concrete `ScanField`/`UpdateField` pair,
    // the same shape `order_impl.rs` originally hand-wrote once.
    (@impl_pair $record:ty; $owner:ident : $owner_value:ty; $forwarded:ident : $forwarded_value:ty) => {
        impl<S> $crate::generic_spike::query::ScanField<$record, $forwarded>
            for $crate::generic_spike::store::Scanned<S, $record, $owner>
        where
            S: $crate::generic_spike::query::ScanField<$record, $forwarded>,
        {
            fn scan(&self) -> Vec<$forwarded_value> {
                $crate::generic_spike::store::Scanned::inner(self).scan()
            }
        }

        impl<S> $crate::generic_spike::query::UpdateField<$record, $forwarded>
            for $crate::generic_spike::store::Scanned<S, $record, $owner>
        where
            S: $crate::generic_spike::query::UpdateField<$record, $forwarded>,
        {
            fn update(
                &mut self,
                id: <$record as $crate::generic_spike::traits::Record>::Id,
                value: $forwarded_value,
            ) -> Result<
                (),
                $crate::generic_spike::NotFound<<$record as $crate::generic_spike::traits::Record>::Id>,
            > {
                $crate::generic_spike::store::Scanned::inner_mut(self).update(id, value)
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

// Forwarding impl #4: `Symmetric<S, ..>` re-exposing `GetById`.
impl<S, R, Marker> GetById<R> for Symmetric<S, R, Marker>
where
    R: SymmetricRelation<Marker>,
    S: GetById<R>,
{
    fn get(&self, id: R::Id) -> Option<R> {
        self.inner.get(id)
    }
}

// Forwarding impl #5: `Symmetric<S, ..>` re-exposing `FilterEq`.
impl<S, R, Marker, IndexMarker> FilterEq<R, IndexMarker> for Symmetric<S, R, Marker>
where
    R: SymmetricRelation<Marker> + IndexedField<IndexMarker>,
    S: FilterEq<R, IndexMarker>,
{
    fn filter_eq(&self, value: &R::IndexValue) -> Vec<R::Id> {
        self.inner.filter_eq(value)
    }
}

// Forwarding impl #6: `Symmetric<S, ..>` re-exposing `ScanField` — this is
// the one this spike's own benchmark actually calls (`DogGenericStore` is
// a `Symmetric<..>` at the top).
impl<S, R, Marker, ScanMarker> ScanField<R, ScanMarker> for Symmetric<S, R, Marker>
where
    R: SymmetricRelation<Marker> + ScannableField<ScanMarker>,
    S: ScanField<R, ScanMarker>,
{
    fn scan(&self) -> Vec<R::ScanValue> {
        self.inner.scan()
    }
}

// Forwarding impl #7: `Symmetric<S, ..>` re-exposing `UpdateField`.
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
impl<S, C, Marker> Parent<C, Marker> for S
where
    C: ChildOf<Marker>,
    S: GetById<C>,
{
    fn parent(&self, child_id: C::Id) -> Option<C::ParentId> {
        self.get(child_id).map(|c| c.parent_id())
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
    pub fn new(inner: S, children: &[C]) -> Self {
        let mut children_of: HashMap<P::Id, Vec<C::Id>> = HashMap::new();
        for child in children {
            children_of
                .entry(child.parent_id())
                .or_default()
                .push(child.id());
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

// Forwarding impl #8: `Reversed<S, ..>` re-exposing `GetById` (for the
// child record type `C` — see the design doc §4.3's own account of the
// real mistake it caught here: an earlier draft tried to forward
// `GetById<P>`, the *parent* type, which a `C`-only inner store can never
// actually provide).
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

// Forwarding impl #9: `Reversed<S, ..>` re-exposing `FilterEq` on `C`.
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

// Forwarding impl #10: `Reversed<S, ..>` re-exposing `ScanField` on `C`.
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

// Forwarding impl #11: `Reversed<S, ..>` re-exposing `UpdateField` on `C`.
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
