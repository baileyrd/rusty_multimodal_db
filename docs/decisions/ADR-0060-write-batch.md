# ADR-0060: Batch the runtime writes in one request

- Status: **Accepted as designed** (2026-09-08 — the owner picked option
  (c): pipelined by default, atomic under a flag; (a) pipelined-only,
  (b) atomic-only, (d) session, and (e) decline declined). Proposed and
  then implemented on one branch once the pick was made.
- Date: 2026-09-08
- Deciders: baileyrd
- Related: `docs/design/SERVER-WRITE-BATCH-DESIGN.md` (the full design),
  `ADR-0013` (the field-update batch this generalizes), `ADR-0024`–
  `ADR-0026` (the session and redo journal it revisits), `ADR-0046`–
  `ADR-0051` (the runtime writes), `ADR-0054` (`ReplaceIf`, the
  per-record atomic compare-and-replace the consumer already has),
  `docs/reports/2026-09-07-hub-spike-report.md`.
- Supersedes/Superseded by: none. Would append one `Request` and one
  `Response` variant at protocol 22.

## Context

The hub's sync push applies a batch of heterogeneous records one at a
time — ~2 round trips per record — because the only batch on the wire
(`Request::Transaction`, `ADR-0013`) carries field updates only, not the
runtime writes (`Insert`/`Replace`/`ReplaceIf`/`Delete`/`Link`,
`ADR-0046`–`0051`) the push is made of. The unmet need is round trips.
Correctness is already covered per record: the hub's sync is
last-writer-wins per record, and `ReplaceIf` (`ADR-0054`) makes one
record's compare-and-replace atomic against a concurrent writer.

## Decision

Propose `Request::WriteBatch { ops: Vec<WriteOp> }` — `WriteOp` an enum
over the five runtime writes, each carrying the body of its single-shot
request — applied under one acquisition of the table's write lock, in
place of a round trip per record. Recommend **option (a), pipelined**:
answered `Response::BatchResults { results: Vec<WriteResult> }`, one
outcome per op in order, each identical to that op's single-shot
outcome; each op stands on its own, no cross-record atomicity, no
journal-format change. The atomicity guarantee is the fork, held for the
owner:

- **(a) Pipelined (recommended)** — per-op results, not atomic; matches
  the per-record LWW; no journal change.
- **(b) Atomic** — all-or-nothing (`Ok`/`BatchFailed { index, code }`);
  needs `JOURNAL_FORMAT_VERSION` 2 with idempotent redo, and is still
  not atomic against a mid-apply I/O failure without a storage rollback
  primitive (a larger round).
- **(c) Both** — (a) with an `atomic` flag selecting (b).
- **(d) Extend the session** to stage runtime writes — (b)'s atomicity
  plus interactive round trips the push does not want.
- **(e) Decline** — the push stays ~2N round trips; `ReplaceIf` already
  gave it per-record atomicity.

## Consequences

- Positive (a): the push goes from ~2N round trips to one batch, with no
  new guarantee to reason about — each op is its single-shot write,
  batched — and no journal, storage, or rollback change.
- Named, not hidden: (a) is not cross-record atomic. For a per-record
  LWW consumer that is the right trade; a consumer that needs
  all-or-nothing is the trigger for (b)/(c), a clean follow-on.
- Named, not hidden: even (b) is atomic only against precondition
  failures and (when journaled) crashes, never against a mid-apply
  durability error, until a storage rollback primitive exists.
- A wire change either way: protocol 22, one request and one response
  variant; a pre-22 client is unaffected (rule 3).

## Acceptance and implementation

