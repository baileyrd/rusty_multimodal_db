# AGENTS.md

## Scope

Applies to the whole repository.

## Project shape

- Purpose: empirically benchmark three storage-layout backends (AoS, SoA,
  UUID-canonical-store-with-views) behind one `DogStore` trait, to decide
  whether a UUID-canonical store can serve row/column/graph access as
  views rather than physical copies. See `docs/charter/CHARTER.md`.
- Rust structure: single crate at repo root (`rusty_multimodal_db`), no
  workspace — this is a small, single-purpose experiment, not a platform.
  `src/` is the library (`record`, `generator`, `store` + three backend
  submodules); `benches/` holds the Criterion suite.
- Architectural boundaries: see
  `docs/architecture/SYSTEM-ARCHITECTURE.md` — one-way dependency
  direction (backends depend on `store`/`record`, never the reverse), and
  the "views, not copies" boundary for `CanonicalStore` (ADR-0001).

## Coordination

Follow `WORKFLOW.md` for handoffs and review — it governs process, not
project architecture.

## Canonical commands

- Format: `cargo fmt --all -- --check`
- Lint: `cargo clippy --all-targets --all-features -- -D warnings`
- Test: `cargo test --all-features`
- Docs/build: `cargo doc --all-features --no-deps` and `cargo check
  --all-targets`
- Benchmarks (wall-clock, cross-platform): `cargo bench`
- Benchmarks (cache-miss, Linux bare-metal only): `cargo bench --features
  perf-events --bench cache_events` — see ADR-0002 before relying on this
  anywhere else.

## Change rules

- `Result` + `?` over panics; `unwrap()`/`expect()` only inside `#[cfg(test)]`.
- Tests for all non-trivial logic: happy path plus at least one
  boundary/failure case.
- Docstrings (`///`) on public functions/structs/modules — this repo is
  past the initial spike as of the `GENERATOR` unit onward.
- No speculative generality: `DogRecord`'s three fields are fixed; don't
  add a generic schema/column-type system without an explicit,
  user-approved ADR first (record-shape changes are called out as
  hard-to-reverse in the charter).
- New dependencies must be justified in one line in the commit message or
  an ADR. Prefer the standard library; check `docs/decisions/` before
  assuming a crate is needed if something similar was already evaluated.
- This is a standalone experimental repo, not a `rustils` consumer —
  `rustils`'s RFC v2 consumer-gate rule does not apply here.
- Flat control flow, guard clauses over deep nesting.
- Update `docs/PROJECT-STATUS.md`, the roadmap, and
  `docs/traceability/TRACEABILITY.md` when a roadmap unit's state changes.

## Definition of done

- Tests pass (`cargo test --all-features`), lints clean
  (`cargo clippy ... -D warnings`), formatting clean (`cargo fmt --check`).
- Public items documented.
- Roadmap/spec-registry/traceability/status updated in the same PR (or an
  immediate follow-up) when the unit changes state.
- An ADR exists for any new consequential design choice per
  `docs/decisions/` cadence guidance (this project is in active
  bootstrap/major-development, so default to writing one — see the
  `rust-repo-lifecycle` skill's `adr-cadence.md`).
