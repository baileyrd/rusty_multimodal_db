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

use super::edge_blob::{self, EdgeBlob};
use super::query::{
    AllIds, Children, FilterEq, GetById, Neighbors, Parent, ScanField, UpdateField,
};
use super::traits::{ChildOf, IndexedField, Record, ScannableField, SchemaTag, SymmetricRelation};
use super::NotFound;
use crate::durability::DurabilityError;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::path::Path;

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
    // Entry points, one per layer kind. Each layer that owns one
    // scannable field and forwards the others needs its own set of pairs;
    // the layer is spelled by name (not as a path) because a `$layer:path`
    // fragment cannot be followed by `<`, and the two layers this crate
    // has are enumerated so the invocation site doesn't need to know
    // where they live. Adding a third layer means adding one arm here.
    // `for Layer; $record; Marker1: Value1, Marker2: Value2, ...`
    //
    // These arms come before the bare `$record:ty` one on purpose: `for`
    // also opens a `for<'a> ...` type, so a `ty` fragment matcher tried
    // first would fail *hard* on `for Scanned` instead of falling through.
    (for Scanned; $record:ty; $($marker:ident : $value:ty),+ $(,)?) => {
        $crate::forward_scannable_pairs!(
            @rotate [$crate::generic::store::Scanned] $record; []; [$($marker : $value),+]
        );
    };
    (for MmapScanned; $record:ty; $($marker:ident : $value:ty),+ $(,)?) => {
        $crate::forward_scannable_pairs!(
            @rotate [$crate::generic::mmap_scanned::MmapScanned] $record; []; [$($marker : $value),+]
        );
    };

    // The original entry point: a record type and its `ScannableField`
    // markers, each with its concrete `ScanValue` type (needed because
    // macro_rules! can't look up an associated type). Generates the pairs
    // for the in-memory `Scanned` layer.
    // `$record; Marker1: Value1, Marker2: Value2, ...`
    ($record:ty; $($marker:ident : $value:ty),+ $(,)?) => {
        $crate::forward_scannable_pairs!(for Scanned; $record; $($marker : $value),+);
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
    // The layer's path travels as one bracketed token tree (`$layer:tt`,
    // e.g. `[$crate::generic::store::Scanned]`) through the internal
    // arms — a single `tt` so it can sit inside the `@pairs` repetition
    // without a depth mismatch — and is only opened up by the payload arm,
    // which splices it in front of `<S, $record, $owner>`.
    (@rotate $layer:tt $record:ty; [$($prefix:ident : $prefix_value:ty),*]; [$owner:ident : $owner_value:ty $(, $rest:ident : $rest_value:ty)*]) => {
        $crate::forward_scannable_pairs!(
            @pairs $layer $record; $owner : $owner_value;
            [$($prefix : $prefix_value,)* $($rest : $rest_value),*]
        );
        $crate::forward_scannable_pairs!(
            @rotate $layer $record; [$($prefix : $prefix_value,)* $owner : $owner_value]; [$($rest : $rest_value),*]
        );
    };
    // Base case: nothing left to peel off — every owner has had its
    // pairs generated.
    (@rotate $layer:tt $record:ty; [$($prefix:ident : $prefix_value:ty),*]; []) => {};

    // Emits one `@impl_pair` per marker in the "everything else" list,
    // for the fixed `$owner`.
    (@pairs $layer:tt $record:ty; $owner:ident : $owner_value:ty; [$($forwarded:ident : $forwarded_value:ty),* $(,)?]) => {
        $(
            $crate::forward_scannable_pairs!(@impl_pair $layer $record; $owner : $owner_value; $forwarded : $forwarded_value);
        )*
    };

    // The actual payload: one concrete `ScanField`/`UpdateField` pair.
    // Both layers expose the same `inner`/`inner_mut` accessors, which is
    // all the generated body relies on.
    (@impl_pair [$($layer:tt)*] $record:ty; $owner:ident : $owner_value:ty; $forwarded:ident : $forwarded_value:ty) => {
        impl<S> $crate::generic::query::ScanField<$record, $forwarded>
            for $($layer)*<S, $record, $owner>
        where
            S: $crate::generic::query::ScanField<$record, $forwarded>,
        {
            fn scan(&self) -> Vec<$forwarded_value> {
                $($layer)*::inner(self).scan()
            }
        }

        impl<S> $crate::generic::query::UpdateField<$record, $forwarded>
            for $($layer)*<S, $record, $owner>
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
                $($layer)*::inner_mut(self).update(id, value)
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

/// The edge list [`Symmetric::read_portable_edges`] returns: the pairs
/// [`Symmetric::create`] was given, in the order it was given them.
type PortableEdges<R> = Vec<(<R as Record>::Id, <R as Record>::Id)>;

/// File portability for the edge list — `STORAGE-016`, per
/// `docs/design/SYMMETRIC-EDGE-PORTABILITY-DESIGN.md` (Accepted) and
/// ADR-0018. [`Symmetric::new`] builds its adjacency from a caller-supplied
/// slice and persists nothing, which is right for the in-memory stacks
/// but leaves a durable stack's symmetric edges living only in the
/// caller's hands: `GenericMmapStore` carries its records in
/// `<path>.records`, so a directory holding that pair could rebuild the
/// core store but not the `Symmetric` layer above it. This block adds the
/// `create`/`open`/`open_portable` triple that closes the gap — the edge
/// list, as given and in caller order, in a companion "edge blob" at a
/// caller-supplied `edges_path` (see `crate::generic::edge_blob`, and
/// `edges_path` there for the `<path>.edges` single-relation convention
/// the domain helpers use).
///
/// Bounded `R::Id: Serialize + DeserializeOwned` and `R: SchemaTag` here
/// and here only (`SYMPORT-FR-006`, `SCHTAG-FR-002`): `new`, the
/// `Neighbors` impl, and every forwarding impl keep their bounds exactly,
/// and no existing call site changes. The blob's header carries
/// `R::SCHEMA_TAG`'s hash, so an edge blob written for one record type is
/// refused, by name, when opened as a relation over another with the
/// same `Id` type (`SCHTAG-FR-001`). A durable stack assembled with `new`
/// rather than `create`/`open` writes no blob and is not portable —
/// closed by convention in the domain helpers, not by type (the design
/// doc's stated non-goal).
impl<S, R, Marker> Symmetric<S, R, Marker>
where
    R: SymmetricRelation<Marker> + SchemaTag,
    R::Id: Serialize + DeserializeOwned,
{
    /// Write `edges` to `edges_path`, then [`Self::new`] (`SYMPORT-FR-001`).
    /// Always writes: this is the constructor for a fresh stack, the
    /// analogue of `GenericMmapStore::create`. `inner` is received already
    /// built; if the blob write fails it is dropped with the error, and
    /// its own files (if any) remain valid for a retried `create` or an
    /// [`Self::open`].
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Serde`] if `edges` can't be serialized,
    /// [`DurabilityError::Io`] if the blob can't be written.
    pub fn create(
        inner: S,
        edges: &[(R::Id, R::Id)],
        edges_path: &Path,
    ) -> Result<Self, DurabilityError> {
        EdgeBlob::new(edges, R::SCHEMA_TAG)
            .encode()?
            .write(edges_path)?;
        Ok(Self::new(inner, edges))
    }

    /// [`Self::new`] over the caller's `edges`, keeping the blob at
    /// `edges_path` current with them (`SYMPORT-FR-004`): the header's
    /// fingerprint is compared against `edges` first, and only if they
    /// differ — a changed or reordered edge list, a missing, short,
    /// foreign, wrong-version, or wrong-tag file, a directory written
    /// before the edge blob existed or before it carried a tag
    /// (`SCHTAG-FR-004`) — is the blob re-encoded and rewritten. The
    /// common reopen with the same edges never writes. The adjacency is
    /// always built from `edges`, never from the blob, on this path.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Serde`] if a stale blob's edges can't
    /// be serialized, [`DurabilityError::Io`] if it can't be rewritten.
    pub fn open(
        inner: S,
        edges: &[(R::Id, R::Id)],
        edges_path: &Path,
    ) -> Result<Self, DurabilityError> {
        let blob = EdgeBlob::new(edges, R::SCHEMA_TAG);
        if !blob.is_current_at(edges_path) {
            blob.encode()?.write(edges_path)?;
        }
        Ok(Self::new(inner, edges))
    }

    /// The edge list persisted at `edges_path`, in the order it was
    /// written (`SYMPORT-FR-002`). Reads only the blob; never writes,
    /// never touches the inner store.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::RecordBlobUnreadable`], naming
    /// `edges_path`, if the blob is missing, isn't one (wrong magic — a
    /// `GENBLOB\0` or `DOGBLOB\0` file included), was written by an
    /// incompatible version, carries another record type's schema tag
    /// (`SCHTAG-FR-003`), doesn't decode, or doesn't match its own header
    /// fingerprint (`SYMPORT-FR-005`).
    pub fn read_portable_edges(edges_path: &Path) -> Result<PortableEdges<R>, DurabilityError> {
        edge_blob::read(edges_path, R::SCHEMA_TAG)
    }

    /// Rebuild the layer from its blob alone — exactly
    /// `Ok(Self::new(inner, &Self::read_portable_edges(edges_path)?))`
    /// (`SYMPORT-FR-003`). Because the blob preserves edge order and
    /// [`Self::new`] pushes adjacency entries in edge order, `neighbors`
    /// returns the same sequences the original layer did
    /// (`SYMPORT-FR-008`). Never writes.
    ///
    /// # Errors
    ///
    /// Everything [`Self::read_portable_edges`] can return.
    pub fn open_portable(inner: S, edges_path: &Path) -> Result<Self, DurabilityError> {
        Ok(Self::new(inner, &Self::read_portable_edges(edges_path)?))
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

// Forwarding impl: `Symmetric<S, ..>` re-exposing `AllIds` (`SQL-FR-005`,
// ADR-0034) — the identical shape every other capability is forwarded
// through this layer in.
impl<S, R, Marker> AllIds<R> for Symmetric<S, R, Marker>
where
    R: SymmetricRelation<Marker>,
    S: AllIds<R>,
{
    fn all_ids(&self) -> Vec<R::Id> {
        self.inner.all_ids()
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

// Forwarding impl: `Reversed<S, ..>` re-exposing `AllIds` on the child
// record type `C` (`SQL-FR-005`, ADR-0034) — the same "for `C`, not `P`"
// shape `GetById`/`FilterEq`/`ScanField`/`UpdateField` above already take.
impl<S, P, C, Marker> AllIds<C> for Reversed<S, P, C, Marker>
where
    P: Record,
    C: ChildOf<Marker, ParentId = P::Id>,
    S: AllIds<C>,
{
    fn all_ids(&self) -> Vec<C::Id> {
        self.inner.all_ids()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::fresh_temp_dir;
    use std::path::PathBuf;

    // The smallest record type that can sit under a `Symmetric` layer —
    // the blob is independent of what `S` is, so nothing here needs a
    // `.mmap` file or a real domain.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Node {
        id: u32,
    }

    impl Record for Node {
        type Id = u32;
        fn id(&self) -> u32 {
            self.id
        }
    }

    impl SchemaTag for Node {
        const SCHEMA_TAG: &'static str = "store::tests::Node";
    }

    struct Linked;
    impl SymmetricRelation<Linked> for Node {}

    type Layer = Symmetric<BaseStore<Node>, Node, Linked>;

    // A second record type over the same `u32` id and the same relation
    // marker: its edge blob is byte-for-byte a `Node` edge blob except for
    // the tag, which is the only thing that keeps it out of `Layer`.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Other {
        id: u32,
    }

    impl Record for Other {
        type Id = u32;
        fn id(&self) -> u32 {
            self.id
        }
    }

    impl SchemaTag for Other {
        const SCHEMA_TAG: &'static str = "store::tests::Other";
    }

    impl SymmetricRelation<Linked> for Other {}

    type OtherLayer = Symmetric<BaseStore<Other>, Other, Linked>;

    fn nodes() -> Vec<Node> {
        (1..=4).map(|id| Node { id }).collect()
    }

    fn edges() -> Vec<(u32, u32)> {
        vec![(1, 2), (2, 3), (1, 3)]
    }

    fn scratch(label: &str) -> (PathBuf, PathBuf) {
        let dir = fresh_temp_dir(label).unwrap();
        let edges_path = edge_blob::edges_path(&dir.join("store.mmap"));
        (dir, edges_path)
    }

    fn all_neighbors(layer: &Layer) -> Vec<Vec<u32>> {
        (1..=4)
            .map(|id| Neighbors::<Node, Linked>::neighbors(layer, id))
            .collect()
    }

    #[test]
    fn create_then_open_portable_rebuilds_the_same_adjacency_in_the_same_order() {
        let (dir, edges_path) = scratch("symmetric_create_open_portable");
        let original = Layer::create(BaseStore::new(nodes()), &edges(), &edges_path).unwrap();
        assert!(edges_path.is_file());

        let portable = Layer::open_portable(BaseStore::new(nodes()), &edges_path).unwrap();
        assert_eq!(all_neighbors(&portable), all_neighbors(&original));
        assert_eq!(
            Neighbors::<Node, Linked>::neighbors(&portable, 1),
            vec![2, 3],
            "edge order (1,2) before (1,3) must survive the round trip"
        );
        assert_eq!(
            Neighbors::<Node, Linked>::neighbors(&portable, 4),
            Vec::<u32>::new()
        );
        assert_eq!(Layer::read_portable_edges(&edges_path).unwrap(), edges());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_with_the_same_edges_does_not_rewrite_the_blob() {
        let (dir, edges_path) = scratch("symmetric_open_no_rewrite");
        let _ = Layer::create(BaseStore::new(nodes()), &edges(), &edges_path).unwrap();
        let before_bytes = std::fs::read(&edges_path).unwrap();
        let before_mtime = std::fs::metadata(&edges_path).unwrap().modified().unwrap();

        let reopened = Layer::open(BaseStore::new(nodes()), &edges(), &edges_path).unwrap();
        assert_eq!(
            Neighbors::<Node, Linked>::neighbors(&reopened, 2),
            vec![1, 3]
        );
        assert_eq!(std::fs::read(&edges_path).unwrap(), before_bytes);
        assert_eq!(
            std::fs::metadata(&edges_path).unwrap().modified().unwrap(),
            before_mtime
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_with_changed_edges_rewrites_the_blob() {
        let (dir, edges_path) = scratch("symmetric_open_rewrite");
        let _ = Layer::create(BaseStore::new(nodes()), &edges(), &edges_path).unwrap();
        let changed = vec![(1, 2), (3, 4)];

        let reopened = Layer::open(BaseStore::new(nodes()), &changed, &edges_path).unwrap();
        assert_eq!(Neighbors::<Node, Linked>::neighbors(&reopened, 3), vec![4]);
        assert_eq!(Layer::read_portable_edges(&edges_path).unwrap(), changed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_with_reordered_edges_counts_as_changed() {
        let (dir, edges_path) = scratch("symmetric_open_reorder");
        let _ = Layer::create(BaseStore::new(nodes()), &edges(), &edges_path).unwrap();
        let mut reordered = edges();
        reordered.swap(0, 2);

        let reopened = Layer::open(BaseStore::new(nodes()), &reordered, &edges_path).unwrap();
        assert_eq!(
            Neighbors::<Node, Linked>::neighbors(&reopened, 1),
            vec![3, 2],
            "reordered input must be observable through neighbors"
        );
        assert_eq!(Layer::read_portable_edges(&edges_path).unwrap(), reordered);
        let portable = Layer::open_portable(BaseStore::new(nodes()), &edges_path).unwrap();
        assert_eq!(
            Neighbors::<Node, Linked>::neighbors(&portable, 1),
            vec![3, 2]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_blob_is_a_typed_error_from_open_portable_and_healed_by_open() {
        let (dir, edges_path) = scratch("symmetric_missing_blob");
        match Layer::open_portable(BaseStore::new(nodes()), &edges_path) {
            Err(DurabilityError::RecordBlobUnreadable { path, cause }) => {
                assert_eq!(path, edges_path);
                assert!(cause.starts_with("cannot read file"), "{cause}");
            }
            Err(other) => panic!("expected RecordBlobUnreadable, got {other:?}"),
            Ok(_) => panic!("expected RecordBlobUnreadable, got a layer"),
        }
        assert!(matches!(
            Layer::read_portable_edges(&edges_path),
            Err(DurabilityError::RecordBlobUnreadable { .. })
        ));

        // The pre-feature directory case: `open` with the caller's edges
        // writes the blob it finds missing, after which the portable
        // path works.
        let healed = Layer::open(BaseStore::new(nodes()), &edges(), &edges_path).unwrap();
        assert_eq!(Neighbors::<Node, Linked>::neighbors(&healed, 1), vec![2, 3]);
        assert!(edges_path.is_file());
        let portable = Layer::open_portable(BaseStore::new(nodes()), &edges_path).unwrap();
        assert_eq!(all_neighbors(&portable), all_neighbors(&healed));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn create_over_an_existing_blob_always_rewrites_it() {
        let (dir, edges_path) = scratch("symmetric_create_overwrites");
        let _ = Layer::create(BaseStore::new(nodes()), &edges(), &edges_path).unwrap();
        let fresh = vec![(4, 1)];
        let _ = Layer::create(BaseStore::new(nodes()), &fresh, &edges_path).unwrap();
        assert_eq!(Layer::read_portable_edges(&edges_path).unwrap(), fresh);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn another_record_types_edge_blob_is_a_tag_error_from_the_read_only_paths_and_healed_by_open() {
        let (dir, edges_path) = scratch("symmetric_other_tag");
        let others: Vec<Other> = (1..=4).map(|id| Other { id }).collect();
        let _ = OtherLayer::create(BaseStore::new(others), &edges(), &edges_path).unwrap();

        // Acceptance criterion 4: same `Id`, same edges, same bytes in
        // the body — refused by name, never decoded (`SCHTAG-FR-001`).
        for result in [
            Layer::open_portable(BaseStore::new(nodes()), &edges_path).map(|_| ()),
            Layer::read_portable_edges(&edges_path).map(|_| ()),
        ] {
            match result {
                Err(DurabilityError::RecordBlobUnreadable { path, cause }) => {
                    assert_eq!(path, edges_path);
                    assert!(
                        cause.starts_with(
                            "schema tag mismatch: this store expects `store::tests::Node`"
                        ),
                        "{cause}"
                    );
                }
                Err(other) => panic!("expected RecordBlobUnreadable, got {other:?}"),
                Ok(()) => panic!("expected RecordBlobUnreadable, got a layer"),
            }
        }

        // `open` with the caller's edges treats the wrong-tag blob as
        // stale and rewrites it under `Node`'s tag (`SCHTAG-FR-004`).
        let healed = Layer::open(BaseStore::new(nodes()), &edges(), &edges_path).unwrap();
        assert_eq!(Neighbors::<Node, Linked>::neighbors(&healed, 1), vec![2, 3]);
        assert_eq!(Layer::read_portable_edges(&edges_path).unwrap(), edges());
        let portable = Layer::open_portable(BaseStore::new(nodes()), &edges_path).unwrap();
        assert_eq!(all_neighbors(&portable), all_neighbors(&healed));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_version_1_edge_blob_is_a_version_error_and_healed_by_open() {
        let (dir, edges_path) = scratch("symmetric_v1_blob");
        let _ = Layer::create(BaseStore::new(nodes()), &edges(), &edges_path).unwrap();
        // The exact image STORAGE-016 v0.1.0 wrote: version 1, the shared
        // 20-byte header, no tag (`SCHTAG-FR-006`).
        let bytes = std::fs::read(&edges_path).unwrap();
        let mut v1 = bytes[..20].to_vec();
        v1.extend_from_slice(&bytes[28..]);
        v1[8..12].copy_from_slice(&1u32.to_le_bytes());
        std::fs::write(&edges_path, &v1).unwrap();

        match Layer::read_portable_edges(&edges_path) {
            Err(DurabilityError::RecordBlobUnreadable { cause, .. }) => {
                assert!(
                    cause.starts_with("blob version mismatch: file has 1, this build expects 2"),
                    "{cause}"
                );
            }
            Err(other) => panic!("expected RecordBlobUnreadable, got {other:?}"),
            Ok(_) => panic!("expected RecordBlobUnreadable, got edges"),
        }

        let healed = Layer::open(BaseStore::new(nodes()), &edges(), &edges_path).unwrap();
        assert_eq!(Neighbors::<Node, Linked>::neighbors(&healed, 1), vec![2, 3]);
        assert_eq!(std::fs::read(&edges_path).unwrap(), bytes);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