- 2026-09-08: proposed, design only.
- 2026-09-08: the owner picked option (c). Implemented on the same branch
  as `SERVER-001` v0.50.0 / FR-060 — `WriteOp`/`WriteResult` and the two
  wire variants at protocol 22, `ConnectionStore::{apply_write_op,
  write_batch}` (pipelined default) with the atomic single-`with_exclusive`
  override on `Memory`/`Entity`/`Relation`, both clients, the pins and
  `SERVER-002` v0.11.0. The atomic flag is precondition- and
  isolation-atomic; crash-atomicity across the batch stays the named
  storage follow-on (option (c)'s journal-format path was scoped to that
  follow-on rather than built here, since the append logs fsync per op
  and per-op durability already holds). (PR #229.)

- 2026-09-08, Step 1 part A repair: the server now routes batch Link/Delete
  through registry-aware validation and cascades. `write_batch_checked` combines
  foreign and local preconditions in operation order before any write; prior
  inserts/deletes affect endpoint existence, including both Entity endpoints.
  The earliest rejection names its op and changes no table. An atomic adapter
  without this hook refuses with Unsupported without applying anything.
- One relationship mutex per `serve_tables` instance, allocated only when a
  registered relation declares a `target_table`, covers only Link, Delete and WriteBatch (both modes). Only Delete, single or
  batched, removes records; every such request takes this mutex, so a passed
  far-endpoint check cannot be invalidated before Link applies. Insert adds
  records; Replace/ReplaceIf preserve identity and adjacency; UpdateField,
  Transaction and Commit only update fields. MultiSymmetric's replacement
  forwards without touching adjacency, and its Compact rewrites the existing
  live edge blobs without deleting records or edges. Those requests, sessions,
  and reads stay outside the server section and use adapter locks alone.
- Registries without foreign relations skip the relationship section. On
  multi-table servers with foreign relations, pipelined batches of up to
  `MAX_BATCH_OPS` ops hold the section for their duration.
- Lock order remains relationship mutex then one adapter's internal sections.
  Far reads release their adapter locks before own-table apply. Detaches visit
  registered tables in order after own-table apply, inside the relationship
  mutex but outside the own-table exclusive section. No path holds locks on
  two adapters, and no adapter acquires the relationship mutex; Commit only
  acquires its journal/adapter sections and cannot form a lock cycle with
  Link, Delete or another batch. A poisoned unit mutex is recovered with
  `into_inner`, not treated as a persistent Storage refusal.
- Atomic batches remain precondition-atomic across the registry and isolated
  during own-table preflight/apply by the adapter's exclusive lock. Link/Delete
  and other batches cannot interleave through cross-table checks and detaches.
  Individual adapter reads remain consistent; reads spanning tables may observe
  the interval between own-table deletion and detaches. This is not a global
  read snapshot. Direct writes through retained store handles or other server
  instances remain outside the relationship section. Pipelined outcomes remain
  independent. No throughput claim is made.
- Memory, Entity, Relation, Reminder and Employee implement atomic
  preflight/apply under one `with_exclusive`. Reminder has no links and rejects
  Link with Unsupported during preflight. Employee supports only
  `collaborates_with` Link, checking both endpoints and self-loops before any
  apply; other write ops reject with Unsupported at their index. Order has no
  `link_records` override. Dog/Order, without an atomic
  implementation, refuse nonempty socket atomic batches at index 0 with
  Unsupported, applying nothing. This explicitly replaces the baseline trait
  fallback's per-op application with abort for those socket requests; empty
  atomic batches still succeed and pipelined requests keep per-op behavior.
- Detach Unsupported/Malformed is skipped. After successful own-table apply,
  a detach Storage failure returns BatchResults with Failed(Storage) in that
  Delete's slot in either mode, preserving the real results elsewhere and
  attempting subsequent detaches. Precondition failures keep TransactionFailed.
  The record is already gone; applied own-table ops and earlier detaches remain.
  An own-table mid-apply Storage failure leaves partial own-table effects and
  bypasses the detach pass. No I/O rollback or crash-atomicity is claimed;
  crashes can leave adjacency/CountEdges dangling while Join skips absent rows.
  Recovery/isolation probes and the snapshot contract remain pending part B.
  See [implementation notes and regressions](../reports/2026-09-08-batch-cross-table-repair.md).

- The `src/generic/insert_log.rs` write-handle fix was authorized by the host
  coordinator in the build feedback of 2026-09-08 (F3), outside the work order's
  file list. The host will commit it separately from the batch repair. It opens
  the upgrade temporary file with write access before syncing, without changing
  the log format or replay semantics.
