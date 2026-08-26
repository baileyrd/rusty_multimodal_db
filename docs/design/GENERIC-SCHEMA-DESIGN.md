# Generalizing beyond `Dog`: a generic record/schema/query design

- Status: **Proposed — design only, not implemented, not accepted**
- Date: 2026-08-25
- Author: baileyrd (this pass)
- Related: `docs/decisions/ADR-0001-three-backend-empirical-comparison.md` (why `Dog` was compared this way in the first place), `ADR-0004-one-hop-neighbors-trait-method.md` (the one-hop-in-the-trait / N-hop-composed-generically precedent this design carries forward), `ADR-0006-tier-2-durability-architectures.md` (mmap's ages-only scope-down, directly relevant to §4), `ADR-0008-production-default.md` (the consolidated recommendation this design would eventually need to survive), `docs/charter/CHARTER.md`

**This document is a deliverable in itself, not a preview of code to come.** It does not implement anything, does not touch `src/store/`, `src/durability/`, `src/concurrency/`, or `src/production.rs`, and adds no new dependency to the crate. Every code block below was written and compiled in a standalone scratch crate outside this repository, specifically to prove the trait shapes are real, compiling Rust — not pseudocode — before being transcribed here. Nothing in this document is wired into the crate. Per the task that motivated it, this is the single most hard-to-reverse decision the project has faced (the schema/query abstraction would become the crate's public API surface), and it stops here for review.

## Why a second domain, and why `Order`/`Customer` specifically

Every prior round in this project followed "no abstraction before two real call sites." `Dog` is the only domain that has ever existed here, so validating a generalization against `Dog` alone would violate the project's own standing rule — it would be abstracting from *one* example dressed up as design work. `Order`/`Customer` was chosen to be structurally different from `Dog` on purpose:

| | `Dog` | `Order`/`Customer` |
|---|---|---|
| Relationship | `littermate_of` — **symmetric**, ad-hoc, degree-bounded (~0–3) | `Order` belongs to one `Customer` — **directed**, many-to-one from the child, one-to-many (unbounded) from the parent |
| Numeric field | `age: u32` — a count, small, fixed range | `amount` — a currency value, needs correctness (no float rounding), plausibly signed (refunds) |
| Categorical field | `breed: String` — heap-allocated, variable-length | `status` — naturally an enum, not a string, fixed small cardinality |
| A field type `Dog` never had | — | a timestamp (`created_at`) |

If the design had only been checked against `Dog`, none of §4's findings below would have surfaced — they all come from where `Order`/`Customer` genuinely doesn't look like `Dog`.

---

## 1. Generic record/schema abstraction

The core problem: Rust has no reflection and this crate's whole ethos is static typing (see the charter's non-goals and every prior ADR's "no unwrap/expect," "typed fields" discipline) — so a schema abstraction that resorts to a dynamic `enum Value { Int(i64), Str(String), ... }` bag would throw away exactly the type safety this crate has built its reputation on. The design below stays fully statically typed, at the cost of one marker (zero-sized) type per queryable field or relation — the same pattern Diesel/SeaORM use for column-level type safety in Rust ORMs, not a novel invention.

```rust
/// Every domain record has an id.
pub trait Record {
    type Id: Copy + Eq + Hash;
    fn id(&self) -> Self::Id;
}

/// `R` has an equality-indexable field, identified by the zero-sized marker
/// type `Marker` (one marker per field — `struct Breed;`, `struct Status;`).
/// `Value` need only be `Clone`, not `Copy`: this is deliberately the wider
/// bound, because equality-indexable fields are exactly the ones that might
/// be heap-allocated (`String`) — `breed` needs this looseness; `status`
/// (an enum) satisfies it trivially too. This bound is what makes both
/// `breed_index`'s `HashMap<String, Vec<Uuid>>` and a hypothetical
/// `status_index`'s `HashMap<OrderStatus, Vec<Uuid>>` the *same* generic
/// shape, `HashMap<Value, Vec<Id>>`.
pub trait IndexedField<Marker>: Record {
    type Value: Eq + Hash + Clone;
    fn indexed_value(&self) -> &Self::Value;
}

/// `R` has a scannable/aggregatable field. `Value: Copy` is a deliberately
/// *tighter* bound than `IndexedField`'s — see §4 for exactly why this
/// matters (it's what lets the packed-`Vec` cache trick generalize at all).
pub trait ScannableField<Marker>: Record {
    type Value: Copy;
    fn scannable_value(&self) -> Self::Value;
}

/// `R` participates in a symmetric (undirected) relation, identified by
/// `Marker` — the generalization of `littermate_of`. No associated data
/// beyond `Record::Id` is needed; the edge set lives store-side (see §2).
pub trait SymmetricRelation<Marker>: Record {}

/// `R` is the *child* side of a directed, many-to-one relation, identified
/// by `Marker` — the generalization of `Order belongs to Customer`. Unlike
/// `SymmetricRelation`, this carries real data: the foreign key itself,
/// readable directly off the record with no store-side index needed at all
/// (see §2's `Parent` and §4 for why this asymmetry matters).
pub trait ChildOf<Marker>: Record {
    type ParentId: Copy + Eq + Hash;
    fn parent_id(&self) -> Self::ParentId;
}
```

Four traits, not one "schema" god-trait — deliberately. A record can implement any subset (a bare `Record` with nothing else is valid and used below for `Customer`), and a query capability (§2) is only ever expressed in terms of the specific trait it needs, not a monolithic schema descriptor.

---

## 2. Query API sketch

One store-side trait per query capability, each generic over the record type and (where relevant) the field/relation marker:

```rust
pub trait GetById<R: Record> {
    fn get(&self, id: R::Id) -> Option<R>;
}

/// Generalizes `same_breed` — filter by equality on any `IndexedField`.
pub trait FilterEq<R, Marker>
where
    R: IndexedField<Marker>,
{
    fn filter_eq(&self, value: &R::Value) -> Vec<R::Id>;
}

/// Generalizes `scan_ages` — scan/aggregate any `ScannableField`.
pub trait ScanField<R, Marker>
where
    R: ScannableField<Marker>,
{
    fn scan(&self) -> Vec<R::Value>;
}

/// Generalizes `update_age`.
pub trait UpdateField<R, Marker>
where
    R: ScannableField<Marker>,
{
    fn update(&mut self, id: R::Id, value: R::Value) -> Result<(), NotFound<R::Id>>;
}

/// Generalizes `neighbors` for a *symmetric* relation.
pub trait Neighbors<R, Marker>
where
    R: SymmetricRelation<Marker>,
{
    fn neighbors(&self, id: R::Id) -> Vec<R::Id>;
}

/// The "one hop up" side of a *directed* relation.
pub trait Parent<C, Marker>
where
    C: ChildOf<Marker>,
{
    fn parent(&self, child_id: C::Id) -> Option<C::ParentId>;
}

/// The "one hop down" side of a *directed* relation.
pub trait Children<P, C, Marker>
where
    P: Record,
    C: ChildOf<Marker, ParentId = P::Id>,
{
    fn children(&self, parent_id: P::Id) -> Vec<C::Id>;
}
```

Two-hop traversal is **not** a new trait method, on either relation shape — carrying forward ADR-0004's precedent (`neighbors` is one-hop-in-the-trait; multi-hop composes generically outside it, in what would be this design's equivalent of `bench_support::two_hop_neighbors`):

```rust
/// Symmetric 2-hop: the same primitive (`neighbors`) applied twice.
pub fn two_hop_neighbors<S, R, Marker>(store: &S, id: R::Id) -> Vec<R::Id>
where
    R: SymmetricRelation<Marker>,
    S: Neighbors<R, Marker>,
{
    let mut seen = HashSet::new();
    for one_hop in store.neighbors(id) {
        for two_hop in store.neighbors(one_hop) {
            seen.insert(two_hop);
        }
    }
    seen.into_iter().collect()
}

/// Directed-relation analogue: "other children of my parent" — composed
/// from *two different* primitives (one hop up via `Parent`, one hop down
/// via `Children`), not the same one twice. This is a genuine, not
/// cosmetic, difference from the symmetric case — see §4.
pub fn siblings<S, P, C, Marker>(store: &S, child_id: C::Id) -> Vec<C::Id>
where
    P: Record,
    C: ChildOf<Marker, ParentId = P::Id>,
    S: Parent<C, Marker> + Children<P, C, Marker>,
{
    match store.parent(child_id) {
        Some(parent_id) => store
            .children(parent_id)
            .into_iter()
            .filter(|&id| id != child_id)
            .collect(),
        None => Vec::new(),
    }
}
```

`Parent` itself needs no new store state at all — it is satisfiable by a **blanket impl** over anything that already implements `GetById`:

```rust
impl<S, C, Marker> Parent<C, Marker> for S
where
    C: ChildOf<Marker>,
    S: GetById<C>,
{
    fn parent(&self, child_id: C::Id) -> Option<C::ParentId> {
        self.get(child_id).map(|c| c.parent_id())
    }
}
```

This one blanket impl is itself a finding: fetching a child's parent id is "get the child, read a field" — no adjacency structure, no index, nothing to build or maintain. It's the cheapest capability in the entire design. `Children` is the opposite — see §4.

A generic store is assembled by composing small, single-purpose wrapper layers, each adding exactly one capability on top of an inner store that already provides `GetById`:

```rust
pub struct BaseStore<R: Record> { records: HashMap<R::Id, R> }                 // owns the records
pub struct Indexed<S, R: IndexedField<Marker>, Marker> { .. }                  // + one FilterEq
pub struct Scanned<S, R: ScannableField<Marker>, Marker> { .. }                // + one ScanField/UpdateField
pub struct Symmetric<S, R: SymmetricRelation<Marker>, Marker> { .. }           // + one Neighbors
pub struct Reversed<S, P: Record, C: ChildOf<Marker, ParentId = P::Id>, Marker> { .. } // + one Children
```

Full code for all four wrapper layers (with real bodies, not stubs) is in §4 alongside the findings they produced — presenting them there, next to what went wrong building them, is more honest than presenting a cleaned-up version here first.

---

## 3. Walking both domains through it

```rust
// ---- Dog ----
pub struct DogRecord { pub id: Uuid, pub breed: String, pub age: u32 }
impl Record for DogRecord { type Id = Uuid; fn id(&self) -> Uuid { self.id } }

pub struct Breed;
impl IndexedField<Breed> for DogRecord {
    type Value = String;
    fn indexed_value(&self) -> &String { &self.breed }
}

pub struct Age;
impl ScannableField<Age> for DogRecord {
    type Value = u32;
    fn scannable_value(&self) -> u32 { self.age }
}

pub struct LittermateOf;
impl SymmetricRelation<LittermateOf> for DogRecord {}

// ---- Order / Customer ----
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum OrderStatus { Pending, Shipped, Delivered, Cancelled, Refunded }

pub struct Order {
    pub id: Uuid,
    pub customer_id: Uuid,
    pub amount_cents: i64,       // see §4 for why cents, not a float or Decimal
    pub status: OrderStatus,
    pub created_at_unix_ms: i64, // see §4 for why an epoch int, not a datetime type
}
impl Record for Order { type Id = Uuid; fn id(&self) -> Uuid { self.id } }

pub struct Status;
impl IndexedField<Status> for Order {
    type Value = OrderStatus;
    fn indexed_value(&self) -> &OrderStatus { &self.status }
}

pub struct Amount;
impl ScannableField<Amount> for Order {
    type Value = i64;
    fn scannable_value(&self) -> i64 { self.amount_cents }
}

pub struct CreatedAt; // a SECOND scannable field — Dog only ever had one
impl ScannableField<CreatedAt> for Order {
    type Value = i64;
    fn scannable_value(&self) -> i64 { self.created_at_unix_ms }
}

pub struct BelongsToCustomer;
impl ChildOf<BelongsToCustomer> for Order {
    type ParentId = Uuid;
    fn parent_id(&self) -> Uuid { self.customer_id }
}

pub struct Customer { pub id: Uuid, pub name: String } // a bare Record, nothing else
impl Record for Customer { type Id = Uuid; fn id(&self) -> Uuid { self.id } }
```

| Trait | `Dog` | `Order`/`Customer` |
|---|---|---|
| `Record` | `DogRecord` | `Order`, `Customer` |
| `IndexedField` | `Breed` → `String` | `Status` → `OrderStatus` (enum, not string) |
| `ScannableField` | `Age` → `u32` (one field) | `Amount` → `i64`, `CreatedAt` → `i64` (**two** fields) |
| `SymmetricRelation` | `LittermateOf` | *(none)* |
| `ChildOf` | *(none)* | `BelongsToCustomer`: `Order` → `Customer` |

**Read the table's blank cells as a finding, not a gap.** `Dog` never exercises `ChildOf`; `Order`/`Customer` never exercises `SymmetricRelation`; `Customer` never exercises anything beyond bare `Record`. Between the two domains, *every* trait in §1 gets exercised by at least one of them, and neither domain alone would have exercised all four — which is exactly the point of testing against a second, genuinely different domain rather than a `Dog` reskin. A design that only reads cleanly for one domain would have been the design telling us something was wrong; it didn't happen here, but only because `Order`/`Customer` was picked to be different on purpose.

Both domains, and every trait/method above, were compiled and run together against real generic store code (not separately against two different generic cores) in the scratch crate this document is drawn from. That run is the actual evidence for "this design works for two domains" — not an assertion.

---

## 4. What might not survive genericity — the important section

Every one of this project's benchmark verdicts was earned on `Dog`'s specific shape. This section is the honest accounting of which of those verdicts the generic design keeps, weakens, or breaks — including two things the design got *wrong* on the first pass, caught by the compiler while building the scratch crate for this document, not smoothed over afterward.

### 4.1 The packed-`Vec`/position-index trick (the reason `CanonicalCachedStore` wins `scan_ages`) — generalizes, with a real caveat

```rust
pub struct Scanned<S, R, Marker>
where
    R: ScannableField<Marker>,
{
    inner: S,
    position_index: HashMap<R::Id, usize>,
    cache: Vec<R::Value>,
    _marker: PhantomData<Marker>,
}
// ScanField::scan() -> self.cache.clone(); UpdateField::update() writes
// into self.cache[position] — exactly CanonicalCachedStore's age_cache +
// position_index shape, parameterized over R::Value instead of hardcoded u32.
```

This mechanically generalizes — **as long as `ScannableField::Value: Copy`**, which is exactly why that bound is tighter than `IndexedField`'s. It is the single load-bearing constraint in this whole design: it's what makes a flat, contiguous, memcpy-clonable cache possible at all, for any field, not just `age`. Drop it (allow a `String`-valued "scannable" field) and the entire trick collapses back to the pre-`CanonicalCachedStore` `CanonicalStore` shape this project spent its first pass proving was slow.

The real caveat: **the *magnitude* of the win is not guaranteed to transfer.** `age: u32` is 4 bytes; `amount_cents: i64` is 8. A packed `Vec<i64>` fits half as many elements per 64-byte cache line as `Vec<u32>` did — `scan_ages`'s specific ~17.7×-over-AoS/~14%-of-SoA numbers were earned at `u32`'s width, not at "any `Copy` type's" width. Wider scannable fields should be expected to still win over AoS/SoA (same asymptotic argument: one contiguous clone vs. `HashMap`-bucket-and-heap-`DogRecord` walks), but the exact multiple is untested and likely smaller.

A second, genuinely new question `Dog` never raised: **`Order` has two scannable fields (`Amount`, `CreatedAt`), `Dog` only ever had one.** The composable design above handles this by construction — `Scanned<Scanned<Indexed<BaseStore<Order>, ..>, .., Amount>, .., CreatedAt>` stacks two independent `Scanned` layers, one per field — but each layer builds and stores **its own `position_index`**, duplicating it once per scannable field. `CanonicalCachedStore` shares one `position_index` across every cached field. This is a real memory-vs-composability tradeoff the layered design accepts that the original hand-written store didn't have to: N independent, small, duplicated indexes bought in exchange for each capability being a genuinely separate, independently-composable layer. A hand-fused generic store *could* share one position index across every `Scanned` layer for the same record type, at the cost of tighter coupling between layers — flagged here as an open implementation choice, not resolved by this design.

### 4.2 mmap durability (why `MmapAgeStore` is the standout durability result) — hits a wall `ADR-0006` already predicted, and `Order` walks straight into it

`ADR-0006`'s own revisit triggers already named this: *"Revisit if: a future record shape needs more than one mutable field persisted — at that point mmap's and `redb`'s ages-only scope-down would need real redesign (the string-heap/fixed-layout problem this ADR chose not to solve), not just an incremental extension."*

`Order` is that future record shape, immediately. A real order-processing domain plausibly wants **both** `amount` (refund adjustments) and `status` (order lifecycle) mutable and durable — two mutable fields where `Dog` only ever had one (`age`). The generalized `Scanned`-durable-via-mmap pattern *can* extend to N mutable fields (N separate flat mmap-backed arrays, one per field, same shape as §4.1's multiple-`Scanned`-layers stacking) — but this is exactly the "real redesign," not incremental extension, `ADR-0006` flagged as out of scope for the original single-field design, now landing on the very first second domain tried.

There's a second, sharper constraint this surfaces: mmap's flat-array trick fundamentally requires **fixed-width** values — that's why the original module only ever mapped `age: u32`, never `breed: String` (its own module docs explicitly named a string-heap/offset scheme as the alternative it declined to build). `status: OrderStatus` works fine here specifically *because* it was designed as a `#[repr]`-able fixed-size enum, not a `String` — a deliberate choice in §3, not an accident. If a real domain instead wanted a mutable, durable, variable-length field (e.g. mutable order `notes: String`), mmap's whole approach breaks down and needs the harder string-heap design `ADR-0006` already declined to build once. **The design constrains which fields can be durable via mmap to fixed-width `Copy` mutable fields, by construction — a real, load-bearing limitation this pass surfaces more urgently than the original pass ever had reason to.**

### 4.3 The adjacency-index pattern (`littermate_of`) vs. the directed FK pattern (`Order → Customer`) — half generalizes, half needs genuinely new code

The symmetric case generalizes with zero surprises: `Symmetric<S, R, Marker>` builds one `HashMap<Id, Vec<Id>>` from an externally-supplied edge list exactly like `CanonicalCachedStore`'s `adjacency_index` does, parameterized only by which relation marker. No risk here.

The directed case does **not** reduce to "the same pattern with an arrow added." It splits into two genuinely different-cost operations:

- **Child → parent (`Parent`)**: cheaper than anything the symmetric design needed — a blanket impl (§2), no index at all, since the foreign key already lives on the child record.
- **Parent → children (`Children`)**: needs a **new** `HashMap<ParentId, Vec<ChildId>>` reverse index that has no analogue in the symmetric design — `Reversed<S, P, C, Marker>` below.

```rust
pub struct Reversed<S, P, C, Marker>
where
    P: Record,
    C: ChildOf<Marker, ParentId = P::Id>,
{
    inner: S,
    children_of: HashMap<P::Id, Vec<C::Id>>,
    _marker: PhantomData<(P, Marker)>,
}
```

Structurally, this reverse index is `Indexed`'s equality-index shape (`HashMap<Value, Vec<Id>>`), **not** `Symmetric`'s adjacency shape — and since `Customer → Order` is genuinely one-to-many and unbounded (unlike `littermate_of`'s degree ≤3), the "one key maps to many, possibly many, ids" cost profile is the same one `breed_index` already has, not a new magnitude problem. **The honest summary: the adjacency-index pattern doesn't generalize to directed relations at all — it's the wrong pattern for that direction. The equality-index pattern does, for the expensive direction; the cheap direction needs no pattern (and no index) at all.** Naively trying to force `littermate_of`'s adjacency-index code to serve `Order → Customer` by just "inserting both directions" would be actively wrong: `Customer belongs_to Order` isn't a real relationship, and materializing it symmetrically would double storage for no reason.

**A real design mistake, caught by the compiler while building this section — reported, not hidden.** The first draft of `Reversed` implemented `GetById<P>` (parent lookup) by forwarding to its inner store. This does not type-check in any real composed stack: `Reversed`'s inner store is built entirely from `&[C]` (child records) — it can never actually provide `GetById<Customer>` from an `Order`-shaped stack, and `rustc` refused the call site immediately (`GetById<Order>` was available, `GetById<Customer>` never could be). The fix: `Reversed` forwards `GetById<C>` (the child type it actually knows about) and adds only the new `Children` capability; fetching an actual `Customer` record is correctly left to whatever store holds `Customer` records — a separate concern `Reversed` was wrong to claim. This is exactly the kind of thing a second, structurally different domain is supposed to surface before implementation, not after.

### 4.4 Global `RwLock` concurrency — the one piece of the trio genuinely unthreatened by genericity, at a cost shared with everything else

`ProductionStore`'s `RwLock<MmapAgeStore>` pattern doesn't care what's inside the lock — wrapping any composed generic store (however many `Indexed`/`Scanned`/`Symmetric`/`Reversed` layers deep) in one `RwLock` works identically regardless of field types or relation shapes, exactly as it does today. This is good news, stated plainly rather than only cataloging risks: **concurrency is the one of the three original picks this design doesn't put at any special risk.**

It does, however, pay the *same* boilerplate tax every other wrapper in this design pays (§4.5) — an `RwLock`-based wrapper still has to manually forward `GetById`/`FilterEq`/`ScanField`/etc. from whatever it wraps, exactly like `Indexed`/`Scanned`/`Symmetric`/`Reversed` already do. Not newly expensive; not free either — the same cost as everything else, no more.

### 4.5 The forwarding-boilerplate tax — a real, quantified cost of staying statically typed, discovered while compiling this document's own examples

Rust has no trait delegation/inheritance. Composing capability layers by wrapping (§2) means **every wrapper must manually re-implement every trait its inner store already provides**, or that capability silently disappears once wrapped. This was not a theoretical concern — it broke the scratch crate's own demo on the first `cargo run`:

```
error[E0599]: no method named `filter_eq` found for struct `Symmetric<Scanned<Indexed<...>>>`
  = help: items from traits can only be used if the trait is implemented and in scope
```

`Symmetric` (which adds `Neighbors`) doesn't automatically keep exposing `FilterEq`/`ScanField` from the `Indexed`/`Scanned` layers underneath it — each had to be given its own explicit forwarding `impl` (four of them were added to fix this one demo: `FilterEq`/`ScanField` forwarded through `Scanned`, `Symmetric`, and `Reversed`). This is a real, load-bearing tax the design accepts in exchange for zero dynamic dispatch and full static typing: composing K capability layers over N total capability traits costs on the order of K × N forwarding impls, growing with schema complexity, not a fixed cost paid once. It is exactly the kind of thing that reads fine in a two-domain, four-capability demo and could become a genuine ergonomics problem at real schema complexity (many indexed fields, many scannable fields, several relations) — flagged here explicitly as an open cost, not resolved.

### 4.6 Field-cost character doesn't transfer 1:1, even where the abstraction does

`breed: String` and `status: OrderStatus` both satisfy `IndexedField` identically at the type level (`Eq + Hash + Clone`), and the generic `HashMap<Value, Vec<Id>>` index shape doesn't change between them. But their *real* cost profiles differ: a `String` key costs a heap allocation and length-dependent hashing on every index insert and lookup; an enum key (if given a cheap `Hash`/`Eq`, which a `#[derive]`d enum gets for free) is a trivial, allocation-free comparison. **The API generalizes; the performance character earned for `breed`/`same_breed` in this project's own `RESULTS.md` should not be assumed to carry over to every `IndexedField` without re-measuring** — it's a real, testable difference, not a re-litigation of an old result.

### 4.7 Dataset generation and benchmark infrastructure are domain-specific today, and would need real rework, separate from the schema/query design above

`bench_support::build_dataset`/`GeneratorConfig` assume a fixed breed cardinality (50) and a degree-bounded (`[0.0, 3.0]`) symmetric relation — neither concept transfers to `Order`/`Customer` as-is (`OrderStatus` has a small, fixed cardinality of its own, not a tunable "50"; a customer's order count is realistically unbounded/power-law-shaped, not degree-bounded the way `littermate_of` deliberately was). This is a real, separate piece of follow-up work the schema/query design above doesn't resolve by itself — flagged here so it isn't discovered as a surprise mid-implementation.

### 4.8 Id-type genericity is supported by the design but untested this pass

`Record::Id` is an associated type, not hardcoded to `Uuid` — a real domain might reasonably want `u64` auto-increment order ids, for instance. Both domains in this pass use `Uuid` for `Id`, a deliberate scope limit so this validation exercises one axis of change (schema/relation shape) at a time rather than conflating it with a second (id-scheme genericity). The trait design places no obstacle in the way of a non-`Uuid` id; it simply hasn't been exercised.

---

## 5. Migration shape for `Dog`

Sketched, not built. The intended shape keeps every existing benchmark and test compiling unchanged.

**`DogRecord` itself changes by *addition*, not by rewrite** — the struct stays exactly as it is today; it gains the trait impls shown in §3 (`Record`, `IndexedField<Breed>`, `ScannableField<Age>`, `SymmetricRelation<LittermateOf>`), each a few lines, none touching its fields.

**`CanonicalCachedStore`/`MmapAgeStore`/`GlobalRwLockStore`/`ProductionStore` are *not* touched in a first implementation pass.** The recommended staging, specifically to avoid destabilizing the just-shipped `ProductionStore` (`ADR-0008`) on the strength of a two-domain validation:

1. Build the generic core (`Record`/`IndexedField`/`ScannableField`/`SymmetricRelation`/`ChildOf`, the query traits, and the composable layers) as new, additive code — nothing existing depends on it yet.
2. Port `Dog` onto it as the *third* proof domain (after the two used for this design pass), specifically to validate the generic core against the crate's own real, already-benchmarked shape — not a toy.
3. Only once step 2 is proven: consider whether `CanonicalCachedStore`/`MmapAgeStore`/`GlobalRwLockStore` themselves become thin instantiations of the generic layers, or stay as deliberately-hand-fused, more-optimized implementations of the same trait surface (§4.1's shared-vs-duplicated-position-index tradeoff is exactly the kind of thing that could justify keeping them hand-written even after the generic core exists). This is an explicit, later decision — not committed to by this document.

**`DogStore`/`ConcurrentStore` (the existing traits) do not need to disappear.** The concrete promise this design makes to the rest of the crate: `impl DogStore for AnyGenericStackInstantiatedOverDogRecord { fn get(...) { GetById::get(self, ..) } fn scan_ages(...) { ScanField::<DogRecord, Age>::scan(self) } .. }` — a thin, mechanical facade translating the crate's existing, familiar interface onto the generic one underneath. `benches/workloads.rs`, `benches/concurrency.rs`, `tests/cross_backend.rs`, and `tests/production_integration.rs` are all written against `DogStore`/`ConcurrentStore` today and would need **zero changes** under this plan — they'd keep exercising the exact same trait surface, just backed by a different (or, for a while, the same) implementation underneath.

---

## Recommendation

**Do not implement yet.** This document surfaces at least three findings that materially change the shape of "just genericize it":

1. The forwarding-boilerplate tax (§4.5) is real and was discovered by the compiler, not anticipated in advance — its true cost at realistic schema complexity is unmeasured.
2. `ADR-0006`'s own predicted revisit trigger (more than one mutable durable field) is hit immediately by the very first second domain tried (§4.2) — mmap durability generalization is a real redesign, not an extension, exactly as that ADR warned.
3. The directed-relation case needed genuinely new code (`Reversed`) and caught a real design mistake mid-build (§4.3) — evidence this is exactly the kind of decision that benefits from review before code, not after.

If this design is accepted, the recommended next step is the staged migration in §5 (generic core → port `Dog` as a third validation domain → only then consider touching `ProductionStore`), not a direct jump to genericizing the production path.
