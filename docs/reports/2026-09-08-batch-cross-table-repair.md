# Batch cross-table repair — 2026-09-08

Step 1 part A, SERVER-001 v0.50.1. Baseline:
`478eeda4544aa98c4948aebc66b5ff8fad609645` (PR #229). Work-order SHA256:
`0d6affb507031c2261bfc5ea159be97e7c052c5e9611f17dafdbfa21f17f3112` (verified).
All edits are in this checkout; no commit, push or publication.

## Repair and regressions

Batch Link previously bypassed foreign-endpoint validation, accepting missing
entities or an unregistered entity table. Batch Delete bypassed cross-table
detaches, leaving memory `mentions` adjacency and counts behind. Both modes now
use the single-request checks/cascade, with ordered atomic preflight and prior-op
existence tracking. Memory checks its local left endpoint and the foreign entity;
Entity checks both local endpoints. If an explicit target names the selected
table, the server's right-endpoint overlay also tracks prior inserts/deletes.

Accepted socket regressions in `tests/server_memory_integration.rs`:

- `batch_cross_table_link_delete_matches_single_and_survives_reopen`: all three
  request modes, reverse adjacency, Join, CountEdges, NotFound and reopen.
- `batch_cross_table_preconditions_and_dependencies`: missing table/endpoint,
  Insert/Link and Delete/Link, independent pipelined results and atomic precondition
  rejection without writes.
- `batch_entity_dependencies_rejection_preserves_cross_table_edges`: both entity
  endpoints, first-failure index, and no cross-table changes on rejection.
- `batch_concurrent_link_and_delete_leave_no_dangling_edges`: competing socket
  Link/Delete writers finish without dangling adjacency/counts. Both writer
  threads are joined; writer panics propagate to the test body.

The old single-table batch test now expects Failed(Unsupported). Wire vectors
and Python fixtures remain unchanged. Unit tests cover detach failures,
NotFound skipping detaches, and the explicit self-target existence overlay.

In `relationship_section_allows_reads_and_recovers_poison`, a socket read
completes while the relationship mutex is held elsewhere; after deliberately
poisoning that unit mutex, Delete reaches its adapter and the connection remains
usable. The Reminder socket tests `reminder_batches_apply_all_record_write_kinds`
and `reminder_batch_rejection_is_atomic_and_pipelined_results_are_independent`
cover atomic success, ordered Insert/Replace/ReplaceIf/Delete outcomes,
malformed field lists and status discriminants, and independent pipelined writes.

## Corrected atomic contract

See [ADR-0060 acceptance](../decisions/ADR-0060-write-batch.md#acceptance-and-implementation).
Only Link, Delete and WriteBatch share a server relationship mutex, and only
when a registered relation declares a target_table. Registries without such
relations skip the section, as pinned by
`relationship_section_depends_on_foreign_relations`. Only Delete
(single or batched) removes a record, so a passed far-endpoint check cannot become
dangling without a delete, and all deletes take the same section. Verified paths:
`GenericProductionStore::{insert,replace,replace_if}` preserve existing identity;
`MultiSymmetric::replace` forwards without changing adjacency; UpdateField and
transaction/session Commit apply field updates only. `MultiSymmetric::compact`
rewrites existing live adjacency and inner records without removing live state.
Compact, reads, sessions and other writes therefore remain outside this mutex.

Lock order is relationship mutex -> one adapter's internal sections. Far reads
release their locks before own-table apply. Detaches visit registered tables in
order after successful own-table apply, inside the relationship section but
outside the own-table exclusive section. No path holds two table locks or takes
the relationship mutex from an adapter/journal section, so Link/Delete, Commit
and other batches cannot introduce a multi-table lock cycle. Poison is recovered
with `unwrap_or_else(|p| p.into_inner())`, since the mutex protects unit state.

Memory, Entity, Relation, Reminder and Employee run ordered atomic preflight/apply under
one own-table exclusive section. Rejection names the earliest op and changes no
table. Dog/Order refuse nonempty socket atomic batches at index 0 with
Unsupported, replacing their baseline per-op/abort fallback; empty atomic batches
succeed. Order has no link_records override. Employee supports only
collaborates_with links, checks both endpoints through an existence overlay,
and rejects self-loops or any non-Link op before applying anything. Since
Employee accepts only Link, earlier ops cannot change record existence.
Reminder has no links and rejects Link during preflight. Pipelined ops
retain independent outcomes. Individual adapter reads remain consistent; a read
spanning tables may see deletion before detaches finish. This is not global
snapshot isolation. Direct writes through retained handles and other server
instances do not participate in this server's relationship mutex.

Detach Unsupported/Malformed is skipped; after successful own-table apply,
Storage returns BatchResults with Failed(Storage) in the Delete slot in either
mode, keeping the actual results elsewhere and attempting subsequent detaches.
Precondition failures keep TransactionFailed. The record, successfully
applied own-table ops and earlier detaches remain applied. A mid-apply own-table
Storage failure bypasses the detach pass. No I/O rollback or batch crash-atomicity
is claimed; crashes can leave stale adjacency/counts while Join skips absent rows.

Pipelined batches of up to MAX_BATCH_OPS ops still hold the relationship section
for their full duration on servers with foreign relations. A `benches/server.rs`
measurement of this cross-table serialization remains future work.

Pipelined ReplaceIf now validates the guard in the common apply_write_op path,
matching single requests and preserving dispatch parity. The Reminder socket
regression sends an unknown numeric guard field and expects UnknownField in
both the single response and the pipelined Failed slot.

## Authorized durability-helper deviation and CI

The `src/generic/insert_log.rs` write-handle fix was authorized by the host
coordinator in the build feedback of 2026-09-08 (F3), outside the work order's
file list. The host will commit it separately from the batch repair. The helper
opens its temporary file with write access before `sync_data`, fixing Windows'
rejection of a read-only handle. No log format, replay or snapshot semantics
change. The existing version-1 upgrade test covers this path. The `dog_server` certificate-path colon
split and live Python test's `python3` executable name remain unchanged, as directed;
the host excludes these two known Windows-only failures from acceptance.

The two accepted Clippy fixes use `as_chunks` (stable in Rust 1.88); no dependency,
protocol, toolchain or rust-version change. CI already has the required lint sets,
tests and MSRV job, so its commands are retained. Pinning stable to reduce future
lint drift remains a separate owner decision. The supplied Rust 1.88 GNU check
passed; this revision does not repeat that check.

## Pending part B

The source merge plan's Step 1 probes remain open and out of scope:

1. Rejected conflicting transaction reappears after reopen.
2. Delete after a journaled transaction breaks reopen with `Replay { RecordNotFound }`.
3. An older journal rewinds a later replacement after compaction.
4. A "snapshot" session observes a later commit.

See the local [transaction journal design](../design/SERVER-TRANSACTION-SESSION-DESIGN.md)
and [snapshot design](../design/SERVER-SESSION-SNAPSHOT-ISOLATION-DESIGN.md).
The external source plan is `data-os-multimodal-merge-plan-2026-09-08.md`, Step 1;
its original checkout was not edited.

## Files changed

- `src/server/serve.rs`: registry-aware batches, narrow relationship section,
  poison recovery, shared checks/detaches and regressions.
- `src/server/{memory,entity,relation,reminder}.rs`: ordered atomic preflight/apply.
- `src/server/employee.rs`: atomic Link preflight/apply and shared link path.
- `src/durability/mmap_store.rs`, `src/server/pem.rs`: accepted Clippy fixes.
- `src/generic/insert_log.rs`: F3 writable sync handle.
- `tests/server_memory_integration.rs`, `tests/server_reminder_integration.rs`:
  socket regressions.
- `docs/decisions/ADR-0060-write-batch.md`: corrected contract and adapter support.
- `docs/PROJECT-STATUS.md`, `docs/roadmap/ROADMAP.md`,
  `docs/specifications/server/SERVER-001-query-layer.md`,
  `docs/specifications/server/SERVER-002-wire-format.md`,
  `docs/specifications/SPEC-REGISTRY.md`, `docs/traceability/TRACEABILITY.md`:
  brief repair references; SERVER-001 is v0.50.1 and unit states are unchanged.
- This report.

## Baseline evidence (accepted, retained)

An isolated `git archive` of the baseline in `target/batch-repair-baseline`, with
only the new memory integration tests copied in, was compiled using a separate
`target/batch-baseline-build`. Both cross-table regressions failed: missing-endpoint
Link returned `[Inserted, Linked, Deleted]` rather than
`[Inserted, Failed(RecordNotFound), Deleted]`; deletion left nonempty neighbors.
Raw output: `target/batch-repair-baseline-isolated.log`. The host independently
confirmed the pre-existing insert-log Windows failure on that untouched source.
The earlier shared-target comparison reused artifacts and is not evidence.

## Verification

The served batch path is `write_batch_across`, which adds registry-aware Link
checks and Delete cascades. `dispatch` and `write_batch` remain table-local
low-level entry points. Tests pin Employee single/pipelined/atomic Link
agreement and rejection without writes; atomic detach Storage tests pin both
the failed Delete slot and preserved outcomes elsewhere.

The exact requested `&&` chain was run. Formatting and all three Clippy commands
passed; the server test command passed 522 library tests, then stopped at the
known Windows certificate-path test (exit 101). Client and Python commands were
therefore run separately. The server suite was rerun with only the two authorized
Windows exclusions; all remaining tests passed.

| Command | Actual result |
| --- | --- |
| `cargo fmt --all -- --check` | PASS; no output |
| `cargo clippy --all-targets --features server,research -- -D warnings` | PASS; finished in 45.49s |
| `cargo clippy --all-targets -- -D warnings` | PASS; finished in 10.08s |
| `cargo clippy --all-targets --features client -- -D warnings` | PASS; finished in 2.40s |
| `cargo test --features server,research` | Exit 101; library: 522 passed; dog_server: 3 passed, 1 known failure |
| Server tests with the two exclusions below | PASS, exit 0; 721 passed, 0 failed, 2 filtered across all targets |
| `cargo test --features client` | PASS, exit 0; 238 passed, 0 failed, no exclusions |
| `python -m unittest discover -s clients/python/tests -v` | PASS, exit 0; `Ran 5 tests in 0.009s`, `OK` |

The exclusion run used exactly:

```text
cargo test --features server,research -- --skip tests::certificate_classes_from_env_values_follows_the_documented_table --skip the_python_reference_client_speaks_the_protocol_at_22_and_at_10
```

Raw outputs: [exact chain](../../target/batch-repair-a5-exact-proof.log),
[server with exclusions](../../target/batch-repair-a5-server-proof.log),
[client](../../target/batch-repair-a5-client-proof.log), and
[Python](../../target/batch-repair-a5-python-proof.log).
The Employee agreement, conditional relationship section, and Reminder guard
regressions passed, as did all 17 memory socket tests.
The supplied Rust 1.88 GNU check passed; it was not rerun here.

No actions were denied in this revision. The two named Windows tests remain
acceptance exclusions, not passes. No additional deviations were introduced;
the authorized insert-log deviation and separate commit disposition are recorded
above. No commit, push, publication, dependency, toolchain or protocol change
was performed. The file list above covers the cumulative 19 changed files;
this revision updates Employee, the common server, Reminder tests, ADR-0060,
SERVER-001, SERVER-002, traceability and this report.
