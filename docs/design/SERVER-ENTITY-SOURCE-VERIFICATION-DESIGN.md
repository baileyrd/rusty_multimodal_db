# Server Entity Source Verification (Accepted)

- Status: **Accepted** (promoted from Proposed on 2026-09-05 — the
  owner approved option (a): the ten findings accepted as the record of
  what `rusty_remind_me` does, and both follow-ups — `ENT5-FR-001`
  (normalization collapses internal whitespace) and `ENT5-FR-002`
  (derived `entity_id(name) -> Uuid`, adds `sha2`) — taken as one
  implementation unit; (b) normalization-only and (c) informational-
  only both declined. Acceptance authorizes the follow-ups;
  implementation follows as its own unit — see `ADR-0042`'s
  "Acceptance and implementation" section. The open-label/directed-
  edge/edges-with-hops divergences (F3/F4/F5) stay named for a future
  design round, not built.)
- Date: 2026-09-05
- Related: `docs/design/SERVER-ENTITY-INTEGRATION-VERIFICATION-DESIGN.md`/
  `ADR-0038` (Unit 41 — the same question asked of the MCP *tool
  schemas*; this round asks it of the *source*, which every round since
  has named as unread), `ADR-0039`/`ADR-0040`/`ADR-0041` (each carries
  the standing caveat "that repository's own source never having been
  read this session" — resolved here), `SERVER-001-FR-041`/`FR-042`
  (the merged `Entity` v2 + aliases shape every finding below is
  measured against), `docs/adr/0005-graph-sync.md` in `rusty_remind_me`
  (the owner's own ADR on the graph tables — read directly).
- Source read: `baileyrd/rusty_remind_me` at `29602f1` (public,
  shallow clone; `crates/remind_me_core/src/entity.rs` 908 lines,
  `models.rs`, `db/schema_tables.sql`, `db/schema_indexes.sql`,
  `tests/entity_traverse_test.rs`, `tests/entity_id_test.rs`,
  `crates/remind_me_mcp/src/lib.rs`'s `remind_me_entity_traverse`
  handler, `docs/adr/0005-graph-sync.md`). Every finding cites a
  file:line in that checkout. Not read: `sync/graph.rs`,
  `graph_sync_test.rs`, the hub/API crates — sync and HTTP transport
  are out of this crate's scope; named, not skipped silently.
- Supersedes: none. Findings only; proposes two bounded follow-ups
  and names the larger divergences without building them.

## Purpose and scope

Unit 41 verified `Entity`/`traverse` against `rusty_remind_me`'s MCP
tool *schemas* and got the shape directionally right, but a schema
cannot show relation labels, normalization rules, identity derivation,
or storage. Every design round since (`ADR-0039`, `ADR-0040`,
`ADR-0041`) carried the same explicit caveat — the source was never
read — and `MentionedWith` was shipped as an honest placeholder label
for exactly that reason. The owner's ordered pick "1b" is this round:
read the source, compare it against what was built, and say precisely
where the two agree, where they diverge, and which divergences are
worth closing.

Ten findings below. Two are small enough to propose as bounded
follow-ups (`ENT5-FR-001`/`002`); the rest are named plainly as real,
larger divergences for the owner to weigh, not built here.

## Non-goals

- **Not building anything this round.** Findings and two proposed
  follow-ups; implementation, if accepted, is its own unit.
- **Not matching `rusty_remind_me`'s id bytes.** Its ids are 12-hex
  sha256 prefixes (48 bits), a collision domain its own doc comment
  says is "inherited from the reference rather than chosen here"
  (`entity.rs:40-42`). This crate's `RecordId` is `Uuid`; a derived id
  here can share the *derivation principle* (deterministic from the
  normalized name) without inheriting the truncation. Byte-for-byte
  interop with its ids is impossible regardless (`Uuid` ≠ 12-hex
  `String`) and was never a goal.
- **Not adopting its sync/convergence machinery** (ADR-0005 there:
  outbox triggers, LWW, insert-or-ignore edges, no FKs by design) —
  entirely outside `SERVER-001`'s scope; cited only as the *reason* its
  identity is content-derived.
- **Not reopening `RecordId = Uuid`** — `ADR-0039`'s crate-wide
  invariant stands; Finding 2 shows it is a smaller divergence than
  previously framed.

## Findings

Each finding names what Unit 41 inferred, what the source shows, and
the delta against the merged shape (`FR-041`/`FR-042`).

**F1 — Normalization collapses internal whitespace; ours only trims.**
`normalize_entity_name` (`entity.rs:26-31`) is `split_whitespace().
collect::<Vec<_>>().join(" ").to_lowercase()` — every run of Unicode
whitespace (tabs, newlines, doubled spaces) becomes one space, then
lowercase. Its own doc comment (`entity.rs:19-22`): "An earlier version
only trimmed, which made `\"Bailey  Robertson\"` and `\"bailey
robertson\"` two entities here and one in `remind_me`." Pinned by
`entity_id_test.rs:36,49-53`. `FR-042`'s `normalize` (`src/generic/
store.rs`) is `trim().to_lowercase()` — exactly the version they
retired. Real, small, in-process-only divergence: `NameIndex` treats
`"Ada  Lovelace"` and `"Ada Lovelace"` as different keys today.
**→ `ENT5-FR-001`.**

**F2 — Identity is a derived content-hash id, not a raw name.**
`entity_id(name) = sha256(normalize_entity_name(name))[..12]`
(`entity.rs:44-46`); `entities.id TEXT PRIMARY KEY` (`schema_tables.sql:
36`). Canonical-name lookup is an indexed PK hit by *re-deriving* the
id (`get_entity_by_name`, `entity.rs:195-197`); only *alias* lookup
scans (`resolve_entity`, `entity.rs:212-241`). Unit 41 Finding 1
("addressed by `name`/`aliases`, not `Uuid`") was true at the tool
boundary but the system is id-keyed underneath — the id is simply
*derivable*. Delta: this crate's `Entity.id` is an arbitrary caller-
supplied `Uuid`, so name→record is two round trips (`FilterEq` then
`GetById`) — the open question `ADR-0039`/`ADR-0040` both left
standing. A `Uuid` derived deterministically from the normalized name
(full 128-bit digest, not the 48-bit truncation — see Non-goals) makes
it one indexed `GetById`, with `RecordId` untouched and no wire change.
**→ `ENT5-FR-002`.**

**F3 — There is no relation vocabulary. Labels are free-form, per
triple.** `entity_relations.relation TEXT NOT NULL` (`schema_tables.sql:
48`); written by `maybe_link_entity_relation` (`entity.rs:809-826`) from
a memory's free-text SPO *predicate* whenever both subject and object
resolve to known entities; stored with whitespace collapsed but case
preserved (`entity.rs:780`); the deterministic edge id uses the
lowercased label (`entity_relation_id`, `entity.rs:753-766`). No enum,
no registry, no allowed-list, no `list_relation_kinds` analogue. Tests
use `"knows"`, `"works_with"`, `"introduced"` (`entity_traverse_test.rs`)
— arbitrary. The traverse filter is `r.relation = ?` (`entity.rs:487`),
exact and **case-sensitive** against the stored label: `relation:
"Knows"` misses `"knows"`. Consequence for this crate: Unit 41 Finding 6
("multiple named relation types are real") was correct; the inference
`ADR-0039` built on — that they form a *fixed set knowable at
construction* — was not. `MultiSymmetric` (`FR-041`) is built from a
fixed `[(label, edges)]` list, `RELATION_LABELS` is a compile-time
`[&str; 2]`, and there is no add-edge or add-label path. `MentionedWith`
/`mentioned_with` and `relates_to` are confirmed placeholders — but the
placeholder *strings* were never the mismatch; the *fixed-set model*
is. Named, not built: open-label edges created at write time would be
a redesign of `MultiSymmetric`'s construction model and of `Request::
ListRelationKinds` (which would become `SELECT DISTINCT relation`), a
design round of its own.

**F4 — Edges are directed triples; the walk is bidirectional; the
returned edge keeps its direction.** `(subject_entity_id, relation,
object_entity_id)` stored once (`schema_tables.sql:45-53`), indexed on
both endpoints (`schema_indexes.sql:16-20`). `traverse_entities`
(`entity.rs:447-556`) queries `WHERE subject IN (..) OR object IN (..)`
— reachability is undirected — but every `RelationEdge` it returns
carries `subject_*`/`relation`/`object_*`/`hop` (`entity.rs:436-445`),
so the caller sees who is subject. Delta: `MultiSymmetric` stores
symmetric adjacency and returns bare neighbor ids — direction is not
representable. Also answers `FR-041`'s per-edge-metadata open item:
real edges carry `id`, `created_at`, `updated_at`, `node_id` —
timestamps and sync provenance, **no weight**. Named, not built.

**F5 — The traversal result is edges-with-hops plus entity refs, not
ids-with-depths.** `EntityTraverseResult { found, entity: EntityRef,
hops, edges: Vec<RelationEdge>, entities: Vec<EntityRef{id,name,kind}> }`
(`entity.rs:651-668`). Termination: each hop queries only the entities
*newly discovered* by the previous hop, so the seed never re-enters a
frontier and a cycle yields an empty frontier; `seen_edges` dedupes an
edge found from both endpoints within one hop (`entity.rs:463-466`).
`cap` bounds **edges** across all hops (default `RELATION_TRAVERSAL_CAP
= 20`, clamped `1..=100`); `hops` clamped `1..=3` — clamped, never
rejected (`entity.rs:697-698`, pinned by `entity_traverse_test.rs:
128-140`). Delta: `SchemaDrivenClient::traverse` returns `Vec<(RecordId,
usize)>` — ids and shortest-path depth, no edges, no labels, no names;
`max_depth`/`max_nodes` are caller-supplied and unclamped and bound
*nodes*, not edges. Confirms Unit 41 Finding 5 and sharpens it: the
unit of the bound differs (edges vs. nodes), not just its enforcement.
Named, not built.

**F6 — `kind` is `Option<String>`, indexed, with an "existing wins"
merge.** `kind TEXT DEFAULT NULL` (`schema_tables.sql:38`), `idx_
entities_kind` (`schema_indexes.sql:7`); on upsert "existing kind wins,
input only fills a hole" (`entity.rs:100-101`). Delta: ours is a
required `String`, read-only over the wire, equality-indexed — the
index agrees; nullability and the merge rule do not apply (no upsert
path here). Minor.

**F7 — Aliases: JSON-array column, trimmed, empties dropped, order-
preserving dedup, union-merged on upsert; canonical name beats alias
on resolve.** `aliases TEXT NOT NULL DEFAULT '[]'` (`schema_tables.sql:
39`); write path `entity.rs:62-68,93-98`; resolution `entity.rs:212-
241` — a canonical-name match anywhere beats an alias match found
earlier, and exactly *one* entity is returned. Delta: `FR-042` stores
aliases raw (no trim/dedup on write) and `FilterEq` on `label` returns
*every* entity sharing a normalized key — a deliberate Non-goal there
("collision handling is the caller's"). The two systems agree on what
matches and differ on how many come back. Named; not proposed to
change — returning all is the more honest primitive, and a caller who
wants "canonical beats alias" can rank client-side.

**F8 — Entity enumeration exists in core; it is only absent as an MCP
tool.** `list_entities(conn, limit, offset)` (`entity.rs:390`) and
`EntityListResult` (`entity.rs:376`) are real and paged. Refines Unit 41
Finding 4 ("no entity-enumeration tool exists"): true of the MCP
surface, false of the system. This crate's `Request::Query`/`AllIds`
already cover enumeration — no delta.

**F9 — No `mention_count`; mentions are a link table plus a derived
total.** `memory_entities(memory_id, entity_id, created_at)` (`schema_
tables.sql:80-85`); `EntityProfile { entity, facts, memories, total_
linked_memories }` (`entity.rs:270-281`) — the count is *derived* at
read time, never stored. Confirms Unit 41 Finding 3. Delta: `Entity::
mention_count: i64` remains the synthetic `ScannableField` `FR-041`
kept only because `GenericMmapStore` structurally requires one
`Copy`-typed scannable field. Named, unchanged: the real analogue is a
derived count over a link relation this crate does not model.

**F10 — Convergence, not convenience, is why identity is content-
derived.** `docs/adr/0005-graph-sync.md`: deterministic ids let two
nodes that independently record the same entity or edge converge on
one row; relations are insert-or-ignore and immutable; no foreign keys
by design (sync delivers rows out of order). Out of this crate's scope
entirely — cited because it is the *reason* F2 and F3's shapes are what
they are, which matters when judging whether to copy them.

## Requirements (proposed follow-ups)

- `ENT5-FR-001` — **`NameIndex::normalize` collapses internal
  whitespace**: `key.split_whitespace().collect::<Vec<_>>().join(" ").
  to_lowercase()`, matching `normalize_entity_name` exactly. In-process
  only; no wire, schema, or on-disk change (the index is rebuilt from
  records at every open — `FR-042`'s own "no file of its own" property
  makes this a zero-migration change). The `entity.rs` module doc's
  Non-goal ("ASCII-oriented... not full Unicode case folding") still
  holds — `to_lowercase` is unchanged; only the whitespace rule widens.
- `ENT5-FR-002` — **`Entity` ids derived deterministically from the
  normalized name**: a `pub fn entity_id(name: &str) -> Uuid` in
  `src/generic/entity.rs` — `Uuid::from_bytes` over the first 16 bytes
  of `sha256(normalize(name))` (full width, not the 48-bit truncation;
  see Non-goals) — used by `create_entity_production_stack`'s callers
  and documented as *the* way to mint an `Entity` id, so `GetById(
  entity_id("Ada Lovelace"))` is one indexed round trip and two
  machines minting the same name converge. `RecordId` stays `Uuid`; no
  wire change; existing tests' hand-picked `Uuid::from_u128(n)` ids
  keep working (the derivation is a convention, not a constraint the
  store enforces — matching how `rusty_remind_me`'s own `upsert_entity`
  derives but its schema does not `CHECK`). `sha2` is not a current
  dependency — one new dev/prod dependency, named plainly, the first
  since `rusty_tls`.

## Considered options

**What to do with the ten findings.** **(a) (proposed) accept as
findings; take `ENT5-FR-001` and `ENT5-FR-002` as one small follow-up
unit; name F3/F4/F5 as the real divergences for a future design round
without building them now** — closes the two cheap, high-value deltas
(a one-line normalization fix that prevents the exact split the
reference already suffered; a derivation convention that collapses
name-lookup to one round trip) and leaves the open-label/directed-
edge/edge-result redesign as a decision with full information for the
first time. **(b) accept the findings; take only `ENT5-FR-001`** — the
normalization fix is unambiguous; deriving ids adds a dependency and a
convention the owner may not want. **(c) accept as informational; no
engine change** — `Entity` stands as built; the caveat "source unread"
is retired from every doc that carries it, nothing else moves.

Option (a) proposed.

**Whether to copy the 12-hex id truncation for interop.** **(a)
(proposed) no — full 128-bit `Uuid` from the same digest.** Byte
interop is impossible anyway (`Uuid` vs. 12-hex `String`); the *principle*
(deterministic, convergent) is what carries over, and the 48-bit domain
is one the reference's own author calls inherited, not chosen. **(b)
yes, pad a 48-bit prefix into a `Uuid`** — rejected: inherits a
collision domain for no interop gain.

Option (a) proposed.

## Proposed shape

```rust
// src/generic/store.rs — ENT5-FR-001
fn normalize(key: &str) -> String {
    key.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

// src/generic/entity.rs — ENT5-FR-002
/// The deterministic id for an entity name: `Uuid` over the first 16
/// bytes of `sha256(normalize(name))`. Two callers minting the same
/// name — in any casing or spacing — get the same id, so
/// `GetById(entity_id(name))` resolves a canonical name in one indexed
/// round trip. A convention, not a store-enforced constraint.
pub fn entity_id(name: &str) -> Uuid {
    let digest = sha2::Sha256::digest(normalize(name));
    Uuid::from_bytes(digest[..16].try_into().expect("32-byte digest"))
}
```

## Data/state and invariants

- `ENT5-FR-001`: no on-disk change — `NameIndex` is rebuilt from
  records at every open, so a reopened store simply builds the new map.
- `ENT5-FR-002`: no on-disk change — ids are still `Uuid`s in the same
  slots; only how a caller *chooses* them changes. Existing data with
  arbitrary ids stays valid.

## Errors, failure, recovery, and observability

- No new error surface. A name whose normalization is empty
  (`"   "`) derives a `Uuid` like any other; `NameIndex` already
  registers such a key harmlessly and `rusty_remind_me` rejects it
  upstream (`entity.rs:164`) — a caller-side validation this crate's
  store never had and this round does not add.

## Security, privacy, and compatibility

- No `PROTOCOL_VERSION` change, no wire change, no schema change.
- `sha2` as a new dependency (`ENT5-FR-002`) — named plainly; the first
  new `Cargo.toml` dependency since `rusty_tls` (`FR-019`). Pure Rust,
  RustCrypto, no build script. If the owner prefers zero new
  dependencies, option (b) drops `ENT5-FR-002` and with it the
  dependency.

## Acceptance criteria

1. `NameIndex` resolves `"Ada  Lovelace"`, `"Ada\tLovelace"`, and
   `" ada lovelace "` to the same id as `"Ada Lovelace"` — pinned by a
   unit test mirroring `entity_id_test.rs:49-53`'s three cases.
2. `entity_id("Bailey Robertson") == entity_id("  Bailey   Robertson  ")
   == entity_id("bailey robertson")`, and differs from `entity_id(
   "Bailey Robertson II")`.
3. An `Entity` created with `id: entity_id(&label)` is found by
   `GetById(entity_id(label_in_any_casing))` over a real socket in one
   round trip — no `FilterEq`.
4. Every existing `Entity` test using `Uuid::from_u128(n)` ids passes
   unchanged.
5. Every doc comment and ADR that carries "that repository's own source
   never having been read" is updated to cite this document instead.

## Verification plan

- `src/generic/store.rs`: `normalize` unit test with the three
  whitespace cases plus the existing case-only cases.
- `src/generic/entity.rs`: `entity_id` determinism/distinctness test.
- `tests/server_entity_integration.rs`: one appended test for criterion
  3.
- Full sweep per this project's bar (`fmt`/`clippy -D warnings`/`test`/
  `test --all-features`/`doc` at the 64-warning baseline).

## Traceability

- → a new ADR (`ADR-0042`); if (a) or (b) is accepted, `SERVER-001`'s
  next minor / FR for the follow-up(s).
- Resolves by pointer the "source unread" caveat in `ADR-0039`,
  `ADR-0040`, `ADR-0041`, and `src/generic/entity.rs`'s module doc.
- Not sourced from `docs/FUTURE-GROWTH.md` — the eighth round in the
  `rusty_remind_me`-motivated line `ADR-0036` started.

## Open questions

- Whether to pursue open-label, directed edges (F3/F4) — a redesign of
  `MultiSymmetric`'s fixed-construction model and of `ListRelationKinds`.
  Real, larger, now decidable with full information; not proposed here.
- Whether `traverse` should return edges-with-hops and entity refs
  (F5) rather than ids-with-depths — would be the first `traverse`
  result-shape change since `ADR-0037`; not proposed here.
- Whether `mention_count` should become a derived count over a
  link relation this crate does not model (F9) — no; named as the
  reason it stays synthetic.
- `sync/graph.rs` and the hub/API crates remain unread — out of scope,
  not a gap in the entity-model findings above.

## Change history

- 2026-09-05: Initial findings, the eighth round in the
  `rusty_remind_me`-motivated line `ADR-0036` started — the source read
  directly at `29602f1`; ten findings, two proposed follow-ups
  (`ENT5-FR-001`/`002`), three larger divergences named for a future
  round.
- 2026-09-05: Accepted as proposed, `ADR-0042` option (a); (b) and (c)
  declined.
