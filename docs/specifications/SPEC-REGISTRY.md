# Specification Registry

Status vocabulary: `Proposed`, `Draft`, `Accepted`, `In Progress`,
`Implemented`, `Verified`, `Blocked`, `Deferred`, `Deprecated`,
`Superseded`.

| ID | Title | Version | Design | Implementation | Verification | Depends on | Owner | Location | Evidence |
|---|---|---:|---|---|---|---|---|---|---|
| `STORAGE-001` | Dataset generator and record model | 0.1.0 | Accepted | Implemented | Verified | — | baileyrd | `docs/specifications/storage/STORAGE-001-dataset-generator.md` | `src/generator.rs` tests |
| `STORAGE-002` | `DogStore` trait and three backend implementations | 0.1.0 | Accepted | Implemented | Verified | `STORAGE-001` | baileyrd | `docs/specifications/storage/STORAGE-002-dogstore-backends.md` | `src/store/**` + `tests/cross_backend.rs` |
| `STORAGE-003` | Criterion benchmark suite and cache-miss instrumentation | 0.1.0 | Accepted | Implemented | Verified (wall-clock); cache-miss counters not producible in this session's environment, see ADR-0002/RESULTS.md | `STORAGE-001`, `STORAGE-002` | baileyrd | `docs/specifications/storage/STORAGE-003-benchmark-suite.md` | `cargo bench` run, `RESULTS.md` |
| `STORAGE-004` | Results and decision writeup | 0.1.0 | Accepted | Implemented | Verified | `STORAGE-003` | baileyrd | `docs/specifications/storage/STORAGE-004-results-writeup.md` | `RESULTS.md` |
| `STORAGE-005` | `CanonicalCachedStore`: canonical store with a materialized age cache | 0.1.0 | Accepted | Implemented | Verified | `STORAGE-001`, `STORAGE-002` | baileyrd | `docs/specifications/storage/STORAGE-005-canonical-cached-backend.md` | `src/store/canonical_cached.rs` tests + `tests/cross_backend.rs` + `RESULTS.md` |

IDs remain stable. Link superseding artifacts rather than reusing an ID for
a new meaning.
