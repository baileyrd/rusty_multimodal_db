# Project Charter

## Name

`rusty_multimodal_db` (GitHub repo name; see "On the name" below).

## Purpose

This is a **benchmark harness**, not a database product. It exists to
empirically test one hypothesis before anyone commits engineering time to
building a real storage engine around it:

> A canonical store, keyed by UUID, can serve as the single source of truth
> for row-oriented, column-oriented, and graph-oriented access — with
> row/column/graph implemented as **views** over that one canonical store
> rather than as separate physical copies of the data.

The alternative, conventional designs under test alongside it:

- **AoS** (array of structs) — classic row-oriented storage. Fast
  full-record reads, slow full-column scans.
- **SoA** (struct of arrays) — classic column-oriented storage. Fast column
  scans, slow full-record reconstruction.

## Users

The sole user is the repo owner (baileyrd), deciding whether to invest in a
UUID-canonical storage design for a future project. There is no external
audience and no production deployment target.

## Primary use case

Run the same generated dataset and the same workloads (point read, column
scan/aggregate, single-field update, one-hop "same breed" lookup, and —
added once a real edge relationship existed to traverse — one-hop and
two-hop `littermate_of` graph traversal) against all backend
implementations behind one trait, at three dataset sizes, and produce
numbers — wall-clock and, where the platform allows, cache-miss counts —
that support a real decision.

## Product shape

A Rust library crate (`DogStore` trait + three implementations + dataset
generator) plus a Criterion benchmark suite plus a `RESULTS.md` writeup.
No server, no persistence, no network surface, no CLI beyond what's needed
to drive benchmarks. Everything lives in memory for the duration of a
benchmark run.

## Explicit non-goals

- Not building a production database or storage engine.
- Not implementing a generic arbitrary-schema system. The record type is
  fixed at three fields (`id: Uuid`, `breed: String`, `age: u32`) for this
  pass — see ADR-0001 and the "avoid speculative generality" engineering
  constraint.
- Not implementing a general-purpose graph query language or arbitrary-hop
  traversal. `same_breed` remains a narrow shared-attribute stand-in, not a
  real edge; one real edge relationship (`littermate_of`) and one-hop
  (`DogStore::neighbors`) plus generically-composed two-hop traversal were
  added as a scoped follow-on once that relationship existed (see
  `STORAGE-006` and ADR-0004) — still not a general graph engine, multiple
  relationship types, or N-hop (3+) traversal.
- Not implementing persistence, durability, transactions, or concurrency
  control. Everything is single-threaded, in-memory, and rebuilt fresh per
  benchmark iteration.
- Not picking a winner on theory. The UUID-canonical-store idea is a
  hypothesis under test, not a foundation to defend. A loss on some
  workload is a valid, useful result. A hybrid outcome (canonical store
  wins some workloads, a baseline wins others) is an acceptable and likely
  finding.

## Success measures

- All three backends implement the same `DogStore` trait and pass the same
  unit test suite.
- The benchmark suite runs all four workloads against all three backends
  at 1K / 100K / 1M records, cleanly parameterized so results are directly
  comparable.
- `RESULTS.md` states a verdict **per workload**, not one overall winner,
  and explicitly calls out any workload where the canonical-store approach
  loses to a naive baseline and any workload where it wins clearly.
- The cache-miss instrumentation question (Windows-native vs. Linux `perf`)
  is answered and documented, not silently skipped.

## Constraints

- Primary dev machine is native Windows, not WSL2. `perf stat` is not
  directly available there; see ADR-0002 for how cache-miss instrumentation
  is handled.
- A Fedora Server machine (`baileyai`) is available for Linux-only
  measurement if needed.
- Minimal dependencies; each new crate is justified in the commit that
  introduces it or in an ADR.
- `Result` + `?` over panics; no `unwrap()`/`expect()` outside tests.

## Ownership, license, data classification

- Owner: baileyrd (single maintainer).
- License: MIT (matches the `rusty_*` ecosystem convention).
- Data classification: none. The dataset is synthetic and generated at
  benchmark time from a seeded RNG; no real or sensitive data is ever
  involved.

## On the name

The task that seeded this repo suggested `rusty_multimodel_bench`
(multi-**model**, i.e. row/column/graph access models) as a name that
better signals "storage-layout benchmark" and avoids reading as an
ML/AI "multimodal data" (text+image+audio) project. The GitHub repository
was created as `rusty_multimodal_db` before that naming discussion
happened, and the repo cannot be renamed from inside a session — so this
charter records the discrepancy rather than silently picking one. If the
owner wants to rename the GitHub repository to `rusty_multimodel_bench` (or
another alternative), that's a manual step on github.com; the local
history and remote will need `git remote set-url` afterward. Until then,
the crate itself is named `rusty_multimodal_db` to match the repository,
and this file is the record of why.
