# Server Entity Aliases and Case-Insensitive Name Lookup (Accepted)

- Status: **Accepted** (promoted from Proposed on 2026-09-05 — the
  owner approved the design as proposed, `ADR-0040` option (a):
  `aliases` as a durable field, `NameIndex` as the new secondary-index
  primitive, `label`/alias lookup via the existing `FilterEq` shapes,
  no protocol bump; (b) lookup-only without `aliases` and (c) closing
  as not warranted both declined). Acceptance authorizes the design;
  implementation follows as its own unit — see `ADR-0040`'s
  "Acceptance and implementation" section.
- Date: 2026-09-05
- Related: `ADR-0039`/`docs/design/SERVER-ENTITY-V2-REDESIGN-DESIGN.md`
  (named both `aliases` and case/whitespace-insensitive name
  resolution as explicit, paired Non-goals — this round closes both),
  `ADR-0037`/`docs/design/SERVER-ENTITY-DOMAIN-DESIGN.md` (`Entity`
  v1, superseded by v2), `SERVER-001-FR-041` (`Entity` v2's own
  implementation, the current shape this round revises), `ADR-0022`
  (the append-only wire-compatibility rules — cited below to show this
  round's headline capability needs **no** protocol bump at all),
  Unit 41's own integration-verification findings (`docs/design/
  SERVER-ENTITY-INTEGRATION-VERIFICATION-DESIGN.md`, Finding 3: real
  `aliases`; the "case- and whitespace-insensitively" resolution rule
  named in the same round).
- Supersedes: none. Additive to `Entity` v2's existing shape — no
  field or relation is removed, `label`/`kind`/`mention_count`/
  `RelatesTo`/`MentionedWith` are all unchanged.

## Purpose and scope

`ADR-0039`'s own "Non-goals" named two real `rusty_remind_me`
capabilities and deliberately declined to build either that round:
`aliases` (Unit 41 Finding 3 — real, first-class, multiple alternate
names per entity) and case/whitespace-insensitive `name` resolution
(`rusty_remind_me` resolves its own primary key that way; this crate's
`FilterEq` has always been exact-string equality). Both were named as
real, deferred gaps, not built.

This document investigates the current, real, merged shape (`Entity`
v2, `SERVER-001-FR-041`) directly and proposes closing both gaps
together, since they are the same underlying capability from a
caller's point of view: "give me the id of the entity known by this
name," where "this name" may be spelled differently in case or
whitespace, and may be the primary `label` or one of several aliases.

The investigation below finds the headline capability — resolving an
id from a name or alias, normalized — needs **no wire-protocol
change at all**, a genuinely smaller footprint than `Entity` v2's own
protocol-10 round. It also finds a real, structural reason this can't
simply extend `kind`'s existing `IndexedField` slot, and a real,
new-in-this-crate's-history limitation on what `aliases` itself can
expose over the wire this round.

## Non-goals

- **Not making `aliases` readable over the wire this round.**
  `ScanValue` (`src/server/protocol.rs`) has exactly five variants —
  `U32`/`I64`/`Bool`/`Str`/`F64` — none of them a list. Every existing
  field in this crate's history that carries "every capability flag
  `false`" (`label` itself, `Reminder::title`, `Order::created_at_
  unix_ms`) is still *representable* as one `ScanValue` and so still
  appears in a `GetById`/`Query` response. `aliases: Vec<String>` is
  not representable as any existing `ScanValue` variant at all — this
  is a genuinely new category, not a repeat of `label`'s own shape:
  a durable field with **no wire representation whatsoever**, not
  merely one with every capability flag off. Exposing it would need a
  new `ScanValue::StrList(Vec<String>)` variant (a real, small,
  appended wire addition, precedent: `F64`, ADR-0035) or remodeling
  aliases as edges to synthetic string-keyed nodes (examined below,
  rejected). Deferred explicitly — see "Open questions."
  *Deferred no longer: `ADR-0041` / `SERVER-001-FR-044` (v0.34.0) added
  `ScanValue::StrList` at protocol 11.*
- **Not changing `RecordId` or `Entity`'s identity.** `Entity` stays
  `Uuid`-keyed, unchanged from `ADR-0039`. Resolving a name/alias to
  an id remains a real, separate round trip for a caller who only has
  a name — this round makes that round trip real and correct, not a
  single-hop replacement for it.
- **Not fuzzy or substring matching.** `rusty_remind_me` resolves
  `name` "case- and whitespace-insensitively" (Unit 41), not
  fuzzily — this round's own normalization (lowercase, trim) matches
  that exactly, no more. A caller wanting prefix/fuzzy search gets
  `Unsupported` from this mechanism, same as before.
- **Not deduplicating or validating name/alias collisions across
  entities.** Two entities sharing a normalized name or alias is not
  rejected or flagged — `filter_eq` already returns `Vec<RecordId>`
  (zero, one, or many matches), the same shape every other `FilterEq`
  call in this crate already has; a caller gets every match and
  decides what to do with more than one.
- **Not generalizing this mechanism to any other domain this round.**
  `Entity` is again the one instance proving the primitive is real,
  matching `MultiSymmetric`/`MultiNeighbors`'s own precedent one round
  earlier — built domain-agnostic in `crate::generic`, exercised by
  exactly one domain.
- **Not Unicode-aware case folding.** Normalization is `str::
  to_lowercase()` plus `str::trim()` — correct for ASCII and most
  single-codepoint-per-character text, not a full Unicode
  case-folding implementation (`ß`/`İ`-shaped edge cases are out of
  scope). Named plainly; no evidence this round has that it matters
  for real `rusty_remind_me` data, since that repository's own source
  is still unread (Unit 41's own standing open question). *Since read —
  `ADR-0042`, Finding F1: its `normalize_entity_name` is
  `split_whitespace().join(" ").to_lowercase()`, also not Unicode case
  folding; this Non-goal holds, but the trim-only whitespace rule this
  document proposed did not — corrected at `ENT5-FR-001`.*

## Context and terminology

`Entity` v2's real, merged shape (`src/generic/entity.rs`, `SERVER-
001-FR-041`): `Entity { id: Uuid, label: String, kind: String,
mention_count: i64 }`. `kind` is the domain's one `IndexedField`
(`KindField`, `IndexValue = String`) and one `ScannableField`
(`MentionCountField` is the scannable one, not `kind` — see `FR-041`'s
own "Recorded deviations"). `label` carries every capability flag
`false` today — present in every `GetById` result, but not filterable,
scannable, or updatable — specifically because `GenericMmapStore<R,
IndexMarker, ScanMarker>` (`src/generic/mmap_store.rs`) structurally
admits exactly one `IndexedField` marker and one `ScannableField`
marker per record type; `kind` already occupies the one index slot,
so `label` could not also become one without either widening that
struct's own type parameters (examined below) or a second, independent
mechanism.

Two facts, checked directly against the current code before writing
this proposal, make the shape below both possible and comparatively
small:

- **`GenericMmapStore`'s `records: HashMap<R::Id, R>` (line 454)
  already holds the full Rust struct**, not just the one indexed/
  scanned field — the mmap-slot-file constraints (`mmap_field.rs`'s
  fixed-width, `Copy`-typed contract) apply only to the *one* durable
  `ScannableField`'s own slot. A new plain field on `Entity`
  (`aliases: Vec<String>`) needs no mmap changes at all; it just rides
  along in the struct like every field that isn't the one indexed or
  scanned field already does.
- **The record blob (`src/generic/record_blob.rs`) is `Vec<R>`,
  `serde`-serialized whole** (line 18's own doc comment: "the blob is
  `Vec<R>` alone"). Durability for a new plain field is free — no new
  blob format, no `BLOB_VERSION` bump, the same reason `mention_count`
  and `label` themselves needed no special durability handling beyond
  `Entity`'s own `#[derive(Serialize, Deserialize)]`.

What is *not* free: `GenericMmapStore`'s own primary index (`index:
HashMap<R::IndexValue, Vec<R::Id>>`, line 455) is a **single** map,
rebuilt from `records` at `create`/`open` (not separately persisted —
confirmed by reading the same construction path `open`/`create` both
call). Making `label`-or-`alias` lookups real needs a **second** such
map, and `GenericMmapStore<R, IndexMarker, ScanMarker>`'s own two type
parameters don't have room for a second `IndexMarker` without a
structural rework of the struct and its mmap-slot file layout — the
same shape of constraint `Symmetric`'s single direct `Neighbors` impl
put on multi-relation lookup one round earlier, resolved there by a
new, independent wrapper primitive (`MultiSymmetric`) rather than
widening `GenericMmapStore` itself. The same move applies here.

## Requirements

- `ENT3-FR-001` — **`Entity` gains `aliases: Vec<String>`** (new,
  plain field, durable via the existing whole-record blob
  serialization — no mmap/index special-casing). Carries no `FieldRef`
  tag and does not appear in any `GetById`/`Query`/`Aggregate`
  response this round — see Non-goals for why this is not merely
  "every capability flag `false`" but a genuinely new "no wire
  representation at all" category.
- `ENT3-FR-002` — **A new, domain-agnostic secondary-index primitive**,
  `NameIndex<S, R>` (`src/generic/store.rs`, new) plus its own trait,
  `NameIndexed` (`src/generic/query.rs`, new: `fn index_keys(&self) ->
  Vec<String>`) — the `MultiSymmetric`/`MultiNeighbors` naming and
  placement precedent, one round later. `NameIndex` wraps an inner
  store, builds `HashMap<String, Vec<R::Id>>` from the inner store's
  own records at construction (via its `AllIds`/`GetById`, the same
  "rebuilt from records, not separately persisted" shape
  `GenericMmapStore`'s own primary index already has — **no new blob
  file**, a real, notable asymmetry from `MultiSymmetric`'s own
  per-label `.edges` blobs, since index keys are fully derivable from
  each record's own fields while edges are not).
- `ENT3-FR-003` — **Normalization**: lowercase (`str::to_lowercase`)
  plus trim (`str::trim`), applied identically when `NameIndex` builds
  its map and when a lookup query arrives — matching Unit 41's own
  "case- and whitespace-insensitively" finding. A small, pure,
  fully-specified transform, hand-tested directly (the same
  sufficiency reasoning `src/server/pem.rs`'s own PEM decoder tests
  already use for a comparable transform) — no `AUTH-FR-006`-style
  timing measurement needed, this is not a secret-comparison path.
- `ENT3-FR-004` — **`Entity::index_keys` returns `[normalize(label)]`
  plus one entry per `normalize(alias)` for each `aliases` entry** —
  every name a caller might reasonably resolve the entity by, in one
  list, deduplication left to `HashMap`'s own key semantics (inserting
  the same normalized key twice for one id is a harmless no-op,
  `Vec::push`ing the same id twice is avoided by checking `contains`
  before insert — see "Proposed shape").
- `ENT3-FR-005` — **`EntityConnectionStore::filter_eq` on
  `FIELD_LABEL` becomes real** (was `Unsupported`): normalizes the
  incoming `ScanValue::Str` query the same way, looks it up in the new
  index, returns every matching id (`Ok(vec![])` for no match, the
  same "empty, not an error" shape every other `FilterEq` already
  has). `(FIELD_LABEL, _)` for any other `ScanValue` variant stays
  `Err(ErrorCode::Malformed)`. **No new `Request`, `Response`,
  `ErrorCode`, or `PROTOCOL_VERSION`** — this reuses `Request::
  FilterEq`/`ScanValue::Str`/`Response::RecordList` exactly as they
  exist today.
- `ENT3-FR-006` — **`DomainSchema`'s reported `label` field
  capabilities gain `filter_eq: true`** (was `false`). A data-value
  change on an existing, unchanged-shape struct, not a wire-structure
  change — `DomainSchema`'s own field layout (`FieldCapabilities`'s
  struct definition) is untouched, so this needs no version bump
  either, re-confirmed against `ADR-0022`'s own rules (which govern
  *structural* compatibility, not the data a schema reports).
- `ENT3-FR-007` — **`GenericProductionStore` gains one inherent
  method**, `find_by_name<R>(&self, name: &str) -> Vec<R::Id>` where
  `S: FindByName<R>` (`src/generic/production.rs`), mirroring
  `neighbors`/`neighbors_by_relation`'s own thin-wrapper precedent —
  normalizes `name` once, delegates to the `NameIndex` layer.

## Considered options

**Where normalization happens.** **(a) (proposed) server-side,
always** — `NameIndex` normalizes both when building its map and when
a `filter_eq` query arrives, so a caller may pass raw, un-normalized
text and still resolve correctly; matches how `kind_from_u32`-shaped
validation has always lived server-side in this crate, not pushed to
every client. **(b) client-side, server treats as exact match** —
rejected: pushes correctness-critical logic to every caller
independently, real risk of client/server normalization drifting
apart over time with no way to detect it. **(c) both** — redundant,
adds nothing (b) doesn't already risk, rejected.

Option (a) proposed.

**How to add the second index.** Investigated directly against
`GenericMmapStore`'s own structure: **(a) widen `GenericMmapStore<R,
IndexMarker, ScanMarker>` to `GenericMmapStore<R, IndexMarker,
ScanMarker, SecondIndexMarker>` (or an arbitrary list)** — rejected;
the mmap-slot file's own layout and every existing call site's type
signature are built around exactly one index and one scan slot,
widening it is a materially larger structural rework than this
round's mandate, the same proportionality call `ADR-0037` made when it
deferred the `Symmetric`-forwarding fix rather than rework `Symmetric`
in place (a call Unit 42 later had to revisit anyway, for a different
reason — but the *lesson*, prefer a new wrapper primitive over
reworking an existing structural constraint, is the one this option
would ignore). **(b) (proposed) a new, independent wrapper primitive**,
`NameIndex`, alongside `GenericMmapStore` — the `MultiSymmetric`
precedent applied to indices instead of relations; sidesteps the
constraint rather than fighting it. **(c) client-side full-table scan
with case-insensitive comparison, no server-side index at all** —
rejected: real cost this specifically avoids (every other `IndexedField`
lookup in this crate is server-side for the same reason — `O(matches)`
not `O(records)`), and this crate has never shipped a
`FilterEq`-shaped capability that requires a full scan when an index
is possible.

Option (b) proposed.

**What to build for `aliases`' own wire-readability this round.**
**(a) add `ScanValue::StrList(Vec<String>)` now, protocol 11** — a
real, small, appended addition (the `F64`/ADR-0035 precedent), but
this round's actual load-bearing need — resolving an id *from* a name
or alias — does not require it at all; building it anyway would be
scope beyond what's needed to close the two named non-goals. **(b)
(proposed) defer readability entirely, ship the lookup-only capability
with zero protocol change** — the smaller, proportional slice matching
this round's own real motivating need, the same "name the gap
precisely, build only what's needed now" discipline `ADR-0037`/`ADR-
0039` both already used. **(c) remodel `aliases` as edges to
synthetic, string-keyed nodes (`HasAlias`-shaped relation)** —
examined and rejected: every relation in this crate connects two
records of a `Record`-implementing type via `SymmetricRelation`/
`ChildOf`; a bare `String` is not a `Record`, so this would need an
entirely new relation-to-non-record-value primitive, clearly out of
proportion to what closing these two non-goals needs.

Option (b) proposed.

## Proposed shape

```rust
// src/generic/query.rs
/// A record that resolves under more than one normalized string key
/// (a primary name plus zero or more aliases) — the runtime-keyed
/// analogue of `IndexedField<Marker>` for a caller who doesn't yet
/// know a record's id, only one of the strings it might be called.
pub trait NameIndexed: Record {
    /// Every string this record should resolve under, un-normalized —
    /// `NameIndex` normalizes each entry the same way at both build
    /// and query time.
    fn index_keys(&self) -> Vec<String>;
}

pub trait FindByName<R: NameIndexed> {
    fn find_by_name(&self, name: &str) -> Vec<R::Id>;
}

// src/generic/store.rs
pub struct NameIndex<S, R: NameIndexed> {
    inner: S,
    index: HashMap<String, Vec<R::Id>>,
}

fn normalize(s: &str) -> String {
    s.trim().to_lowercase()
}

impl<S, R: NameIndexed> NameIndex<S, R>
where
    S: GetById<R> + AllIds<R>,
{
    pub fn new(inner: S) -> Self {
        let mut index: HashMap<String, Vec<R::Id>> = HashMap::new();
        for id in inner.all_ids() {
            let record = inner.get(id).expect("id from all_ids() exists");
            for key in record.index_keys() {
                let bucket = index.entry(normalize(&key)).or_default();
                if !bucket.contains(&id) {
                    bucket.push(id);
                }
            }
        }
        Self { inner, index }
    }
}

impl<S, R: NameIndexed> FindByName<R> for NameIndex<S, R> {
    fn find_by_name(&self, name: &str) -> Vec<R::Id> {
        self.index.get(&normalize(name)).cloned().unwrap_or_default()
    }
}

// GetById/AllIds/Flush/FilterEq/ScanField/UpdateField/Neighbors-family
// all forward to `inner` unchanged — the identical forwarding shape
// `MultiSymmetric` already established for the layer beneath it.
```

```rust
// src/generic/entity.rs
pub struct Entity {
    pub id: Uuid,
    pub label: String,
    pub kind: String,
    pub mention_count: i64,
    /// ENT3-FR-001: new. No `FieldRef`, no wire representation this
    /// round — see module docs / this design's own Non-goals.
    pub aliases: Vec<String>,
}

impl NameIndexed for Entity {
    fn index_keys(&self) -> Vec<String> {
        let mut keys = vec![self.label.clone()];
        keys.extend(self.aliases.iter().cloned());
        keys
    }
}

pub type EntityProductionStack =
    NameIndex<MultiSymmetric<GenericMmapStore<Entity, KindField, MentionCountField>, Entity>, Entity>;
```

```rust
// src/server/entity.rs — EntityConnectionStore::filter_eq
fn filter_eq(&self, field: FieldRef, value: &ScanValue) -> Result<Vec<RecordId>, ErrorCode> {
    match (field, value) {
        (FIELD_KIND, ScanValue::Str(kind)) => Ok(self.store.filter_eq::<Entity>(kind)),
        (FIELD_KIND, _) => Err(ErrorCode::Malformed),
        (FIELD_LABEL, ScanValue::Str(name)) => Ok(self.store.find_by_name::<Entity>(name)),
        (FIELD_LABEL, _) => Err(ErrorCode::Malformed),
        (FIELD_MENTION_COUNT, _) => Err(ErrorCode::Unsupported),
        _ => Err(ErrorCode::UnknownField),
    }
}
```

## Data/state and invariants

- No new persistent file. `NameIndex`'s own map is rebuilt from the
  wrapped store's records at construction (`create`/`open`/`open_
  portable` all route through `NameIndex::new`), the same "derived,
  not separately durable" shape `GenericMmapStore`'s own primary index
  already has — unlike `MultiSymmetric`'s per-label `.edges` blobs,
  which *are* separately persisted, since edges aren't derivable from
  records alone the way index keys are. Worth naming plainly: the two
  "Multi-" primitives this line has now produced are not the same
  shape of thing, despite the shared naming convention — one needs its
  own durable file, the other doesn't, and this document's own
  "Requirements" section names why.
- `Entity`'s `SchemaTag` (`"entity::Entity"`) is unchanged — the
  record type gains a field, not a new identity; existing `Entity`
  v2-shaped `<path>.records` blobs from before this round are not
  binary-compatible with the new, four-field struct (a real, accepted
  break — `Entity` still has zero real deployed data, the same
  "straight revision, not a migration" posture every prior `Entity`
  round has used).

## Errors, failure, recovery, and observability

- No new `ErrorCode`. A name/alias that matches nothing is `Ok(vec![])`,
  the same "empty, not an error" shape every other `FilterEq` already
  has — never `ErrorCode::UnknownField`, which is reserved for a field
  reference the schema doesn't have at all.
- `(FIELD_LABEL, _)` for any non-`Str` `ScanValue` is `ErrorCode::
  Malformed` — the same "wrong value kind for this field" shape
  `FIELD_KIND`'s own non-`Str` case already has.

## Security, privacy, and compatibility

- **No `PROTOCOL_VERSION` change.** `Request::FilterEq`/`ScanValue::
  Str`/`Response::RecordList` are all unchanged, byte-for-byte;
  `DomainSchema`'s own struct layout is unchanged. Only a data value
  inside an existing field (`label`'s `FieldCapabilities.filter_eq`,
  `false` → `true`) changes — re-checked directly against `ADR-0022`'s
  own four rules, which govern the *shape* of `Request`/`Response`/
  `DomainSchema`, not what values a server reports inside them. A
  `SchemaDrivenClient` built before this round already handles a
  domain reporting `filter_eq: true` on some field and `false` on
  another — that's the schema-driven design's whole point (`ADR-
  0011`), not new behavior this round has to build.
- `aliases` itself carries zero wire exposure this round — nothing to
  gate, encrypt, or audit differently than `Entity`'s existing fields
  already are.
- `find_by_name`/`filter_eq` on `label` are read-only, gated exactly
  like every other `FilterEq` call (authentication only); not
  overlaid by a read-your-writes session nor tracked into a
  snapshot-isolation read set beyond what `FilterEq` itself already
  is (unchanged — `FilterEq` was never read-your-writes-overlaid or
  read-set-tracked to begin with, only `GetById` is).

## Acceptance criteria

1. `Entity` gains `aliases: Vec<String>`, durable across `create`/
   `open`/`open_portable`, with no `FieldRef` and no appearance in any
   `GetById`/`Query`/`Aggregate` response.
2. `NameIndex`/`NameIndexed`/`FindByName` exist exactly as specified;
   `EntityProductionStack`'s new shape compiles and every existing
   `MultiSymmetric`-forwarded operation (`neighbors_by_relation`,
   `list_relation_kinds`, `filter_eq` on `kind`, `scan_field`/`update_
   field` on `mention_count`) still works unchanged through the new
   outer layer.
3. `Request::FilterEq` on `FIELD_LABEL` with a query matching
   `label` exactly (any case, any leading/trailing whitespace)
   returns that entity's id; matching an alias, same; matching
   neither returns `Ok(vec![])`; a non-`Str` value is `Malformed`.
4. Two entities sharing a normalized name or alias both appear in the
   result — no silent collision handling.
5. `DomainSchema` for the `Entity` domain reports `filter_eq: true`
   for `label`'s field, unchanged (`false`) for `mention_count`; no
   other domain's schema changes at all.
6. Every pre-existing golden vector in `src/server/protocol.rs` is
   byte-for-byte unchanged; `PROTOCOL_VERSION` stays 10.
7. Every existing test in `tests/server_*.rs` for every domain other
   than `Entity` is unchanged; `Entity`'s own tests gain coverage for
   the new field and lookup capability without breaking any existing
   assertion.

## Verification plan

- `src/generic/store.rs` unit tests: `NameIndex` construction from a
  small in-memory fixture (multiple keys per record, two records
  sharing a key, an empty-aliases record), `find_by_name` with mixed
  case/whitespace input, a miss returning empty, every forwarded
  operation (`GetById`/`AllIds`/`Flush`/`FilterEq`/`ScanField`/
  `UpdateField`/the `MultiNeighbors` family) exercised through the new
  outer layer against a `MultiSymmetric`-wrapped fixture.
- `src/generic/entity.rs` unit tests: `aliases` round-trips through
  `create`/`open`/`open_portable`; `index_keys()` returns `label` plus
  every alias, un-normalized (normalization is `NameIndex`'s own job,
  not `Entity`'s).
- `src/server/entity.rs` unit tests: `filter_eq` on `FIELD_LABEL` by
  exact `label`, by an alias, by mismatched case/whitespace, by a
  non-`Str` value, and a real miss; `describe()`'s reported `label`
  capabilities.
- `tests/server_entity_integration.rs` (extended, not rewritten):
  real-socket `FilterEq` on `label`/an alias/a case-varied query
  against a real `entity_server`, plus a two-entities-share-an-alias
  fixture proving both ids come back.

## Traceability

- → `SERVER-001` next minor / FR (`ENT3-FR-001`–`007`), a new ADR
  (`ADR-0040`) — closes `ADR-0039`'s own two paired Non-goals
  (`aliases`, case/whitespace-insensitive name resolution).
- Not sourced from `docs/FUTURE-GROWTH.md` — the sixth round in the
  `rusty_remind_me`-motivated line `ADR-0036` started.

## Open questions

- Whether `aliases` ever becomes wire-readable, and via which
  mechanism (`ScanValue::StrList` vs. a relation-based remodeling) —
  named, not decided (see Non-goals). *Decided and built: `ScanValue::StrList` at
  protocol 11, `ADR-0041` / `SERVER-001-FR-044` (v0.34.0), with a
  rule-3 content strip so pre-11 clients see this round's shape.*
- Whether Unicode-aware case folding ever matters for real
  `rusty_remind_me` data — no evidence yet either way, that
  repository's own source still unread. *Source since read (`ADR-0042`,
  F1): it uses plain `to_lowercase` too, so the two systems agree; still
  no evidence either way that folding matters — open on the merits, no
  longer for lack of information.*
- Whether a combined "resolve name, then fetch full record" single
  round trip is ever worth a server-side primitive, rather than the
  two separate calls (`FilterEq` then `GetById`) this round leaves a
  caller to make — named, not solved; real cost, not yet shown to
  matter, the identical open question `ADR-0039`'s own design doc
  already left standing for plain name-based lookup generally.

## Change history

- 2026-09-05: Initial proposal, the sixth round in the
  `rusty_remind_me`-motivated line `ADR-0036` started — closes `ADR-
  0039`'s own paired `aliases`/case-insensitive-name-resolution
  Non-goals via a new secondary-index primitive, `NameIndex`, needing
  no `PROTOCOL_VERSION` change for the headline lookup capability.
- 2026-09-05: Accepted as proposed, `ADR-0040` option (a); (b) and (c)
  declined.
- 2026-09-05: **Implemented** (`SERVER-001-FR-042`, v0.32.0). Landed
  as proposed — no deviation. Every "Proposed shape" sketch is real
  code with the same names (`NameIndexed::index_keys`, `FindByName::
  find_by_name`, `NameIndex::new`, `normalize`, the `filter_eq` arm);
  `src/server/protocol.rs` untouched, as predicted. The "no new file"
  invariant in "Data/state and invariants" is pinned by a test that
  lists the store directory. One thing this document left implicit is
  now a pinned test: the SQL `WHERE label = '..'` path stays an
  exact-match full scan, deliberately separate from the normalized
  `FilterEq` — see `ADR-0040`'s own implementation-log entry.
