# STORAGE-001 — Dataset generator and record model

- Version: 0.1.0
- Status: Accepted
- Owners: baileyrd
- Depends on: none
- Supersedes: none

## Purpose and scope

Define the fixed record shape under test and a deterministic, configurable
synthetic dataset generator that produces the exact same logical dataset
for all three backends being compared, so any measured performance
difference is attributable to backend design rather than input variance.

## Non-goals

- No generic/arbitrary schema support — the record shape is fixed (see
  ADR-0001 and the charter's non-goals).
- No persistence of generated datasets to disk; datasets are generated
  in-memory per benchmark run.
- No real-world data; all values are synthetic.

## Context and terminology

- **Cardinality / reuse ratio**: how many distinct `breed` values exist
  relative to record count. Low cardinality (few distinct breeds, heavily
  reused) is the case where normalization (a breed → UUIDs index, as in
  `CanonicalStore`) is expected to pay off; high cardinality (breed close
  to unique per record) is the case where it's expected to pay off less
  or not at all. Making this configurable is what lets the benchmark test
  that specific claim rather than assume it.

## Requirements

- `STORAGE-001-FR-001`: The record type is exactly:
  ```rust
  struct DogRecord {
      id: Uuid,
      breed: String,
      age: u32,
  }
  ```
- `STORAGE-001-FR-002`: The generator accepts record count (`n`), a breed
  cardinality parameter (number of distinct breed strings to draw from),
  and a seed, and is a pure function of those three inputs: the same
  `(n, cardinality, seed)` always produces the same sequence of records
  (same IDs, breeds, and ages in the same order).
- `STORAGE-001-FR-003`: Every generated `id` is unique within a single
  generated dataset.
- `STORAGE-001-FR-004`: `age` values are generated within a bounded,
  documented range (e.g. 0..=20) so `scan_ages`'s average is a meaningful,
  sanity-checkable number.
- `STORAGE-001-FR-005`: Breed values are drawn uniformly at random, with
  replacement, from a generated pool of `cardinality` distinct strings.
  With `cardinality << n` breeds repeat heavily; with `cardinality == n`
  breeds repeat far less, though not perfectly uniquely — sampling with
  replacement from a pool the same size as the draw count still produces
  collisions (the birthday-paradox expectation is ~63% distinct, i.e.
  `n * (1 - 1/e)`, not 100%). Both ends of the range must be reachable by
  configuration, not hard-coded; exact per-record uniqueness at
  `cardinality == n` is explicitly not a requirement.
- `STORAGE-001-NFR-001`: Generating 1,000,000 records completes in a few
  seconds at most on ordinary developer hardware (not a hard gate, just a
  sanity bound so it doesn't dominate benchmark iteration time).

## Architecture and interfaces

`src/record.rs` defines `DogRecord`. `src/generator.rs` defines a
`GeneratorConfig { n: usize, breed_cardinality: usize, seed: u64 }` and a
`generate(config: &GeneratorConfig) -> Vec<DogRecord>` function using a
seeded PRNG (`rand`'s `StdRng::seed_from_u64` or equivalent — not
`thread_rng`, which is not reproducible).

## Data/state and invariants

- Invariant: no duplicate `id` within one generated `Vec<DogRecord>`.
- Invariant: `breed_cardinality >= 1` when `n >= 1` (an empty breed pool
  with records requested is a configuration error, not a panic — see
  below).

## Errors, failure, recovery, and observability

- `breed_cardinality == 0` while `n > 0` is a configuration error.
  `generate` is designed to make this state unreachable via its type
  (e.g. validated at construction) rather than requiring a runtime
  `Result`, since this is a benchmark-input concern, not a user-facing
  fallible operation. If validation is needed, it returns `Result`, not a
  panic — no `unwrap()`/`expect()` outside tests per the project's
  engineering constraints.

## Security, privacy, and compatibility

Not applicable — synthetic in-memory data only, no persistence, no
external input.

## Acceptance criteria

- Two calls to `generate` with identical config produce identical output
  (byte-for-byte equal `Vec<DogRecord>`).
- Two calls with different seeds (same `n`, `breed_cardinality`) produce
  different output.
- Generated `id`s are unique for `n` up to at least 1,000,000.
- With `breed_cardinality = 1`, every record has the same breed. Raising
  `breed_cardinality` toward `n` strictly increases the number of distinct
  breeds observed, approaching but not reaching full per-record uniqueness
  (sampling with replacement — see FR-005).

## Verification plan

Unit tests in `src/generator.rs` covering: determinism given a fixed seed,
uniqueness of generated IDs, cardinality bounds (min and max), and a
boundary case (`n = 0`).

## Traceability

Implements: dataset generator deliverable in the originating task.
Feeds: `STORAGE-002` (backends are constructed from generator output),
`STORAGE-003` (benchmarks parameterize over generator config).

## Open questions

- Should breed string *length* also be configurable (currently fixed
  short synthetic strings)? Deferred — not needed to test the
  cardinality/normalization question this pass is scoped to.

## Change history

- 0.1.0 (2026-08-24): Initial accepted draft.
