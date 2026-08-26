//! The composable capability-wrapper layers from
//! `docs/design/GENERIC-SCHEMA-DESIGN.md` §2/§4, implemented for real (not
//! stubs) — this is where the design doc's §4.5 forwarding-boilerplate tax
//! either shows up again under a real build, or doesn't. It does: see the
//! forwarding `impl` blocks below, each one required for `Symmetric<..>`
//! (the outermost layer in `DogGenericStore`) to still expose `GetById`/
//! `FilterEq`/`ScanField`/`UpdateField` from the layers underneath it.
//! Compare against the design doc's own count in its module docs' report.

use super::query::{FilterEq, GetById, Neighbors, ScanField, UpdateField};
use super::traits::{IndexedField, Record, ScannableField, SymmetricRelation};
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
    index: HashMap<R::Value, Vec<R::Id>>,
    _marker: PhantomData<Marker>,
}

impl<S, R, Marker> Indexed<S, R, Marker>
where
    R: IndexedField<Marker>,
{
    pub fn new(inner: S, records: &[R]) -> Self {
        let mut index: HashMap<R::Value, Vec<R::Id>> = HashMap::new();
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
    fn filter_eq(&self, value: &R::Value) -> Vec<R::Id> {
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
    cache: Vec<R::Value>,
    _marker: PhantomData<Marker>,
}

impl<S, R, Marker> Scanned<S, R, Marker>
where
    R: ScannableField<Marker>,
{
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
    fn scan(&self) -> Vec<R::Value> {
        self.cache.clone()
    }
}

impl<S, R, Marker> UpdateField<R, Marker> for Scanned<S, R, Marker>
where
    R: ScannableField<Marker>,
{
    fn update(&mut self, id: R::Id, value: R::Value) -> Result<(), NotFound<R::Id>> {
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
    // `R::Value` alone is ambiguous here: both `ScannableField<Marker>`
    // and `IndexedField<IndexMarker>` are in scope on `R`, each with their
    // own associated `Value` — the exact "ambiguous-associated-type"
    // failure mode the design doc's own scratch crate hit once already
    // (§4.3), reproduced here by this forwarding impl instead.
    fn filter_eq(&self, value: &<R as IndexedField<IndexMarker>>::Value) -> Vec<R::Id> {
        self.inner.filter_eq(value)
    }
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
    fn filter_eq(&self, value: &R::Value) -> Vec<R::Id> {
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
    fn scan(&self) -> Vec<R::Value> {
        self.inner.scan()
    }
}

// Forwarding impl #7: `Symmetric<S, ..>` re-exposing `UpdateField`.
impl<S, R, Marker, ScanMarker> UpdateField<R, ScanMarker> for Symmetric<S, R, Marker>
where
    R: SymmetricRelation<Marker> + ScannableField<ScanMarker>,
    S: UpdateField<R, ScanMarker>,
{
    fn update(&mut self, id: R::Id, value: R::Value) -> Result<(), NotFound<R::Id>> {
        self.inner.update(id, value)
    }
}
