# Traceability

| Requirement | Roadmap | Decision/interface | Implementation | Verification | PR/release | State |
|---|---|---|---|---|---|---|
| `STORAGE-001-FR-001..005` | `GENERATOR` | `DogRecord`, `GeneratorConfig`/`generate` | `src/record.rs`, `src/generator.rs` | unit tests in `src/generator.rs` (8 tests, all passing) | this PR | Implemented/Verified |
| `STORAGE-002-FR-001..007` | `BACKENDS` | ADR-0001, `DogStore` trait | `src/store/mod.rs`, `src/store/{aos,soa,canonical}.rs` | unit tests (18) + cross-backend tests (4), all passing | this PR | Implemented/Verified |
| `STORAGE-003-FR-001..005` | `BENCH-SUITE`, `CACHE-MISS` | ADR-0002 | `benches/workloads.rs`, `benches/cache_events.rs`, `src/bench_support.rs` | `cargo bench` run (36/36 cases); `--features perf-events` build verified on Linux, runtime deferred to `baileyai` | this PR | Implemented/Verified |
| `STORAGE-004-FR-001..006` | `RESULTS` | ADR-0001, ADR-0002 | `RESULTS.md` | manual acceptance review against `STORAGE-004` criteria | this PR | Implemented/Verified |
| `STORAGE-005-FR-001..006` | `HYBRID-BACKEND` | ADR-0003, `DogStore` trait | `src/store/canonical_cached.rs` | unit tests (8, staleness test highest-priority) + cross-backend tests (5, incl. `all_backends_reflect_update_age_in_scan_ages_immediately`) + `cargo bench` 4-way run, all passing | follow-on PR | Implemented/Verified |

Trace both directions: requirements reach evidence, and material code has
a requirement, defect, maintenance policy, or ADR. Update this table in
the same PR that changes implementation state.
